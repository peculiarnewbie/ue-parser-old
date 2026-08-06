use std::env;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use serde::Serialize;
use uasset_parser::asset::{
    AssetDecodeContext, AssetErrorKind, DecodedAsset, EnumCppForm, decode_export,
};
use uasset_parser::asset::{
    DATA_ASSET_CLASS, PRIMARY_DATA_ASSET_CLASS, SKELETON_CLASS, USERDEFINEDENUM_CLASS,
    USERDEFINEDSTRUCT_CLASS,
};
use uasset_parser::package::{PackageError, PackageErrorKind, PackageIndex, TableLocation};
use uasset_parser::property::{PropertyRecord, PropertyValue, RawReason};
use uasset_parser::schema::{ClassSchema, SchemaProvider, StructSchema};
use uasset_parser::{Package, PackageSummary};
#[cfg(feature = "utrace")]
use utrace_parser::utrace::{
    CpuTimelineIndexInfo, CpuTimelineQuery, CpuTimelineQueryResult, TimelineIndexError,
    TimelineIndexRequest, TraceCoverage, TraceDashboard, TraceError, TraceErrorKind, TraceInspect,
    TraceInventory,
};

const SCHEMA_VERSION: u32 = 6;
#[cfg(feature = "utrace")]
const UTRACE_SCHEMA_VERSION: u32 = 2;
const EXIT_SUCCESS: u8 = 0;
const EXIT_MALFORMED: u8 = 2;
const EXIT_UNSUPPORTED: u8 = 3;
const EXIT_IO: u8 = 4;
const EXIT_INTERNAL: u8 = 5;
/// At least one export failed to decode but the package and other exports parsed.
const EXIT_PARTIAL: u8 = 6;
const EXIT_RESOURCE_LIMIT: u8 = 7;
const EXIT_USAGE: u8 = 64;

fn main() -> ExitCode {
    ExitCode::from(run(env::args_os().skip(1).collect()))
}

fn run(arguments: Vec<OsString>) -> u8 {
    match Command::parse(arguments) {
        Ok(Command::Help) => write_stdout(HELP.as_bytes()),
        Ok(Command::Version) => {
            write_stdout(format!("uasset {}\n", env!("CARGO_PKG_VERSION")).as_bytes())
        }
        Ok(Command::Inspect(options)) => inspect(&options),
        Ok(Command::Authoring(options)) => authoring(&options),
        Ok(Command::Utrace(command)) => run_utrace(command),
        Err(error) => {
            eprintln!("uasset: {error}\n\n{USAGE}");
            EXIT_USAGE
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OutputFormat {
    Text,
    Json,
}

#[derive(Debug, Eq, PartialEq)]
struct InspectOptions {
    input: Input,
    format: OutputFormat,
    /// When set on `utrace dashboard`, retain a bounded CPU timeline for this frame.
    timeline_frame: Option<u32>,
    timeline_limit: usize,
    max_frames: usize,
    gpu_timeline_frame: Option<u32>,
    gpu_timeline_limit: usize,
    /// When set on `dashboard-progress`, write a bounded capture-wide CPU index.
    timeline_index_output: Option<PathBuf>,
    timeline_index_max_intervals: usize,
    /// Repeatable local symbol search roots (`--symbol-path`). Never network.
    symbol_paths: Vec<PathBuf>,
}

#[derive(Debug, Eq, PartialEq)]
enum Input {
    File(PathBuf),
    Stdin,
}

impl Input {
    fn display_name(&self) -> String {
        match self {
            Self::File(path) => path.to_string_lossy().into_owned(),
            Self::Stdin => "-".to_owned(),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
enum Command {
    Inspect(InspectOptions),
    Authoring(InspectOptions),
    Utrace(UtraceCommand),
    Help,
    Version,
}

#[derive(Debug, Eq, PartialEq)]
enum UtraceCommand {
    Inspect(InspectOptions),
    Dashboard(InspectOptions),
    DashboardBundle(InspectOptions),
    DashboardProgress(InspectOptions),
    DashboardHelp,
    Inventory(InspectOptions),
    Coverage(CoverageOptions),
    Html(UtraceHtmlOptions),
    Timeline(TimelineCommand),
}

#[derive(Debug, Eq, PartialEq)]
enum TimelineCommand {
    Index(TimelineIndexOptions),
    Query(TimelineQueryOptions),
    Help,
}

#[derive(Debug, Eq, PartialEq)]
struct TimelineIndexOptions {
    input: Input,
    output: PathBuf,
    format: OutputFormat,
    max_intervals: usize,
}

#[derive(Debug, Eq, PartialEq)]
struct TimelineQueryOptions {
    index: PathBuf,
    format: OutputFormat,
    start_cycle: Option<u64>,
    end_cycle: Option<u64>,
    thread_id: Option<u16>,
    search: Option<String>,
    limit: usize,
}

#[derive(Debug, Eq, PartialEq)]
struct CoverageOptions {
    input: Input,
    format: OutputFormat,
    universe: Option<PathBuf>,
}

#[derive(Debug, Eq, PartialEq)]
struct UtraceHtmlOptions {
    input: Input,
    output: Option<PathBuf>,
}

impl Command {
    fn parse(arguments: Vec<OsString>) -> Result<Self, String> {
        let mut arguments = arguments.into_iter();
        let Some(command) = arguments.next() else {
            return Err("missing command".to_owned());
        };
        match command.to_str() {
            Some("inspect") => Self::parse_inspect(arguments.collect()),
            Some("authoring") => match Self::parse_inspect(arguments.collect())? {
                Self::Inspect(options) => Ok(Self::Authoring(options)),
                _ => unreachable!("parse_inspect only returns Inspect"),
            },
            Some("utrace") => Self::parse_utrace(arguments.collect()),
            Some("-h" | "--help" | "help") => {
                reject_trailing_arguments(arguments)?;
                Ok(Self::Help)
            }
            Some("-V" | "--version" | "version") => {
                reject_trailing_arguments(arguments)?;
                Ok(Self::Version)
            }
            Some(command) => Err(format!("unknown command {command:?}")),
            None => Err("command is not valid UTF-8".to_owned()),
        }
    }

    fn parse_utrace(arguments: Vec<OsString>) -> Result<Self, String> {
        let mut arguments = arguments.into_iter();
        let Some(command) = arguments.next() else {
            return Err("utrace requires a command".to_owned());
        };
        match command.to_str() {
            Some("inspect") => match Self::parse_inspect(arguments.collect())? {
                Self::Inspect(options) => Ok(Self::Utrace(UtraceCommand::Inspect(options))),
                _ => unreachable!("parse_inspect only returns Inspect"),
            },
            Some("dashboard") => {
                let dashboard_arguments = arguments.collect::<Vec<_>>();
                if dashboard_arguments.len() == 1
                    && matches!(dashboard_arguments[0].to_str(), Some("-h" | "--help"))
                {
                    Ok(Self::Utrace(UtraceCommand::DashboardHelp))
                } else {
                    Ok(Self::Utrace(UtraceCommand::Dashboard(
                        Self::parse_utrace_dashboard(dashboard_arguments)?,
                    )))
                }
            }
            Some("dashboard-bundle") => Ok(Self::Utrace(UtraceCommand::DashboardBundle(
                Self::parse_utrace_dashboard(arguments.collect())?,
            ))),
            Some("dashboard-progress") => Ok(Self::Utrace(UtraceCommand::DashboardProgress(
                Self::parse_utrace_dashboard(arguments.collect())?,
            ))),
            Some("inventory") => match Self::parse_inspect(arguments.collect())? {
                Self::Inspect(options) => Ok(Self::Utrace(UtraceCommand::Inventory(options))),
                _ => unreachable!("parse_inspect only returns Inspect"),
            },
            Some("coverage") => Ok(Self::Utrace(UtraceCommand::Coverage(Self::parse_coverage(
                arguments.collect(),
            )?))),
            Some("html") => Ok(Self::Utrace(UtraceCommand::Html(Self::parse_utrace_html(
                arguments.collect(),
            )?))),
            Some("timeline") => Ok(Self::Utrace(UtraceCommand::Timeline(
                Self::parse_utrace_timeline(arguments.collect())?,
            ))),
            Some(command) => Err(format!("unknown utrace command {command:?}")),
            None => Err("utrace command is not valid UTF-8".to_owned()),
        }
    }

    fn parse_inspect(arguments: Vec<OsString>) -> Result<Self, String> {
        let mut format = OutputFormat::Text;
        let mut input = None;
        let mut index = 0;

        while index < arguments.len() {
            let argument = &arguments[index];
            match argument.to_str() {
                Some("--format") => {
                    index += 1;
                    let value = arguments
                        .get(index)
                        .ok_or_else(|| "--format requires text or json".to_owned())?;
                    format = parse_format(value)?;
                }
                Some(value) if value.starts_with("--format=") => {
                    format = parse_format(OsString::from(&value["--format=".len()..]).as_os_str())?;
                }
                Some("-h" | "--help") => {
                    return Err("use `uasset help` for command usage".to_owned());
                }
                Some(value) if value.starts_with('-') && value != "-" => {
                    return Err(format!("unknown inspect option {value:?}"));
                }
                _ if input.is_some() => {
                    return Err("inspect accepts exactly one input".to_owned());
                }
                Some("-") => input = Some(Input::Stdin),
                _ => input = Some(Input::File(PathBuf::from(argument))),
            }
            index += 1;
        }

        Ok(Self::Inspect(InspectOptions {
            input: input.ok_or_else(|| "inspect requires a file path or `-`".to_owned())?,
            format,
            timeline_frame: None,
            timeline_limit: 500,
            max_frames: 120,
            gpu_timeline_frame: None,
            gpu_timeline_limit: 500,
            timeline_index_output: None,
            timeline_index_max_intervals: utrace_parser::utrace::DEFAULT_MAX_INDEXED_INTERVALS,
            symbol_paths: Vec::new(),
        }))
    }

    fn parse_utrace_dashboard(arguments: Vec<OsString>) -> Result<InspectOptions, String> {
        let mut format = OutputFormat::Text;
        let mut input = None;
        let mut timeline_frame = None;
        let mut timeline_limit = 500;
        let mut max_frames = 120;
        let mut gpu_timeline_frame = None;
        let mut gpu_timeline_limit = 500;
        let mut timeline_index_output = None;
        let mut timeline_index_max_intervals = utrace_parser::utrace::DEFAULT_MAX_INDEXED_INTERVALS;
        let mut symbol_paths = Vec::new();
        let mut index = 0;

        while index < arguments.len() {
            let argument = &arguments[index];
            match argument.to_str() {
                Some("--format") => {
                    index += 1;
                    let value = arguments
                        .get(index)
                        .ok_or_else(|| "--format requires text or json".to_owned())?;
                    format = parse_format(value)?;
                }
                Some(value) if value.starts_with("--format=") => {
                    format = parse_format(OsString::from(&value["--format=".len()..]).as_os_str())?;
                }
                Some("--frame") => {
                    index += 1;
                    let value = arguments
                        .get(index)
                        .ok_or_else(|| "--frame requires a frame number".to_owned())?;
                    timeline_frame = Some(parse_u32_arg(value, "--frame")?);
                }
                Some(value) if value.starts_with("--frame=") => {
                    timeline_frame = Some(parse_u32_arg(
                        OsString::from(&value["--frame=".len()..]).as_os_str(),
                        "--frame",
                    )?);
                }
                Some("--timeline-limit") => {
                    index += 1;
                    let value = arguments
                        .get(index)
                        .ok_or_else(|| "--timeline-limit requires a count".to_owned())?;
                    timeline_limit = parse_usize_arg(value, "--timeline-limit")?;
                }
                Some(value) if value.starts_with("--timeline-limit=") => {
                    timeline_limit = parse_usize_arg(
                        OsString::from(&value["--timeline-limit=".len()..]).as_os_str(),
                        "--timeline-limit",
                    )?;
                }
                Some("--max-frames") => {
                    index += 1;
                    let value = arguments
                        .get(index)
                        .ok_or_else(|| "--max-frames requires a count".to_owned())?;
                    max_frames = parse_usize_arg(value, "--max-frames")?;
                }
                Some(value) if value.starts_with("--max-frames=") => {
                    max_frames = parse_usize_arg(
                        OsString::from(&value["--max-frames=".len()..]).as_os_str(),
                        "--max-frames",
                    )?;
                }
                Some("--gpu-frame") => {
                    index += 1;
                    let value = arguments
                        .get(index)
                        .ok_or_else(|| "--gpu-frame requires a frame number".to_owned())?;
                    gpu_timeline_frame = Some(parse_u32_arg(value, "--gpu-frame")?);
                }
                Some(value) if value.starts_with("--gpu-frame=") => {
                    gpu_timeline_frame = Some(parse_u32_arg(
                        OsString::from(&value["--gpu-frame=".len()..]).as_os_str(),
                        "--gpu-frame",
                    )?);
                }
                Some("--gpu-timeline-limit") => {
                    index += 1;
                    let value = arguments
                        .get(index)
                        .ok_or_else(|| "--gpu-timeline-limit requires a count".to_owned())?;
                    gpu_timeline_limit = parse_usize_arg(value, "--gpu-timeline-limit")?;
                }
                Some(value) if value.starts_with("--gpu-timeline-limit=") => {
                    gpu_timeline_limit = parse_usize_arg(
                        OsString::from(&value["--gpu-timeline-limit=".len()..]).as_os_str(),
                        "--gpu-timeline-limit",
                    )?;
                }
                Some("--timeline-index-output") => {
                    index += 1;
                    let value = arguments
                        .get(index)
                        .ok_or_else(|| "--timeline-index-output requires a file path".to_owned())?;
                    timeline_index_output = Some(PathBuf::from(value));
                }
                Some(value) if value.starts_with("--timeline-index-output=") => {
                    timeline_index_output =
                        Some(PathBuf::from(&value["--timeline-index-output=".len()..]));
                }
                Some("--timeline-index-max-intervals") => {
                    index += 1;
                    let value = arguments.get(index).ok_or_else(|| {
                        "--timeline-index-max-intervals requires a count".to_owned()
                    })?;
                    timeline_index_max_intervals =
                        parse_usize_arg(value, "--timeline-index-max-intervals")?;
                }
                Some(value) if value.starts_with("--timeline-index-max-intervals=") => {
                    timeline_index_max_intervals = parse_usize_arg(
                        OsString::from(&value["--timeline-index-max-intervals=".len()..])
                            .as_os_str(),
                        "--timeline-index-max-intervals",
                    )?;
                }
                Some("--symbol-path") => {
                    index += 1;
                    let value = arguments
                        .get(index)
                        .ok_or_else(|| "--symbol-path requires a directory".to_owned())?;
                    symbol_paths.push(PathBuf::from(value));
                }
                Some(value) if value.starts_with("--symbol-path=") => {
                    symbol_paths.push(PathBuf::from(&value["--symbol-path=".len()..]));
                }
                Some("-h" | "--help") => {
                    return Err("use `uasset help` for command usage".to_owned());
                }
                Some(value) if value.starts_with('-') && value != "-" => {
                    return Err(format!("unknown dashboard option {value:?}"));
                }
                _ if input.is_some() => {
                    return Err("dashboard accepts exactly one input".to_owned());
                }
                Some("-") => input = Some(Input::Stdin),
                _ => input = Some(Input::File(PathBuf::from(argument))),
            }
            index += 1;
        }

        if timeline_index_output.is_some() && timeline_index_max_intervals == 0 {
            return Err("--timeline-index-max-intervals must be at least one".to_owned());
        }

        Ok(InspectOptions {
            input: input.ok_or_else(|| "dashboard requires a file path or `-`".to_owned())?,
            format,
            timeline_frame,
            timeline_limit,
            max_frames,
            gpu_timeline_frame,
            gpu_timeline_limit,
            timeline_index_output,
            timeline_index_max_intervals,
            symbol_paths,
        })
    }

    fn parse_coverage(arguments: Vec<OsString>) -> Result<CoverageOptions, String> {
        let mut format = OutputFormat::Text;
        let mut input = None;
        let mut universe = None;
        let mut index = 0;

        while index < arguments.len() {
            let argument = &arguments[index];
            match argument.to_str() {
                Some("--format") => {
                    index += 1;
                    let value = arguments
                        .get(index)
                        .ok_or_else(|| "--format requires text or json".to_owned())?;
                    format = parse_format(value)?;
                }
                Some(value) if value.starts_with("--format=") => {
                    format = parse_format(OsString::from(&value["--format=".len()..]).as_os_str())?;
                }
                Some("--universe") => {
                    index += 1;
                    let value = arguments
                        .get(index)
                        .ok_or_else(|| "--universe requires a file path".to_owned())?;
                    universe = Some(PathBuf::from(value));
                }
                Some(value) if value.starts_with("--universe=") => {
                    universe = Some(PathBuf::from(&value["--universe=".len()..]));
                }
                Some("-h" | "--help") => {
                    return Err("use `uasset help` for command usage".to_owned());
                }
                Some(value) if value.starts_with('-') && value != "-" => {
                    return Err(format!("unknown coverage option {value:?}"));
                }
                _ if input.is_some() => {
                    return Err("coverage accepts exactly one input".to_owned());
                }
                Some("-") => input = Some(Input::Stdin),
                _ => input = Some(Input::File(PathBuf::from(argument))),
            }
            index += 1;
        }

        Ok(CoverageOptions {
            input: input.ok_or_else(|| "coverage requires a file path or `-`".to_owned())?,
            format,
            universe,
        })
    }

    fn parse_utrace_html(arguments: Vec<OsString>) -> Result<UtraceHtmlOptions, String> {
        let mut input = None;
        let mut output = None;
        let mut index = 0;

        while index < arguments.len() {
            let argument = &arguments[index];
            match argument.to_str() {
                Some("--output") | Some("-o") => {
                    index += 1;
                    let value = arguments
                        .get(index)
                        .ok_or_else(|| "--output requires a file path".to_owned())?;
                    output = Some(PathBuf::from(value));
                }
                Some(value) if value.starts_with("--output=") => {
                    output = Some(PathBuf::from(&value["--output=".len()..]));
                }
                Some("-h" | "--help") => {
                    return Err("use `uasset help` for command usage".to_owned());
                }
                Some(value) if value.starts_with('-') && value != "-" => {
                    return Err(format!("unknown html option {value:?}"));
                }
                _ if input.is_some() => {
                    return Err("html accepts exactly one input".to_owned());
                }
                Some("-") => input = Some(Input::Stdin),
                _ => input = Some(Input::File(PathBuf::from(argument))),
            }
            index += 1;
        }

        Ok(UtraceHtmlOptions {
            input: input.ok_or_else(|| "html requires a file path or `-`".to_owned())?,
            output,
        })
    }

    fn parse_utrace_timeline(arguments: Vec<OsString>) -> Result<TimelineCommand, String> {
        let mut arguments = arguments.into_iter();
        let Some(command) = arguments.next() else {
            return Err("utrace timeline requires `index` or `query`".to_owned());
        };
        let arguments = arguments.collect::<Vec<_>>();
        match command.to_str() {
            Some("index") => {
                Self::parse_utrace_timeline_index(arguments).map(TimelineCommand::Index)
            }
            Some("query") => {
                Self::parse_utrace_timeline_query(arguments).map(TimelineCommand::Query)
            }
            Some("-h" | "--help" | "help") if arguments.is_empty() => Ok(TimelineCommand::Help),
            Some("-h" | "--help" | "help") => {
                Err("utrace timeline help does not accept trailing arguments".to_owned())
            }
            Some(command) => Err(format!("unknown utrace timeline command {command:?}")),
            None => Err("utrace timeline command is not valid UTF-8".to_owned()),
        }
    }

    fn parse_utrace_timeline_index(
        arguments: Vec<OsString>,
    ) -> Result<TimelineIndexOptions, String> {
        let mut input = None;
        let mut output = None;
        let mut format = OutputFormat::Text;
        let mut max_intervals = utrace_parser::utrace::DEFAULT_MAX_INDEXED_INTERVALS;
        let mut index = 0;
        while index < arguments.len() {
            let argument = &arguments[index];
            match argument.to_str() {
                Some("--output") | Some("-o") => {
                    index += 1;
                    let value = arguments
                        .get(index)
                        .ok_or_else(|| "--output requires a file path".to_owned())?;
                    output = Some(PathBuf::from(value));
                }
                Some(value) if value.starts_with("--output=") => {
                    output = Some(PathBuf::from(&value["--output=".len()..]));
                }
                Some("--max-intervals") => {
                    index += 1;
                    let value = arguments
                        .get(index)
                        .ok_or_else(|| "--max-intervals requires a count".to_owned())?;
                    max_intervals = parse_usize_arg(value, "--max-intervals")?;
                }
                Some(value) if value.starts_with("--max-intervals=") => {
                    max_intervals = parse_usize_arg(
                        OsString::from(&value["--max-intervals=".len()..]).as_os_str(),
                        "--max-intervals",
                    )?;
                }
                Some("--format") => {
                    index += 1;
                    let value = arguments
                        .get(index)
                        .ok_or_else(|| "--format requires text or json".to_owned())?;
                    format = parse_format(value)?;
                }
                Some(value) if value.starts_with("--format=") => {
                    format = parse_format(OsString::from(&value["--format=".len()..]).as_os_str())?;
                }
                Some(value) if value.starts_with('-') && value != "-" => {
                    return Err(format!("unknown timeline index option {value:?}"));
                }
                _ if input.is_some() => {
                    return Err("timeline index accepts exactly one trace input".to_owned());
                }
                Some("-") => input = Some(Input::Stdin),
                _ => input = Some(Input::File(PathBuf::from(argument))),
            }
            index += 1;
        }
        if max_intervals == 0 {
            return Err("--max-intervals must be at least one".to_owned());
        }
        Ok(TimelineIndexOptions {
            input: input.ok_or_else(|| "timeline index requires a trace input".to_owned())?,
            output: output.ok_or_else(|| "timeline index requires --output <path>".to_owned())?,
            format,
            max_intervals,
        })
    }

    fn parse_utrace_timeline_query(
        arguments: Vec<OsString>,
    ) -> Result<TimelineQueryOptions, String> {
        let mut index_path = None;
        let mut format = OutputFormat::Text;
        let mut start_cycle = None;
        let mut end_cycle = None;
        let mut thread_id = None;
        let mut search = None;
        let mut limit = 500;
        let mut index = 0;
        while index < arguments.len() {
            let argument = &arguments[index];
            match argument.to_str() {
                Some("--start-cycle") => {
                    index += 1;
                    let value = arguments
                        .get(index)
                        .ok_or_else(|| "--start-cycle requires a cycle".to_owned())?;
                    start_cycle = Some(parse_u64_arg(value, "--start-cycle")?);
                }
                Some(value) if value.starts_with("--start-cycle=") => {
                    start_cycle = Some(parse_u64_arg(
                        OsString::from(&value["--start-cycle=".len()..]).as_os_str(),
                        "--start-cycle",
                    )?);
                }
                Some("--end-cycle") => {
                    index += 1;
                    let value = arguments
                        .get(index)
                        .ok_or_else(|| "--end-cycle requires a cycle".to_owned())?;
                    end_cycle = Some(parse_u64_arg(value, "--end-cycle")?);
                }
                Some(value) if value.starts_with("--end-cycle=") => {
                    end_cycle = Some(parse_u64_arg(
                        OsString::from(&value["--end-cycle=".len()..]).as_os_str(),
                        "--end-cycle",
                    )?);
                }
                Some("--thread") => {
                    index += 1;
                    let value = arguments
                        .get(index)
                        .ok_or_else(|| "--thread requires a thread id".to_owned())?;
                    thread_id = Some(parse_u16_arg(value, "--thread")?);
                }
                Some(value) if value.starts_with("--thread=") => {
                    thread_id = Some(parse_u16_arg(
                        OsString::from(&value["--thread=".len()..]).as_os_str(),
                        "--thread",
                    )?);
                }
                Some("--search") => {
                    index += 1;
                    let value = arguments
                        .get(index)
                        .ok_or_else(|| "--search requires text".to_owned())?;
                    search = Some(
                        value
                            .to_str()
                            .ok_or_else(|| "--search must be valid UTF-8".to_owned())?
                            .to_owned(),
                    );
                }
                Some(value) if value.starts_with("--search=") => {
                    search = Some(value["--search=".len()..].to_owned());
                }
                Some("--limit") => {
                    index += 1;
                    let value = arguments
                        .get(index)
                        .ok_or_else(|| "--limit requires a count".to_owned())?;
                    limit = parse_usize_arg(value, "--limit")?;
                }
                Some(value) if value.starts_with("--limit=") => {
                    limit = parse_usize_arg(
                        OsString::from(&value["--limit=".len()..]).as_os_str(),
                        "--limit",
                    )?;
                }
                Some("--format") => {
                    index += 1;
                    let value = arguments
                        .get(index)
                        .ok_or_else(|| "--format requires text or json".to_owned())?;
                    format = parse_format(value)?;
                }
                Some(value) if value.starts_with("--format=") => {
                    format = parse_format(OsString::from(&value["--format=".len()..]).as_os_str())?;
                }
                Some(value) if value.starts_with('-') => {
                    return Err(format!("unknown timeline query option {value:?}"));
                }
                _ if index_path.is_some() => {
                    return Err("timeline query accepts exactly one index path".to_owned());
                }
                _ => index_path = Some(PathBuf::from(argument)),
            }
            index += 1;
        }
        if limit == 0 {
            return Err("--limit must be at least one".to_owned());
        }
        Ok(TimelineQueryOptions {
            index: index_path.ok_or_else(|| "timeline query requires an index path".to_owned())?,
            format,
            start_cycle,
            end_cycle,
            thread_id,
            search,
            limit,
        })
    }
}

fn reject_trailing_arguments(mut arguments: impl Iterator<Item = OsString>) -> Result<(), String> {
    if arguments.next().is_some() {
        Err("unexpected trailing arguments".to_owned())
    } else {
        Ok(())
    }
}

fn parse_format(value: &std::ffi::OsStr) -> Result<OutputFormat, String> {
    match value.to_str() {
        Some("text") => Ok(OutputFormat::Text),
        Some("json") => Ok(OutputFormat::Json),
        Some(value) => Err(format!("unsupported output format {value:?}")),
        None => Err("output format is not valid UTF-8".to_owned()),
    }
}

fn parse_u32_arg(value: &std::ffi::OsStr, flag: &str) -> Result<u32, String> {
    let Some(text) = value.to_str() else {
        return Err(format!("{flag} value is not valid UTF-8"));
    };
    text.parse::<u32>()
        .map_err(|_| format!("{flag} requires an unsigned integer, got {text:?}"))
}

fn parse_u16_arg(value: &std::ffi::OsStr, flag: &str) -> Result<u16, String> {
    let Some(text) = value.to_str() else {
        return Err(format!("{flag} value is not valid UTF-8"));
    };
    text.parse::<u16>()
        .map_err(|_| format!("{flag} requires an unsigned 16-bit integer, got {text:?}"))
}

fn parse_u64_arg(value: &std::ffi::OsStr, flag: &str) -> Result<u64, String> {
    let Some(text) = value.to_str() else {
        return Err(format!("{flag} value is not valid UTF-8"));
    };
    text.parse::<u64>()
        .map_err(|_| format!("{flag} requires an unsigned integer, got {text:?}"))
}

fn parse_usize_arg(value: &std::ffi::OsStr, flag: &str) -> Result<usize, String> {
    let Some(text) = value.to_str() else {
        return Err(format!("{flag} value is not valid UTF-8"));
    };
    text.parse::<usize>()
        .map_err(|_| format!("{flag} requires a non-negative integer, got {text:?}"))
}

fn inspect(options: &InspectOptions) -> u8 {
    let input_name = options.input.display_name();
    let bytes = match read_input(&options.input) {
        Ok(bytes) => bytes,
        Err(error) => {
            write_error(
                options.format,
                ErrorOutput::io(input_name, error.to_string()),
            );
            return EXIT_IO;
        }
    };

    let package = match Package::parse(&bytes) {
        Ok(package) => package,
        Err(error) => {
            let exit_code = exit_code_for_package_error(&error);
            write_error(options.format, ErrorOutput::package(input_name, &error));
            return exit_code;
        }
    };

    let output = InspectOutput::from_package(input_name, &bytes, &package);
    let partial = !output.decode_errors.is_empty();
    let rendered = match render_output(options.format, &output) {
        Ok(rendered) => rendered,
        Err(error) => {
            eprintln!("uasset: failed to serialize output: {error}");
            return EXIT_INTERNAL;
        }
    };
    let exit = write_stdout(&rendered);
    if exit == EXIT_SUCCESS && partial {
        EXIT_PARTIAL
    } else {
        exit
    }
}

fn authoring(options: &InspectOptions) -> u8 {
    if options.format != OutputFormat::Json {
        eprintln!("uasset: authoring requires --format json");
        return EXIT_USAGE;
    }

    let input_name = options.input.display_name();
    let bytes = match read_input(&options.input) {
        Ok(bytes) => bytes,
        Err(error) => {
            write_error(
                options.format,
                ErrorOutput::io(input_name, error.to_string()),
            );
            return EXIT_IO;
        }
    };
    let package = match Package::parse(&bytes) {
        Ok(package) => package,
        Err(error) => {
            let exit_code = exit_code_for_package_error(&error);
            write_error(options.format, ErrorOutput::package(input_name, &error));
            return exit_code;
        }
    };
    let output = InspectOutput::from_package(input_name, &bytes, &package);
    let mut tables = output
        .assets
        .iter()
        .filter(|asset| matches!(asset.kind, "DataTable" | "CompositeDataTable"));
    let Some(table) = tables.next() else {
        eprintln!("uasset: package contains no supported DataTable export");
        return EXIT_UNSUPPORTED;
    };
    if tables.next().is_some() {
        eprintln!("uasset: package contains more than one DataTable export");
        return EXIT_UNSUPPORTED;
    }

    let authoring = AuthoringSnapshotOutput::from_inspect(&output, table);
    let partial = authoring.completeness == "partial";
    let mut rendered = match serde_json::to_vec(&authoring) {
        Ok(rendered) => rendered,
        Err(error) => {
            eprintln!("uasset: failed to serialize authoring output: {error}");
            return EXIT_INTERNAL;
        }
    };
    rendered.push(b'\n');
    let exit = write_stdout(&rendered);
    if exit == EXIT_SUCCESS && partial {
        EXIT_PARTIAL
    } else {
        exit
    }
}

#[cfg(feature = "utrace")]
fn run_utrace(command: UtraceCommand) -> u8 {
    match command {
        UtraceCommand::Inspect(options) => inspect_utrace(&options),
        UtraceCommand::Dashboard(options) => dashboard_utrace(&options),
        UtraceCommand::DashboardBundle(options) => dashboard_bundle_utrace(&options),
        UtraceCommand::DashboardProgress(options) => dashboard_progress_utrace(&options),
        UtraceCommand::DashboardHelp => write_stdout(UTRACE_DASHBOARD_HELP.as_bytes()),
        UtraceCommand::Inventory(options) => inventory_utrace(&options),
        UtraceCommand::Coverage(options) => coverage_utrace(&options),
        UtraceCommand::Html(options) => html_utrace(&options),
        UtraceCommand::Timeline(TimelineCommand::Index(options)) => timeline_index_utrace(&options),
        UtraceCommand::Timeline(TimelineCommand::Query(options)) => timeline_query_utrace(&options),
        UtraceCommand::Timeline(TimelineCommand::Help) => {
            write_stdout(UTRACE_TIMELINE_HELP.as_bytes())
        }
    }
}

#[cfg(not(feature = "utrace"))]
fn run_utrace(_command: UtraceCommand) -> u8 {
    eprintln!("uasset: utrace support was not enabled at build time");
    EXIT_UNSUPPORTED
}

#[cfg(feature = "utrace")]
fn inspect_utrace(options: &InspectOptions) -> u8 {
    let input_name = options.input.display_name();
    let bytes = match read_input(&options.input) {
        Ok(bytes) => bytes,
        Err(error) => {
            write_utrace_error(
                options.format,
                UtraceErrorOutput::io(input_name, error.to_string()),
            );
            return EXIT_IO;
        }
    };

    let trace = match utrace_parser::utrace::inspect(&bytes) {
        Ok(trace) => trace,
        Err(error) => {
            let exit_code = exit_code_for_trace_error(&error);
            write_utrace_error(options.format, UtraceErrorOutput::trace(input_name, &error));
            return exit_code;
        }
    };
    let output = UtraceInspectOutput {
        schema_version: UTRACE_SCHEMA_VERSION,
        status: "ok",
        path: input_name,
        trace,
    };
    let rendered = match render_utrace_output(options.format, &output) {
        Ok(rendered) => rendered,
        Err(error) => {
            eprintln!("uasset: failed to serialize utrace output: {error}");
            return EXIT_INTERNAL;
        }
    };
    write_stdout(&rendered)
}

#[cfg(feature = "utrace")]
fn dashboard_utrace(options: &InspectOptions) -> u8 {
    if options.timeline_index_output.is_some() {
        eprintln!("uasset: --timeline-index-output is only supported by dashboard-progress");
        return EXIT_USAGE;
    }
    let input_name = options.input.display_name();
    let bytes = match read_input(&options.input) {
        Ok(bytes) => bytes,
        Err(error) => {
            write_utrace_error(
                options.format,
                UtraceErrorOutput::io(input_name, error.to_string()),
            );
            return EXIT_IO;
        }
    };

    #[allow(unused_mut)] // mutated when `utrace-symbols` is enabled
    let mut dashboard = match utrace_parser::utrace::dashboard_with_options(
        &bytes,
        utrace_parser::utrace::DashboardOptions {
            timeline_frame: options.timeline_frame,
            timeline_limit: Some(options.timeline_limit),
            max_frames: Some(options.max_frames),
            gpu_timeline_frame: options.gpu_timeline_frame,
            gpu_timeline_limit: Some(options.gpu_timeline_limit),
        },
    ) {
        Ok(dashboard) => dashboard,
        Err(error) => {
            let exit_code = exit_code_for_trace_error(&error);
            write_utrace_error(options.format, UtraceErrorOutput::trace(input_name, &error));
            return exit_code;
        }
    };
    if !options.symbol_paths.is_empty() {
        #[cfg(feature = "utrace-symbols")]
        {
            let mut resolver =
                utrace_parser::utrace_symbols::PdbSymbolResolver::new(options.symbol_paths.clone());
            utrace_parser::utrace_symbols::enrich_callstacks_with_symbols(
                &mut dashboard.callstacks,
                &mut resolver,
            );
        }
        #[cfg(not(feature = "utrace-symbols"))]
        {
            write_utrace_error(
                options.format,
                UtraceErrorOutput::io(
                    input_name,
                    "--symbol-path requires the utrace-symbols feature".to_owned(),
                ),
            );
            return EXIT_USAGE;
        }
    }
    let output = UtraceDashboardOutput {
        schema_version: UTRACE_SCHEMA_VERSION,
        status: "ok",
        path: input_name,
        dashboard,
    };
    let rendered = match render_utrace_dashboard_output(options.format, &output) {
        Ok(rendered) => rendered,
        Err(error) => {
            eprintln!("uasset: failed to serialize utrace dashboard output: {error}");
            return EXIT_INTERNAL;
        }
    };
    write_stdout(&rendered)
}

#[cfg(feature = "utrace")]
fn dashboard_bundle_utrace(options: &InspectOptions) -> u8 {
    if options.timeline_index_output.is_some() {
        eprintln!("uasset: --timeline-index-output is only supported by dashboard-progress");
        return EXIT_USAGE;
    }
    if options.format != OutputFormat::Json {
        eprintln!("uasset: dashboard-bundle requires --format json");
        return EXIT_USAGE;
    }
    let input_name = options.input.display_name();
    let bytes = match read_input(&options.input) {
        Ok(bytes) => bytes,
        Err(error) => {
            write_utrace_error(
                options.format,
                UtraceErrorOutput::io(input_name, error.to_string()),
            );
            return EXIT_IO;
        }
    };
    let (dashboard, inventory) = match utrace_parser::utrace::dashboard_and_inventory_with_options(
        &bytes,
        utrace_parser::utrace::DashboardOptions {
            timeline_frame: options.timeline_frame,
            timeline_limit: Some(options.timeline_limit),
            max_frames: Some(options.max_frames),
            gpu_timeline_frame: options.gpu_timeline_frame,
            gpu_timeline_limit: Some(options.gpu_timeline_limit),
        },
    ) {
        Ok(output) => output,
        Err(error) => {
            let exit_code = exit_code_for_trace_error(&error);
            write_utrace_error(options.format, UtraceErrorOutput::trace(input_name, &error));
            return exit_code;
        }
    };
    let output = UtraceDashboardBundleOutput {
        schema_version: UTRACE_SCHEMA_VERSION,
        status: "ok",
        path: input_name,
        dashboard,
        inventory,
    };
    match serde_json::to_vec_pretty(&output) {
        Ok(mut rendered) => {
            rendered.push(b'\n');
            write_stdout(&rendered)
        }
        Err(error) => {
            eprintln!("uasset: failed to serialize utrace output: {error}");
            EXIT_INTERNAL
        }
    }
}

#[cfg(feature = "utrace")]
fn dashboard_progress_utrace(options: &InspectOptions) -> u8 {
    if options.format != OutputFormat::Json || !options.symbol_paths.is_empty() {
        eprintln!("uasset: dashboard-progress requires JSON output and does not support symbols");
        return EXIT_USAGE;
    }
    let input_name = options.input.display_name();
    let total_bytes = match &options.input {
        Input::File(path) => fs::metadata(path).ok().map(|metadata| metadata.len()),
        Input::Stdin => None,
    };
    let input: Box<dyn Read> = match &options.input {
        Input::File(path) => match fs::File::open(path) {
            Ok(file) => Box::new(file),
            Err(error) => {
                write_progress_failure(0, error.to_string());
                return EXIT_IO;
            }
        },
        Input::Stdin => Box::new(io::stdin()),
    };
    let mut input = io::BufReader::with_capacity(1024 * 1024, input);
    let mut session = utrace_parser::utrace::ProgressiveDashboardSession::new(
        utrace_parser::utrace::DashboardOptions {
            timeline_frame: options.timeline_frame,
            timeline_limit: Some(options.timeline_limit),
            max_frames: Some(options.max_frames),
            gpu_timeline_frame: options.gpu_timeline_frame,
            gpu_timeline_limit: Some(options.gpu_timeline_limit),
        },
    );
    let mut buffer = vec![0_u8; 1024 * 1024];
    let mut sequence = 0_u64;
    let mut bootstrap_emitted = false;
    let mut last_transport_snapshot = Instant::now();
    let mut last_frame_snapshot = Instant::now();
    let mut last_packet_count = 0_u64;
    let mut last_frame_revision = 0_u64;
    loop {
        let count = match input.read(&mut buffer) {
            Ok(count) => count,
            Err(error) => {
                write_progress_failure(sequence, error.to_string());
                return EXIT_IO;
            }
        };
        if count == 0 {
            break;
        }
        if let Err(error) = session.push_chunk(&buffer[..count]) {
            write_progress_failure(sequence, error.to_string());
            return exit_code_for_trace_error(&error);
        }
        if !bootstrap_emitted {
            if let Some((progress, bootstrap)) = session.bootstrap(total_bytes) {
                if write_progress_json(&ProgressiveCliEvent::Bootstrap {
                    protocol_version: utrace_parser::utrace_progress::PROGRESS_PROTOCOL_VERSION,
                    sequence,
                    progress,
                    bootstrap,
                }) != EXIT_SUCCESS
                {
                    return EXIT_INTERNAL;
                }
                sequence += 1;
                bootstrap_emitted = true;
            }
        }
        let (progress, patch) = session.transport_patch(total_bytes);
        if progress.packets_observed.saturating_sub(last_packet_count) >= 256
            && last_transport_snapshot.elapsed() >= Duration::from_millis(100)
        {
            last_packet_count = progress.packets_observed;
            last_transport_snapshot = Instant::now();
            if write_progress_json(&ProgressiveCliEvent::Snapshot {
                protocol_version: utrace_parser::utrace_progress::PROGRESS_PROTOCOL_VERSION,
                sequence,
                progress,
                patch,
            }) != EXIT_SUCCESS
            {
                return EXIT_INTERNAL;
            }
            sequence += 1;
        }
        let (progress, patch) = session.frame_patch(total_bytes);
        let frame_revision = session.frame_revision();
        if frame_revision != last_frame_revision
            && (last_frame_revision == 0
                || last_frame_snapshot.elapsed() >= Duration::from_millis(100))
        {
            last_frame_revision = frame_revision;
            last_frame_snapshot = Instant::now();
            if write_progress_json(&ProgressiveCliEvent::Snapshot {
                protocol_version: utrace_parser::utrace_progress::PROGRESS_PROTOCOL_VERSION,
                sequence,
                progress,
                patch,
            }) != EXIT_SUCCESS
            {
                return EXIT_INTERNAL;
            }
            sequence += 1;
        }
    }
    let (progress, patch) = session.frame_patch(total_bytes);
    if session.frame_revision() != last_frame_revision {
        if write_progress_json(&ProgressiveCliEvent::Snapshot {
            protocol_version: utrace_parser::utrace_progress::PROGRESS_PROTOCOL_VERSION,
            sequence,
            progress,
            patch,
        }) != EXIT_SUCCESS
        {
            return EXIT_INTERNAL;
        }
        sequence += 1;
    }
    let (progress, patch) = session.analyzing_patch(total_bytes);
    if write_progress_json(&ProgressiveCliEvent::Snapshot {
        protocol_version: utrace_parser::utrace_progress::PROGRESS_PROTOCOL_VERSION,
        sequence,
        progress,
        patch,
    }) != EXIT_SUCCESS
    {
        return EXIT_INTERNAL;
    }
    sequence += 1;
    let complete_progress = session.complete_progress(total_bytes);
    let timeline_index_request =
        options
            .timeline_index_output
            .as_ref()
            .map(|output| TimelineIndexRequest {
                output: output.clone(),
                max_intervals: options.timeline_index_max_intervals,
            });
    let (dashboard, inventory, timeline_index) =
        match session.finish_with_inventory_and_timeline_index(timeline_index_request) {
            Ok(output) => output,
            Err(error) => {
                write_progress_failure(sequence, error.to_string());
                return exit_code_for_trace_error(&error);
            }
        };
    let (timeline_index, timeline_index_warning) = match timeline_index {
        Some(build) => match build.result {
            Ok(index) => (
                Some(ProgressiveTimelineIndexOutput {
                    index_path: build.output.to_string_lossy().into_owned(),
                    index,
                }),
                None,
            ),
            Err(error) => (None, Some(error.to_string())),
        },
        None => (None, None),
    };
    write_progress_json(&ProgressiveCliEvent::Complete {
        protocol_version: utrace_parser::utrace_progress::PROGRESS_PROTOCOL_VERSION,
        sequence,
        progress: complete_progress,
        dashboard: Box::new(UtraceDashboardOutput {
            schema_version: UTRACE_SCHEMA_VERSION,
            status: "ok",
            path: input_name.clone(),
            dashboard,
        }),
        inventory: Box::new(UtraceInventoryOutput {
            schema_version: UTRACE_SCHEMA_VERSION,
            status: "ok",
            path: input_name,
            inventory,
        }),
        timeline_index,
        timeline_index_warning,
    })
}

#[cfg(feature = "utrace")]
fn write_progress_failure(sequence: u64, error: String) {
    let _ = write_progress_json(&ProgressiveCliEvent::Failed {
        protocol_version: utrace_parser::utrace_progress::PROGRESS_PROTOCOL_VERSION,
        sequence,
        error,
    });
}

#[cfg(feature = "utrace")]
fn write_progress_json(event: &ProgressiveCliEvent) -> u8 {
    match serde_json::to_vec(event) {
        Ok(mut rendered) => {
            rendered.push(b'\n');
            write_stdout(&rendered)
        }
        Err(error) => {
            eprintln!("uasset: failed to serialize progressive event: {error}");
            EXIT_INTERNAL
        }
    }
}

#[cfg(feature = "utrace")]
fn timeline_index_utrace(options: &TimelineIndexOptions) -> u8 {
    let input_name = options.input.display_name();
    let (bytes, source_identity) = match read_timeline_input(&options.input) {
        Ok(input) => input,
        Err(error) => {
            write_utrace_error(
                options.format,
                UtraceErrorOutput::io(input_name, error.to_string()),
            );
            return EXIT_IO;
        }
    };
    let index = match utrace_parser::utrace::build_cpu_timeline_index_with_source_identity(
        &bytes,
        &options.output,
        options.max_intervals,
        source_identity,
    ) {
        Ok(index) => index,
        Err(error) => {
            let exit_code = exit_code_for_timeline_index_error(&error);
            write_utrace_error(
                options.format,
                UtraceErrorOutput::io(input_name, error.to_string()),
            );
            return exit_code;
        }
    };
    let output = UtraceTimelineIndexOutput {
        schema_version: UTRACE_SCHEMA_VERSION,
        status: "ok",
        path: input_name,
        index_path: options.output.to_string_lossy().into_owned(),
        index,
    };
    match render_utrace_timeline_index_output(options.format, &output) {
        Ok(rendered) => write_stdout(&rendered),
        Err(error) => {
            eprintln!("uasset: failed to serialize utrace timeline index output: {error}");
            EXIT_INTERNAL
        }
    }
}

#[cfg(feature = "utrace")]
fn read_timeline_input(
    input: &Input,
) -> io::Result<(Vec<u8>, utrace_parser::utrace::SourceIdentity)> {
    let mut reader: Box<dyn Read> = match input {
        Input::File(path) => Box::new(fs::File::open(path)?),
        Input::Stdin => Box::new(io::stdin()),
    };
    let mut bytes = Vec::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    let mut fingerprint = utrace_parser::utrace::SourceFingerprint::new();
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let chunk = &buffer[..count];
        fingerprint.update(chunk);
        bytes.extend_from_slice(chunk);
    }
    Ok((bytes, fingerprint.finish()))
}

#[cfg(feature = "utrace")]
fn timeline_query_utrace(options: &TimelineQueryOptions) -> u8 {
    let path = options.index.to_string_lossy().into_owned();
    let timeline = match utrace_parser::utrace::query_cpu_timeline_index(
        &options.index,
        &CpuTimelineQuery {
            start_cycle: options.start_cycle,
            end_cycle: options.end_cycle,
            thread_id: options.thread_id,
            search: options.search.clone(),
            limit: Some(options.limit),
        },
    ) {
        Ok(timeline) => timeline,
        Err(error) => {
            let exit_code = exit_code_for_timeline_index_error(&error);
            write_utrace_error(
                options.format,
                UtraceErrorOutput::io(path, error.to_string()),
            );
            return exit_code;
        }
    };
    let output = UtraceTimelineQueryOutput {
        schema_version: UTRACE_SCHEMA_VERSION,
        status: "ok",
        path,
        timeline,
    };
    match render_utrace_timeline_query_output(options.format, &output) {
        Ok(rendered) => write_stdout(&rendered),
        Err(error) => {
            eprintln!("uasset: failed to serialize utrace timeline query output: {error}");
            EXIT_INTERNAL
        }
    }
}

#[cfg(feature = "utrace")]
fn inventory_utrace(options: &InspectOptions) -> u8 {
    let input_name = options.input.display_name();
    let bytes = match read_input(&options.input) {
        Ok(bytes) => bytes,
        Err(error) => {
            write_utrace_error(
                options.format,
                UtraceErrorOutput::io(input_name, error.to_string()),
            );
            return EXIT_IO;
        }
    };

    let inventory = match utrace_parser::utrace::inventory(&bytes) {
        Ok(inventory) => inventory,
        Err(error) => {
            let exit_code = exit_code_for_trace_error(&error);
            write_utrace_error(options.format, UtraceErrorOutput::trace(input_name, &error));
            return exit_code;
        }
    };
    let output = UtraceInventoryOutput {
        schema_version: UTRACE_SCHEMA_VERSION,
        status: "ok",
        path: input_name,
        inventory,
    };
    let rendered = match render_utrace_inventory_output(options.format, &output) {
        Ok(rendered) => rendered,
        Err(error) => {
            eprintln!("uasset: failed to serialize utrace inventory output: {error}");
            return EXIT_INTERNAL;
        }
    };
    write_stdout(&rendered)
}

#[cfg(feature = "utrace")]
fn coverage_utrace(options: &CoverageOptions) -> u8 {
    let input_name = options.input.display_name();
    let bytes = match read_input(&options.input) {
        Ok(bytes) => bytes,
        Err(error) => {
            write_utrace_error(
                options.format,
                UtraceErrorOutput::io(input_name, error.to_string()),
            );
            return EXIT_IO;
        }
    };

    let universe = match &options.universe {
        Some(path) => match fs::read_to_string(path) {
            Ok(contents) => Some(
                contents
                    .lines()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(str::to_owned)
                    .collect::<std::collections::BTreeSet<String>>(),
            ),
            Err(error) => {
                write_utrace_error(
                    options.format,
                    UtraceErrorOutput::io(path.to_string_lossy().into_owned(), error.to_string()),
                );
                return EXIT_IO;
            }
        },
        None => None,
    };

    let coverage = match utrace_parser::utrace::coverage(&bytes, universe.as_ref()) {
        Ok(coverage) => coverage,
        Err(error) => {
            let exit_code = exit_code_for_trace_error(&error);
            write_utrace_error(options.format, UtraceErrorOutput::trace(input_name, &error));
            return exit_code;
        }
    };
    let output = UtraceCoverageOutput {
        schema_version: UTRACE_SCHEMA_VERSION,
        status: "ok",
        path: input_name,
        coverage,
    };
    let rendered = match render_utrace_coverage_output(options.format, &output) {
        Ok(rendered) => rendered,
        Err(error) => {
            eprintln!("uasset: failed to serialize utrace coverage output: {error}");
            return EXIT_INTERNAL;
        }
    };
    write_stdout(&rendered)
}

#[cfg(feature = "utrace")]
fn html_utrace(options: &UtraceHtmlOptions) -> u8 {
    let input_name = options.input.display_name();
    let bytes = match read_input(&options.input) {
        Ok(bytes) => bytes,
        Err(error) => {
            write_utrace_error(
                OutputFormat::Text,
                UtraceErrorOutput::io(input_name, error.to_string()),
            );
            return EXIT_IO;
        }
    };

    let dashboard = match utrace_parser::utrace::dashboard(&bytes) {
        Ok(dashboard) => dashboard,
        Err(error) => {
            let exit_code = exit_code_for_trace_error(&error);
            write_utrace_error(
                OutputFormat::Text,
                UtraceErrorOutput::trace(input_name, &error),
            );
            return exit_code;
        }
    };
    let output = UtraceDashboardOutput {
        schema_version: UTRACE_SCHEMA_VERSION,
        status: "ok",
        path: input_name,
        dashboard,
    };
    let rendered = render_utrace_dashboard_html_output(&output);

    match &options.output {
        Some(path) => match fs::write(path, rendered) {
            Ok(()) => EXIT_SUCCESS,
            Err(error) => {
                write_utrace_error(
                    OutputFormat::Text,
                    UtraceErrorOutput::io(path.to_string_lossy().into_owned(), error.to_string()),
                );
                EXIT_IO
            }
        },
        None => write_stdout(rendered.as_bytes()),
    }
}

fn read_input(input: &Input) -> io::Result<Vec<u8>> {
    match input {
        Input::File(path) => fs::read(path),
        Input::Stdin => {
            let mut bytes = Vec::new();
            io::stdin().lock().read_to_end(&mut bytes)?;
            Ok(bytes)
        }
    }
}

fn render_output(
    format: OutputFormat,
    output: &InspectOutput,
) -> Result<Vec<u8>, serde_json::Error> {
    match format {
        OutputFormat::Text => Ok(render_text_output(output).into_bytes()),
        OutputFormat::Json => {
            let mut rendered = serde_json::to_vec(output)?;
            rendered.push(b'\n');
            Ok(rendered)
        }
    }
}

#[cfg(feature = "utrace")]
fn render_utrace_output(
    format: OutputFormat,
    output: &UtraceInspectOutput,
) -> Result<Vec<u8>, serde_json::Error> {
    match format {
        OutputFormat::Text => Ok(render_utrace_text_output(output).into_bytes()),
        OutputFormat::Json => {
            let mut rendered = serde_json::to_vec(output)?;
            rendered.push(b'\n');
            Ok(rendered)
        }
    }
}

#[cfg(feature = "utrace")]
fn render_utrace_dashboard_output(
    format: OutputFormat,
    output: &UtraceDashboardOutput,
) -> Result<Vec<u8>, serde_json::Error> {
    match format {
        OutputFormat::Text => Ok(render_utrace_dashboard_text_output(output).into_bytes()),
        OutputFormat::Json => {
            let mut rendered = serde_json::to_vec(output)?;
            rendered.push(b'\n');
            Ok(rendered)
        }
    }
}

#[cfg(feature = "utrace")]
fn render_utrace_inventory_output(
    format: OutputFormat,
    output: &UtraceInventoryOutput,
) -> Result<Vec<u8>, serde_json::Error> {
    match format {
        OutputFormat::Text => Ok(render_utrace_inventory_text_output(output).into_bytes()),
        OutputFormat::Json => {
            let mut rendered = serde_json::to_vec(output)?;
            rendered.push(b'\n');
            Ok(rendered)
        }
    }
}

#[cfg(feature = "utrace")]
fn render_utrace_coverage_output(
    format: OutputFormat,
    output: &UtraceCoverageOutput,
) -> Result<Vec<u8>, serde_json::Error> {
    match format {
        OutputFormat::Text => Ok(render_utrace_coverage_text_output(output).into_bytes()),
        OutputFormat::Json => {
            let mut rendered = serde_json::to_vec(output)?;
            rendered.push(b'\n');
            Ok(rendered)
        }
    }
}

#[cfg(feature = "utrace")]
fn render_utrace_timeline_index_output(
    format: OutputFormat,
    output: &UtraceTimelineIndexOutput,
) -> Result<Vec<u8>, serde_json::Error> {
    match format {
        OutputFormat::Text => Ok(render_utrace_timeline_index_text_output(output).into_bytes()),
        OutputFormat::Json => {
            let mut rendered = serde_json::to_vec(output)?;
            rendered.push(b'\n');
            Ok(rendered)
        }
    }
}

#[cfg(feature = "utrace")]
fn render_utrace_timeline_query_output(
    format: OutputFormat,
    output: &UtraceTimelineQueryOutput,
) -> Result<Vec<u8>, serde_json::Error> {
    match format {
        OutputFormat::Text => Ok(render_utrace_timeline_query_text_output(output).into_bytes()),
        OutputFormat::Json => {
            let mut rendered = serde_json::to_vec(output)?;
            rendered.push(b'\n');
            Ok(rendered)
        }
    }
}

#[cfg(feature = "utrace")]
fn render_utrace_dashboard_html_output(output: &UtraceDashboardOutput) -> String {
    let dashboard = &output.dashboard;
    let mut rendered = String::new();
    rendered.push_str("<!doctype html>\n<html lang=\"en\">\n<head>\n");
    rendered.push_str("<meta charset=\"utf-8\">\n");
    rendered.push_str("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
    writeln!(
        rendered,
        "<title>UTrace report - {}</title>",
        html_escape(&output.path)
    )
    .unwrap();
    rendered.push_str(
        "<style>
:root { color-scheme: light dark; --border: #d0d7de; --muted: #57606a; --bg: #ffffff; --panel: #f6f8fa; --text: #24292f; }
@media (prefers-color-scheme: dark) { :root { --border: #30363d; --muted: #8b949e; --bg: #0d1117; --panel: #161b22; --text: #c9d1d9; } }
body { margin: 0; font: 14px/1.45 system-ui, -apple-system, BlinkMacSystemFont, \"Segoe UI\", sans-serif; color: var(--text); background: var(--bg); }
main { max-width: 1180px; margin: 0 auto; padding: 24px; }
h1 { margin: 0 0 4px; font-size: 28px; }
h2 { margin: 28px 0 10px; font-size: 18px; }
.muted { color: var(--muted); }
.grid { display: grid; gap: 12px; grid-template-columns: repeat(auto-fit, minmax(160px, 1fr)); }
.metric { border: 1px solid var(--border); border-radius: 6px; padding: 12px; background: var(--panel); }
.metric strong { display: block; font-size: 22px; }
table { width: 100%; border-collapse: collapse; margin-top: 8px; }
th, td { border: 1px solid var(--border); padding: 6px 8px; text-align: left; vertical-align: top; }
th { background: var(--panel); }
code, pre { font-family: ui-monospace, SFMono-Regular, Consolas, \"Liberation Mono\", monospace; }
pre { overflow: auto; border: 1px solid var(--border); border-radius: 6px; padding: 12px; background: var(--panel); }
</style>\n",
    );
    rendered.push_str("</head>\n<body>\n<main>\n");
    write!(
        rendered,
        "<h1>UTrace report</h1>\n<p class=\"muted\">{}</p>\n",
        html_escape(&output.path)
    )
    .unwrap();

    rendered.push_str("<section class=\"grid\">\n");
    html_metric(
        &mut rendered,
        "Threads",
        dashboard.thread_info.len(),
        "declared thread names",
    );
    html_metric(
        &mut rendered,
        "Frames",
        dashboard.frames.len(),
        "CPU frame markers",
    );
    html_metric(
        &mut rendered,
        "CPU scopes",
        dashboard.cpu.scopes.len(),
        "summarized scope specs",
    );
    html_metric(
        &mut rendered,
        "CPU intervals",
        dashboard.cpu.batches.intervals,
        "decoded timing intervals",
    );
    html_metric(
        &mut rendered,
        "Counters",
        dashboard.counters.counters.len(),
        "tracked counters",
    );
    html_metric(
        &mut rendered,
        "Unmodeled events",
        dashboard.unmodeled.event_types,
        "event types not decoded yet",
    );
    rendered.push_str("</section>\n");

    if let Some(prologue) = &dashboard.prologue {
        rendered.push_str("<h2>Trace Timing</h2>\n<table><tbody>\n");
        html_kv_row(&mut rendered, "Start cycle", prologue.start_cycle);
        html_kv_row(&mut rendered, "Cycle frequency", prologue.cycle_frequency);
        html_kv_row(&mut rendered, "Pointer size", prologue.pointer_size);
        if let Some(start_date_time) = prologue.start_date_time {
            html_kv_row(&mut rendered, "Start date time", start_date_time);
        }
        rendered.push_str("</tbody></table>\n");
    }

    if let Some(session) = &dashboard.session {
        rendered.push_str("<h2>Session</h2>\n<table><tbody>\n");
        html_kv_row(&mut rendered, "Platform", &session.platform);
        html_kv_row(&mut rendered, "App", &session.app_name);
        html_kv_row(&mut rendered, "Project", &session.project_name);
        html_kv_row(
            &mut rendered,
            "Configuration",
            format!("{:?}", session.configuration),
        );
        html_kv_row(
            &mut rendered,
            "Target",
            format!("{:?}", session.target_type),
        );
        html_kv_row(&mut rendered, "Build", &session.build_version);
        html_kv_row(&mut rendered, "Branch", &session.branch);
        html_kv_row(&mut rendered, "Changelist", session.changelist);
        rendered.push_str("</tbody></table>\n");
    }

    rendered.push_str("<h2>Threads</h2>\n<table><thead><tr><th>ID</th><th>System ID</th><th>Name</th><th>Sort Hint</th></tr></thead><tbody>\n");
    for thread in dashboard.thread_info.iter().take(50) {
        writeln!(
            rendered,
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            thread.thread_id,
            thread.system_id,
            html_escape(&thread.name),
            thread.sort_hint
        )
        .unwrap();
    }
    rendered.push_str("</tbody></table>\n");

    rendered.push_str("<h2>Frames</h2>\n<table><thead><tr><th>Frame</th><th>CPU Frame Seconds</th><th>CPU Frame Cycles</th><th>Top CPU Scopes</th><th>GPU Queues</th><th>GPU Work</th><th>GPU Work Cycles</th><th>Top GPU Breadcrumbs</th></tr></thead><tbody>\n");
    for frame in dashboard.frame_correlation.frames.iter().take(80) {
        let cpu_scopes = frame
            .top_cpu_scopes
            .iter()
            .take(4)
            .map(|scope| format!("{} ({})", scope.name, scope.total_cycles))
            .collect::<Vec<_>>()
            .join(", ");
        let gpu_breadcrumbs = frame
            .top_gpu_breadcrumbs
            .iter()
            .take(4)
            .map(|breadcrumb| format!("{} ({})", breadcrumb.name, breadcrumb.total_cycles))
            .collect::<Vec<_>>()
            .join(", ");
        writeln!(
            rendered,
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            frame.frame_number,
            html_optional(frame.cpu_metadata_seconds),
            frame.cpu_metadata_cycles,
            html_escape(&cpu_scopes),
            frame.gpu_queue_count,
            frame.gpu_work_count,
            frame.gpu_work_cycles,
            html_escape(&gpu_breadcrumbs)
        )
        .unwrap();
    }
    rendered.push_str("</tbody></table>\n");

    rendered.push_str("<h2>Top CPU Scopes</h2>\n<table><thead><tr><th>Spec</th><th>Name</th><th>Count</th><th>Total Cycles</th><th>Total Seconds</th></tr></thead><tbody>\n");
    for scope in dashboard.cpu.scopes.iter().take(40) {
        writeln!(
            rendered,
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            scope.spec_id,
            html_escape(&scope.name),
            scope.count,
            scope.total_cycles,
            html_optional(scope.total_seconds)
        )
        .unwrap();
    }
    rendered.push_str("</tbody></table>\n");

    rendered.push_str("<h2>Counters</h2>\n<table><thead><tr><th>ID</th><th>Name</th><th>Samples</th><th>Latest</th><th>Min</th><th>Max</th></tr></thead><tbody>\n");
    for counter in dashboard.counters.counters.iter().take(40) {
        writeln!(
            rendered,
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            counter.id,
            html_escape(&counter.name),
            counter.samples,
            html_optional(counter.latest),
            html_optional(counter.min),
            html_optional(counter.max)
        )
        .unwrap();
    }
    rendered.push_str("</tbody></table>\n");

    rendered.push_str("<h2>GPU Queues</h2>\n<table><thead><tr><th>ID</th><th>Name</th><th>Work</th><th>Work Cycles</th><th>Draws</th><th>Primitives</th></tr></thead><tbody>\n");
    for queue in dashboard.gpu.queues.iter().take(40) {
        writeln!(
            rendered,
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            queue.queue_id,
            html_escape(queue.name.as_deref().unwrap_or("")),
            queue.work_count,
            queue.work_total_cycles,
            queue.draw_count,
            queue.primitive_count
        )
        .unwrap();
    }
    rendered.push_str("</tbody></table>\n");

    rendered.push_str("<h2>Parser Progress</h2>\n<table><tbody>\n");
    html_kv_row(
        &mut rendered,
        "Unmodeled event types",
        dashboard.unmodeled.event_types,
    );
    html_kv_row(
        &mut rendered,
        "Unmodeled observed events",
        dashboard.unmodeled.observed_events,
    );
    html_kv_row(&mut rendered, "Log messages", dashboard.logging.messages);
    html_kv_row(
        &mut rendered,
        "Bookmark events",
        dashboard.annotations.bookmarks.events,
    );
    html_kv_row(
        &mut rendered,
        "Region completions",
        dashboard.annotations.regions.completed,
    );
    html_kv_row(
        &mut rendered,
        "Load packages",
        dashboard.loading.package_count,
    );
    html_kv_row(
        &mut rendered,
        "Load requests",
        dashboard.loading.requests.completed,
    );
    html_kv_row(
        &mut rendered,
        "IoStore requests",
        dashboard.io_store.requests_created,
    );
    html_kv_row(
        &mut rendered,
        "IoStore bytes",
        dashboard.io_store.bytes_completed,
    );
    html_kv_row(&mut rendered, "Trace channels", dashboard.channels.count);
    rendered.push_str("</tbody></table>\n");

    rendered.push_str("<h2>Top Unmodeled Events</h2>\n<table><thead><tr><th>Event</th><th>Observed</th></tr></thead><tbody>\n");
    for event in dashboard.unmodeled.events.iter().take(40) {
        writeln!(
            rendered,
            "<tr><td>{}.{}</td><td>{}</td></tr>",
            html_escape(&event.logger),
            html_escape(&event.event),
            event.observed_count
        )
        .unwrap();
    }
    rendered.push_str("</tbody></table>\n");

    rendered.push_str("</main>\n</body>\n</html>\n");
    rendered
}

#[cfg(feature = "utrace")]
fn html_metric(rendered: &mut String, label: &str, value: impl std::fmt::Display, note: &str) {
    writeln!(
        rendered,
        "<div class=\"metric\"><span>{}</span><strong>{}</strong><small class=\"muted\">{}</small></div>",
        html_escape(label),
        html_escape(&value.to_string()),
        html_escape(note)
    )
    .unwrap();
}

#[cfg(feature = "utrace")]
fn html_kv_row(rendered: &mut String, key: &str, value: impl std::fmt::Display) {
    writeln!(
        rendered,
        "<tr><th>{}</th><td>{}</td></tr>",
        html_escape(key),
        html_escape(&value.to_string())
    )
    .unwrap();
}

#[cfg(feature = "utrace")]
fn html_optional(value: Option<f64>) -> String {
    value
        .map(|value| html_escape(&format!("{value:.6}")))
        .unwrap_or_default()
}

#[cfg(feature = "utrace")]
fn html_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&#39;"),
            _ => escaped.push(character),
        }
    }
    escaped
}

#[cfg(feature = "utrace")]
fn render_utrace_coverage_text_output(output: &UtraceCoverageOutput) -> String {
    let mut rendered = String::new();
    let coverage = &output.coverage;
    let summary = &coverage.summary;
    writeln!(rendered, "path: {}", output.path).unwrap();
    writeln!(
        rendered,
        "declared_event_types={} decoded={} partial={} raw={} observed_events={} raw_observed_events={}",
        summary.declared_event_types,
        summary.decoded_event_types,
        summary.partial_event_types,
        summary.raw_event_types,
        summary.observed_events,
        summary.raw_observed_events
    )
    .unwrap();
    writeln!(rendered, "raw declared events (biggest gaps by volume):").unwrap();
    for entry in coverage
        .events
        .iter()
        .filter(|entry| entry.status == utrace_parser::utrace::DecodeStatus::Raw)
        .take(20)
    {
        writeln!(
            rendered,
            "  {}.{} uid={} observed={}",
            entry.logger, entry.event, entry.uid, entry.observed_count
        )
        .unwrap();
    }
    if let Some(universe) = &coverage.universe {
        writeln!(
            rendered,
            "universe: total={} declared_in_trace={} unseen={} not_in_universe={}",
            universe.total,
            universe.declared_in_trace,
            universe.unseen.len(),
            universe.not_in_universe.len()
        )
        .unwrap();
        writeln!(
            rendered,
            "unseen engine events (not declared in this trace):"
        )
        .unwrap();
        for event in universe.unseen.iter().take(40) {
            writeln!(rendered, "  {event}").unwrap();
        }
        if !universe.not_in_universe.is_empty() {
            writeln!(
                rendered,
                "declared but not in universe (game-specific or non-macro):"
            )
            .unwrap();
            for event in universe.not_in_universe.iter().take(20) {
                writeln!(rendered, "  {event}").unwrap();
            }
        }
    }
    rendered
}

#[cfg(feature = "utrace")]
fn render_utrace_inventory_text_output(output: &UtraceInventoryOutput) -> String {
    let mut rendered = String::new();
    let summary = &output.inventory.summary;
    writeln!(rendered, "path: {}", output.path).unwrap();
    writeln!(
        rendered,
        "declared_event_types={} observed_event_types={} observed_events={} decoded={} partial={} raw={} known_event_types={} known_events={}",
        summary.declared_event_types,
        summary.observed_event_types,
        summary.observed_events,
        summary.decoded_event_types,
        summary.partial_event_types,
        summary.raw_event_types,
        summary.known_event_types,
        summary.known_events
    )
    .unwrap();
    writeln!(rendered, "events:").unwrap();
    for event in output.inventory.events.iter().take(40) {
        writeln!(
            rendered,
            "  {}.{} uid={} observed={} status={:?} fields={} samples={}",
            event.logger,
            event.event,
            event.uid,
            event.observed_count,
            event.decode_status,
            event.fields.len(),
            event.samples.len()
        )
        .unwrap();
        for sample in event.samples.iter().take(1) {
            let field_names = sample.fields.keys().cloned().collect::<Vec<_>>().join(", ");
            writeln!(
                rendered,
                "    sample thread={} fields=[{}]",
                sample.thread_id, field_names
            )
            .unwrap();
        }
    }
    if !output.inventory.known_events.is_empty() {
        writeln!(rendered, "known_events:").unwrap();
        for event in &output.inventory.known_events {
            writeln!(
                rendered,
                "  {} uid={} observed={}",
                event.name, event.uid, event.observed_count
            )
            .unwrap();
        }
    }
    rendered
}

#[cfg(feature = "utrace")]
fn render_utrace_timeline_index_text_output(output: &UtraceTimelineIndexOutput) -> String {
    let index = &output.index;
    let mut rendered = String::new();
    writeln!(rendered, "path: {}", output.path).unwrap();
    writeln!(rendered, "index: {}", output.index_path).unwrap();
    writeln!(
        rendered,
        "cpu intervals: {} indexed / {} observed{}",
        index.indexed_interval_count,
        index.total_interval_count,
        if index.truncated { " (capped)" } else { "" },
    )
    .unwrap();
    if let (Some(begin), Some(end)) = (index.begin_cycle, index.end_cycle) {
        writeln!(rendered, "cycle range: {begin}..{end}").unwrap();
    }
    rendered
}

#[cfg(feature = "utrace")]
fn render_utrace_timeline_query_text_output(output: &UtraceTimelineQueryOutput) -> String {
    let timeline = &output.timeline;
    let mut rendered = String::new();
    writeln!(rendered, "index: {}", output.path).unwrap();
    writeln!(
        rendered,
        "cycle range: {}..{}",
        timeline.begin_cycle, timeline.end_cycle
    )
    .unwrap();
    writeln!(
        rendered,
        "cpu intervals: {} matched / {} returned{}",
        timeline.interval_count,
        timeline.intervals.len(),
        if timeline.truncated {
            " (truncated)"
        } else {
            ""
        },
    )
    .unwrap();
    for interval in &timeline.intervals {
        writeln!(
            rendered,
            "  [{}] {} {}..{} ({} cycles)",
            interval.thread_id,
            interval.rendered_name.as_deref().unwrap_or(&interval.name),
            interval.start_cycle,
            interval.end_cycle,
            interval.duration,
        )
        .unwrap();
    }
    rendered
}

#[cfg(feature = "utrace")]
fn render_utrace_dashboard_text_output(output: &UtraceDashboardOutput) -> String {
    let mut rendered = String::new();
    writeln!(rendered, "path: {}", output.path).unwrap();
    if let Some(prologue) = &output.dashboard.prologue {
        writeln!(
            rendered,
            "prologue: start_cycle={} cycle_frequency={}",
            prologue.start_cycle, prologue.cycle_frequency
        )
        .unwrap();
    }
    if let Some(session) = &output.dashboard.session {
        writeln!(
            rendered,
            "session: platform={:?} app={:?} project={:?} config={:?} target={:?} changelist={} branch={:?} build={:?}",
            session.platform,
            session.app_name,
            session.project_name,
            session.configuration,
            session.target_type,
            session.changelist,
            session.branch,
            session.build_version
        )
        .unwrap();
    }
    writeln!(
        rendered,
        "threads: {} frames: {} cpu_specs: {} cpu_batches: {} cpu_intervals: {} gpu_queues: {} gpu_work_intervals: {} gpu_breadcrumb_intervals: {}",
        output.dashboard.thread_info.len(),
        output.dashboard.frames.len(),
        output.dashboard.cpu.specs.len(),
        output.dashboard.cpu.batches.count,
        output.dashboard.cpu.batches.intervals,
        output.dashboard.gpu.queues.len(),
        output.dashboard.gpu.work.intervals,
        output.dashboard.gpu.breadcrumbs.intervals
    )
    .unwrap();
    for scope in output.dashboard.cpu.scopes.iter().take(20) {
        match scope.total_seconds {
            Some(seconds) => writeln!(
                rendered,
                "scope {} {:?}: count={} total_cycles={} total_seconds={}",
                scope.spec_id, scope.name, scope.count, scope.total_cycles, seconds
            )
            .unwrap(),
            None => writeln!(
                rendered,
                "scope {} {:?}: count={} total_cycles={}",
                scope.spec_id, scope.name, scope.count, scope.total_cycles
            )
            .unwrap(),
        }
    }
    writeln!(
        rendered,
        "cpu metadata: specs={} records={} scopes={} resolved={} unresolved={} metadata_bytes={}",
        output.dashboard.cpu.metadata.specs,
        output.dashboard.cpu.metadata.records,
        output.dashboard.cpu.metadata.scopes,
        output.dashboard.cpu.metadata.resolved_scopes,
        output.dashboard.cpu.metadata.unresolved_scopes,
        output.dashboard.cpu.metadata.metadata_bytes
    )
    .unwrap();
    for scope in output.dashboard.cpu.metadata.top.iter().take(10) {
        writeln!(
            rendered,
            "metadata scope {} {:?}: count={} total_cycles={}",
            scope.spec_id, scope.name, scope.count, scope.total_cycles
        )
        .unwrap();
    }
    writeln!(
        rendered,
        "cpu named events: {}",
        output.dashboard.cpu.named_events.len()
    )
    .unwrap();
    for event in output.dashboard.cpu.named_events.iter().take(10) {
        writeln!(
            rendered,
            "cpu event {:?}: observed={} sample={}",
            event.event,
            event.observed_count,
            event.sample.is_some()
        )
        .unwrap();
    }
    writeln!(
        rendered,
        "counters: specs={} samples={} int_samples={} float_samples={} unresolved={}",
        output.dashboard.counters.specs,
        output.dashboard.counters.samples,
        output.dashboard.counters.int_samples,
        output.dashboard.counters.float_samples,
        output.dashboard.counters.unresolved_samples
    )
    .unwrap();
    for counter in output.dashboard.counters.counters.iter().take(10) {
        writeln!(
            rendered,
            "counter {} {:?}: samples={} latest={:?} min={:?} max={:?}",
            counter.id, counter.name, counter.samples, counter.latest, counter.min, counter.max
        )
        .unwrap();
    }
    writeln!(
        rendered,
        "stats: specs={} floating_point={} memory={} clear_every_frame={} groups={} samples={} unresolved={} state_overflow={} malformed_batches={}",
        output.dashboard.stats.specs,
        output.dashboard.stats.floating_point_specs,
        output.dashboard.stats.memory_specs,
        output.dashboard.stats.clear_every_frame_specs,
        output.dashboard.stats.groups.len(),
        output.dashboard.stats.sample_events,
        output.dashboard.stats.unresolved_samples,
        output.dashboard.stats.sample_state_overflow,
        output.dashboard.stats.malformed_batches
    )
    .unwrap();
    for group in output.dashboard.stats.groups.iter().take(10) {
        writeln!(
            rendered,
            "stat group {:?}: specs={} floating_point={} memory={} clear_every_frame={}",
            group.name,
            group.specs,
            group.floating_point_specs,
            group.memory_specs,
            group.clear_every_frame_specs
        )
        .unwrap();
    }
    writeln!(
        rendered,
        "csv: categories={} stats={} declared={} inline={} unresolved={}",
        output.dashboard.csv.categories,
        output.dashboard.csv.stats,
        output.dashboard.csv.declared_stats,
        output.dashboard.csv.inline_stats,
        output.dashboard.csv.unresolved_stats
    )
    .unwrap();
    for category in output.dashboard.csv.top_categories.iter().take(10) {
        writeln!(
            rendered,
            "csv category {} {:?}: stats={} declared={} inline={}",
            category.index,
            category.name,
            category.stats,
            category.declared_stats,
            category.inline_stats
        )
        .unwrap();
    }
    writeln!(
        rendered,
        "loading: classes={} packages={} requests_begun={} requests_completed={} request_cycles={} async_starts={} async_suspends={} async_resumes={}",
        output.dashboard.loading.class_count,
        output.dashboard.loading.package_count,
        output.dashboard.loading.requests.begun,
        output.dashboard.loading.requests.completed,
        output.dashboard.loading.requests.total_cycles,
        output.dashboard.loading.async_loading.starts,
        output.dashboard.loading.async_loading.suspends,
        output.dashboard.loading.async_loading.resumes
    )
    .unwrap();
    for class in output.dashboard.loading.classes.iter().take(10) {
        writeln!(rendered, "load class 0x{:x}: {:?}", class.class, class.name).unwrap();
    }
    for package in output.dashboard.loading.packages.iter().take(10) {
        writeln!(
            rendered,
            "load package 0x{:x} {:?}: header={} imports={} exports={} priority={:?}",
            package.async_package,
            package.name,
            package.total_header_size,
            package.import_count,
            package.export_count,
            package.priority
        )
        .unwrap();
    }
    for request in output.dashboard.loading.requests.samples.iter().take(10) {
        writeln!(
            rendered,
            "load request {}: start={} end={} duration={}",
            request.request_id, request.start_cycle, request.end_cycle, request.duration_cycles
        )
        .unwrap();
    }
    writeln!(
        rendered,
        "iostore: backends={} created={} started={} completed={} failed={} unresolved={} requested_bytes={} completed_bytes={}",
        output.dashboard.io_store.backend_count,
        output.dashboard.io_store.requests_created,
        output.dashboard.io_store.requests_started,
        output.dashboard.io_store.requests_completed,
        output.dashboard.io_store.requests_failed,
        output.dashboard.io_store.requests_unresolved,
        output.dashboard.io_store.bytes_requested,
        output.dashboard.io_store.bytes_completed
    )
    .unwrap();
    for backend in output.dashboard.io_store.backends.iter().take(10) {
        writeln!(
            rendered,
            "iostore backend 0x{:x} {:?}: starts={}",
            backend.backend_handle, backend.name, backend.starts
        )
        .unwrap();
    }
    for request in output.dashboard.io_store.request_samples.iter().take(10) {
        writeln!(
            rendered,
            "iostore request 0x{:x}: status={:?} backend={:?} size={} completed={:?}",
            request.request_handle,
            request.status,
            request.backend_name,
            request.size,
            request.completed_size
        )
        .unwrap();
    }
    writeln!(
        rendered,
        "trace timing: threads={}",
        output.dashboard.trace_timing.thread_count
    )
    .unwrap();
    writeln!(
        rendered,
        "cpu end threads: {}",
        output.dashboard.cpu.end_threads.len()
    )
    .unwrap();
    writeln!(
        rendered,
        "memory: scopes={} unique_tags={}",
        output.dashboard.memory.scope_count,
        output.dashboard.memory.scopes.len()
    )
    .unwrap();
    writeln!(
        rendered,
        "metadata stack: clear_scope={} saved_stack={} restored_stack={} unmatched_restore={}",
        output.dashboard.metadata_stack.clear_scope_count,
        output.dashboard.metadata_stack.saved_stack_count,
        output.dashboard.metadata_stack.restored_stack_count,
        output.dashboard.metadata_stack.unmatched_restore_count
    )
    .unwrap();
    writeln!(
        rendered,
        "slate: added_widgets={} unique_widgets={}",
        output.dashboard.slate.added_widgets,
        output.dashboard.slate.widgets.len()
    )
    .unwrap();
    writeln!(
        rendered,
        "channels: count={} enabled={} read_only={} toggles={}",
        output.dashboard.channels.count,
        output.dashboard.channels.enabled,
        output.dashboard.channels.read_only,
        output.dashboard.channels.toggles
    )
    .unwrap();
    for channel in output.dashboard.channels.channels.iter().take(10) {
        writeln!(
            rendered,
            "channel {} {:?}: enabled={} read_only={} toggles={}",
            channel.id, channel.name, channel.is_enabled, channel.read_only, channel.toggle_count
        )
        .unwrap();
    }
    writeln!(
        rendered,
        "thread groups: begin={} end={} unmatched={} unclosed={}",
        output.dashboard.thread_groups.begin_events,
        output.dashboard.thread_groups.end_events,
        output.dashboard.thread_groups.unmatched_ends,
        output.dashboard.thread_groups.unclosed_groups
    )
    .unwrap();
    for group in output.dashboard.thread_groups.groups.iter().take(10) {
        writeln!(
            rendered,
            "thread group {:?}: begin={} end={} balanced={}",
            group.name, group.begin_count, group.end_count, group.balanced
        )
        .unwrap();
    }
    writeln!(
        rendered,
        "bookmarks: specs={} events={} unresolved={} format_arg_bytes={}",
        output.dashboard.annotations.bookmarks.specs,
        output.dashboard.annotations.bookmarks.events,
        output.dashboard.annotations.bookmarks.unresolved_events,
        output.dashboard.annotations.bookmarks.format_args_bytes
    )
    .unwrap();
    for bookmark in output
        .dashboard
        .annotations
        .bookmarks
        .bookmarks
        .iter()
        .take(10)
    {
        writeln!(
            rendered,
            "bookmark {} {:?}: count={} file={:?} line={:?}",
            bookmark.bookmark_point,
            bookmark.format_string,
            bookmark.count,
            bookmark.file,
            bookmark.line
        )
        .unwrap();
    }
    writeln!(
        rendered,
        "regions: begin={} end={} completed={} unmatched={} unterminated={}",
        output.dashboard.annotations.regions.begin_events,
        output.dashboard.annotations.regions.end_events,
        output.dashboard.annotations.regions.completed,
        output.dashboard.annotations.regions.unmatched_ends,
        output.dashboard.annotations.regions.unterminated
    )
    .unwrap();
    writeln!(
        rendered,
        "logging: categories={} message_specs={} messages={} unresolved={} unknown_category={} format_arg_bytes={}",
        output.dashboard.logging.categories,
        output.dashboard.logging.message_specs,
        output.dashboard.logging.messages,
        output.dashboard.logging.unresolved_messages,
        output.dashboard.logging.specs_with_unknown_category,
        output.dashboard.logging.format_args_bytes
    )
    .unwrap();
    for verbosity in &output.dashboard.logging.verbosity {
        writeln!(
            rendered,
            "log verbosity {:?}: message_specs={} messages={}",
            verbosity.verbosity, verbosity.message_specs, verbosity.messages
        )
        .unwrap();
    }
    for category in output.dashboard.logging.top_categories.iter().take(10) {
        writeln!(
            rendered,
            "log category {:?}: verbosity={:?} message_specs={} messages={}",
            category.name, category.default_verbosity, category.message_specs, category.messages
        )
        .unwrap();
    }
    for message in output.dashboard.logging.top_messages.iter().take(10) {
        writeln!(
            rendered,
            "log point {} {:?}: verbosity={:?} category={:?} count={} file={:?} line={:?}",
            message.log_point,
            message.format_string,
            message.verbosity,
            message.category,
            message.count,
            message.file,
            message.line
        )
        .unwrap();
    }
    writeln!(
        rendered,
        "unmodeled: event_types={} observed_events={}",
        output.dashboard.unmodeled.event_types, output.dashboard.unmodeled.observed_events
    )
    .unwrap();
    for event in output.dashboard.unmodeled.events.iter().take(10) {
        writeln!(
            rendered,
            "unmodeled {}.{}: count={}",
            event.logger, event.event, event.observed_count
        )
        .unwrap();
    }
    for thread in output.dashboard.cpu.threads.iter().take(10) {
        match thread.total_seconds {
            Some(seconds) => writeln!(
                rendered,
                "thread {} {:?}: count={} total_cycles={} total_seconds={}",
                thread.thread_id, thread.name, thread.count, thread.total_cycles, seconds
            )
            .unwrap(),
            None => writeln!(
                rendered,
                "thread {} {:?}: count={} total_cycles={}",
                thread.thread_id, thread.name, thread.count, thread.total_cycles
            )
            .unwrap(),
        }
        for scope in thread.scopes.iter().take(5) {
            writeln!(
                rendered,
                "  scope {} {:?}: count={} total_cycles={}",
                scope.spec_id, scope.name, scope.count, scope.total_cycles
            )
            .unwrap();
        }
    }
    for queue in output.dashboard.gpu.queues.iter().take(10) {
        writeln!(
            rendered,
            "gpu queue {} {:?}: work={} work_cycles={} waits={} frames={} draws={} primitives={}",
            queue.queue_id,
            queue.name,
            queue.work_count,
            queue.work_total_cycles,
            queue.wait_count,
            queue.frame_boundary_count,
            queue.draw_count,
            queue.primitive_count
        )
        .unwrap();
    }
    for breadcrumb in output.dashboard.gpu.breadcrumbs.top.iter().take(10) {
        writeln!(
            rendered,
            "gpu breadcrumb {} {:?}: count={} total_cycles={} metadata_events={}",
            breadcrumb.spec_id,
            breadcrumb.name,
            breadcrumb.count,
            breadcrumb.total_cycles,
            breadcrumb.metadata_events
        )
        .unwrap();
    }
    rendered
}

#[cfg(feature = "utrace")]
fn render_utrace_text_output(output: &UtraceInspectOutput) -> String {
    let mut rendered = String::new();
    writeln!(rendered, "path: {}", output.path).unwrap();
    writeln!(rendered, "magic: {:?}", output.trace.header.magic).unwrap();
    writeln!(
        rendered,
        "transport: {} protocol: {} packet_stream_offset: {}",
        output.trace.header.transport,
        output.trace.header.protocol,
        output.trace.header.packet_stream_offset
    )
    .unwrap();
    if let Some(metadata_size) = output.trace.header.metadata_size {
        writeln!(rendered, "metadata_size: {metadata_size}").unwrap();
    }
    writeln!(
        rendered,
        "packets: count={} sync={} threads={}",
        output.trace.packets.count,
        output.trace.packets.sync_count,
        output.trace.packets.thread_count
    )
    .unwrap();
    writeln!(
        rendered,
        "bytes: raw={} decoded={} compressed_payload={} compressed_decoded={}",
        output.trace.packets.raw_bytes,
        output.trace.packets.decoded_bytes,
        output.trace.packets.compressed_payload_bytes,
        output.trace.packets.compressed_decoded_bytes
    )
    .unwrap();
    for thread in &output.trace.packets.threads {
        writeln!(
            rendered,
            "thread {}: packets={} raw={} decoded={} compressed_payload={} compressed_decoded={}",
            thread.thread_id,
            thread.packet_count,
            thread.raw_bytes,
            thread.decoded_bytes,
            thread.compressed_payload_bytes,
            thread.compressed_decoded_bytes
        )
        .unwrap();
    }
    if let Some(prologue) = &output.trace.prologue {
        write!(
            rendered,
            "prologue: start_cycle={} cycle_frequency={} endian=0x{:04x} pointer_size={}",
            prologue.start_cycle, prologue.cycle_frequency, prologue.endian, prologue.pointer_size,
        )
        .unwrap();
        if let Some(start_date_time) = prologue.start_date_time {
            write!(rendered, " start_date_time={start_date_time}").unwrap();
        }
        writeln!(rendered).unwrap();
    }
    writeln!(
        rendered,
        "thread_info: count={}",
        output.trace.thread_info.len()
    )
    .unwrap();
    for thread in &output.trace.thread_info {
        writeln!(
            rendered,
            "thread_info {}: system_id={} sort_hint={} name={:?}",
            thread.thread_id, thread.system_id, thread.sort_hint, thread.name
        )
        .unwrap();
    }
    writeln!(rendered, "events: count={}", output.trace.events.len()).unwrap();
    for event in &output.trace.events {
        writeln!(
            rendered,
            "event {}.{} uid={} fields={} important={} no_sync={} maybe_has_aux={} definition={}",
            event.logger,
            event.event,
            event.uid,
            event.fields.len(),
            event.flags.important,
            event.flags.no_sync,
            event.flags.maybe_has_aux,
            event.flags.definition
        )
        .unwrap();
        for field in &event.fields {
            match field.ref_uid {
                Some(ref_uid) => writeln!(
                    rendered,
                    "  {}: {} {:?} offset={} size={} ref_uid={}",
                    field.name, field.type_name, field.family, field.offset, field.size, ref_uid
                )
                .unwrap(),
                None => writeln!(
                    rendered,
                    "  {}: {} {:?} offset={} size={}",
                    field.name, field.type_name, field.family, field.offset, field.size
                )
                .unwrap(),
            }
        }
    }
    rendered
}

fn render_text_output(output: &InspectOutput) -> String {
    let mut rendered = String::new();
    writeln!(rendered, "path: {}", output.path).unwrap();
    writeln!(rendered, "package_name: {}", output.package.name).unwrap();
    writeln!(
        rendered,
        "version: legacy={} ue4={} ue5={} licensee={}",
        output.package.version.legacy_file,
        output.package.version.ue4,
        output.package.version.ue5,
        output.package.version.licensee
    )
    .unwrap();
    writeln!(rendered, "package_flags: {}", output.package.package_flags).unwrap();
    writeln!(rendered, "summary_size: {}", output.package.summary_size).unwrap();
    writeln!(
        rendered,
        "total_header_size: {}",
        output.package.total_header_size
    )
    .unwrap();
    writeln!(
        rendered,
        "names: count={} offset={}",
        output.package.names.count, output.package.names.offset
    )
    .unwrap();
    if let Some(table) = &output.package.soft_object_paths {
        writeln!(
            rendered,
            "soft_object_paths: count={} offset={} parsed={}",
            table.count, table.offset, table.parsed_count
        )
        .unwrap();
    }
    writeln!(
        rendered,
        "imports: count={} offset={}",
        output.package.imports.count, output.package.imports.offset
    )
    .unwrap();
    writeln!(
        rendered,
        "exports: count={} offset={}",
        output.package.exports.count, output.package.exports.offset
    )
    .unwrap();
    for asset in &output.assets {
        writeln!(
            rendered,
            "asset: {} {} rows={}",
            asset.kind, asset.object_path, asset.row_count
        )
        .unwrap();
        if let Some(row_struct) = &asset.row_struct {
            writeln!(rendered, "row_struct: {row_struct}").unwrap();
        }
        if let Some(class_path) = &asset.class_path {
            writeln!(rendered, "class: {class_path}").unwrap();
        }
        if let Some(namespace) = &asset.string_table_namespace {
            writeln!(rendered, "namespace: {namespace}").unwrap();
        }
        if let Some(cpp_form) = &asset.enum_cpp_form {
            writeln!(rendered, "cpp_form: {cpp_form}").unwrap();
        }
        for entry in &asset.enum_entries {
            match &entry.display_name {
                Some(display_name) => writeln!(
                    rendered,
                    "  {} = {} ({display_name:?})",
                    entry.name, entry.value
                )
                .unwrap(),
                None => writeln!(rendered, "  {} = {}", entry.name, entry.value).unwrap(),
            }
        }
        if let Some(struct_flags) = asset.struct_flags {
            writeln!(rendered, "struct_flags: {struct_flags:#x}").unwrap();
        }
        for field in &asset.struct_fields {
            let mut line = format!("  {} ({})", field.name, field.type_name);
            if let Some(referenced) = &field.referenced_path {
                line.push_str(&format!(" -> {referenced}"));
            }
            if let Some(display_name) = &field.display_name {
                line.push_str(&format!(" [{display_name:?}]"));
            }
            writeln!(rendered, "{line}").unwrap();
        }
        for property in &asset.properties {
            writeln!(
                rendered,
                "  {} ({}) = {}",
                property.name,
                property.type_name,
                property.value.render()
            )
            .unwrap();
        }
        for row in &asset.rows {
            writeln!(rendered, "  row {}:", row.name).unwrap();
            for property in &row.properties {
                writeln!(
                    rendered,
                    "    {} ({}) = {}",
                    property.name,
                    property.type_name,
                    property.value.render()
                )
                .unwrap();
            }
        }
        for row in &asset.curve_rows {
            writeln!(rendered, "  curve {}:", row.name).unwrap();
            for key in &row.keys {
                writeln!(rendered, "    {} => {}", key.time, key.value).unwrap();
            }
        }
        for entry in &asset.string_table_entries {
            writeln!(rendered, "  {} = {}", entry.key, entry.source).unwrap();
        }
        if !asset.bones.is_empty() {
            writeln!(rendered, "  bones: {}", asset.bones.len()).unwrap();
            for bone in &asset.bones {
                writeln!(rendered, "    {} parent={}", bone.name, bone.parent_index).unwrap();
            }
        }
    }
    for error in &output.decode_errors {
        writeln!(
            rendered,
            "decode_error: {} [{}] {}",
            error.object_path, error.kind, error.message
        )
        .unwrap();
    }
    rendered
}

fn write_stdout(bytes: &[u8]) -> u8 {
    if let Err(error) = io::stdout().lock().write_all(bytes) {
        eprintln!("uasset: failed to write output: {error}");
        EXIT_INTERNAL
    } else {
        EXIT_SUCCESS
    }
}

fn write_error(format: OutputFormat, error: ErrorOutput) {
    match format {
        OutputFormat::Text => {
            let location = match (error.offset, error.field.as_deref()) {
                (Some(offset), Some(field)) => format!(" at byte {offset} ({field})"),
                (Some(offset), None) => format!(" at byte {offset}"),
                (None, Some(field)) => format!(" ({field})"),
                (None, None) => String::new(),
            };
            eprintln!(
                "uasset: {} error for {}{location}: {}",
                error.kind, error.path, error.message
            );
        }
        OutputFormat::Json => match serde_json::to_vec(&error) {
            Ok(mut rendered) => {
                rendered.push(b'\n');
                if let Err(write_error) = io::stderr().lock().write_all(&rendered) {
                    eprintln!("uasset: failed to write error output: {write_error}");
                }
            }
            Err(serialization_error) => {
                eprintln!("uasset: failed to serialize error: {serialization_error}");
            }
        },
    }
}

#[cfg(feature = "utrace")]
fn write_utrace_error(format: OutputFormat, error: UtraceErrorOutput) {
    match format {
        OutputFormat::Text => {
            let location = match (error.offset, error.field.as_deref()) {
                (Some(offset), Some(field)) => format!(" at byte {offset} ({field})"),
                (Some(offset), None) => format!(" at byte {offset}"),
                (None, Some(field)) => format!(" ({field})"),
                (None, None) => String::new(),
            };
            eprintln!(
                "uasset: {} error for {}{location}: {}",
                error.kind, error.path, error.message
            );
        }
        OutputFormat::Json => match serde_json::to_vec(&error) {
            Ok(mut rendered) => {
                rendered.push(b'\n');
                if let Err(write_error) = io::stderr().lock().write_all(&rendered) {
                    eprintln!("uasset: failed to write utrace error output: {write_error}");
                }
            }
            Err(serialization_error) => {
                eprintln!("uasset: failed to serialize utrace error: {serialization_error}");
            }
        },
    }
}

fn exit_code_for_package_error(error: &PackageError) -> u8 {
    match error.kind() {
        PackageErrorKind::MalformedData => EXIT_MALFORMED,
        PackageErrorKind::ResourceLimit => EXIT_RESOURCE_LIMIT,
        PackageErrorKind::UnsupportedFormat
        | PackageErrorKind::UnsupportedVersion
        | PackageErrorKind::UnsupportedCapability => EXIT_UNSUPPORTED,
    }
}

#[cfg(feature = "utrace")]
fn exit_code_for_trace_error(error: &TraceError) -> u8 {
    match error.kind() {
        TraceErrorKind::MalformedData => EXIT_MALFORMED,
        TraceErrorKind::ResourceLimit => EXIT_RESOURCE_LIMIT,
        TraceErrorKind::UnsupportedFormat => EXIT_UNSUPPORTED,
    }
}

#[cfg(feature = "utrace")]
fn exit_code_for_timeline_index_error(error: &TimelineIndexError) -> u8 {
    match error {
        TimelineIndexError::Io(_) => EXIT_IO,
        TimelineIndexError::Malformed(_) => EXIT_MALFORMED,
        TimelineIndexError::InvalidQuery(_) => EXIT_USAGE,
        TimelineIndexError::ResourceLimit(_) => EXIT_RESOURCE_LIMIT,
        _ => EXIT_INTERNAL,
    }
}

#[derive(Serialize)]
struct InspectOutput {
    schema_version: u32,
    status: &'static str,
    path: String,
    package: PackageOutput,
    assets: Vec<AssetOutput>,
    /// Exports that failed to decode. Non-empty implies `status: "partial"`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    decode_errors: Vec<DecodeErrorOutput>,
}

#[derive(Serialize)]
struct DecodeErrorOutput {
    object_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    class_path: Option<String>,
    kind: &'static str,
    message: String,
}

#[derive(Serialize)]
struct AuthoringSnapshotOutput {
    contract: AuthoringContractOutput,
    authority: AuthoringAuthorityOutput,
    completeness: &'static str,
    table: AuthoringTableOutput,
    diagnostics: Vec<AuthoringDiagnosticOutput>,
}

#[derive(Serialize)]
struct AuthoringContractOutput {
    name: &'static str,
    version: AuthoringVersionOutput,
}

#[derive(Serialize)]
struct AuthoringVersionOutput {
    major: u32,
    minor: u32,
}

#[derive(Serialize)]
struct AuthoringAuthorityOutput {
    kind: &'static str,
    #[serde(rename = "packageName")]
    package_name: String,
}

#[derive(Serialize)]
struct AuthoringTableOutput {
    kind: &'static str,
    #[serde(rename = "objectPath")]
    object_path: String,
    #[serde(rename = "rowStruct")]
    row_struct: String,
    #[serde(rename = "parentTables")]
    parent_tables: Vec<String>,
    rows: Vec<AuthoringRowOutput>,
}

#[derive(Serialize)]
struct AuthoringRowOutput {
    id: String,
    name: String,
    fields: Vec<AuthoringFieldOutput>,
}

#[derive(Serialize)]
struct AuthoringFieldOutput {
    name: String,
    #[serde(rename = "typeName")]
    type_name: String,
    value: AuthoringValueOutput,
}

#[derive(Serialize)]
struct AuthoringMapEntryOutput {
    key: AuthoringValueOutput,
    value: AuthoringValueOutput,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum AuthoringValueOutput {
    Bool {
        value: bool,
    },
    Int {
        value: String,
    },
    Uint {
        value: String,
    },
    Float {
        value: AuthoringFloatOutput,
    },
    Double {
        value: AuthoringFloatOutput,
    },
    Name {
        value: String,
    },
    Enum {
        value: String,
    },
    String {
        value: String,
    },
    Text {
        value: String,
    },
    Vector {
        x: f32,
        y: f32,
        z: f32,
    },
    ObjectRef {
        value: Option<String>,
    },
    Guid {
        value: String,
    },
    SoftObjectPath {
        value: String,
    },
    Array {
        values: Vec<AuthoringValueOutput>,
    },
    Set {
        values: Vec<AuthoringValueOutput>,
    },
    Map {
        entries: Vec<AuthoringMapEntryOutput>,
    },
    Struct {
        fields: Vec<AuthoringFieldOutput>,
    },
    Unsupported {
        reason: String,
        #[serde(rename = "byteSize")]
        byte_size: u64,
    },
}

#[derive(Serialize)]
#[serde(untagged)]
enum AuthoringFloatOutput {
    Finite(f64),
    Special(&'static str),
}

impl AuthoringFloatOutput {
    fn from_f64(value: f64) -> Self {
        if value.is_nan() {
            Self::Special("nan")
        } else if value == f64::INFINITY {
            Self::Special("infinity")
        } else if value == f64::NEG_INFINITY {
            Self::Special("-infinity")
        } else {
            Self::Finite(value)
        }
    }
}

#[derive(Serialize)]
struct AuthoringDiagnosticOutput {
    code: &'static str,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
}

impl AuthoringSnapshotOutput {
    fn from_inspect(inspect: &InspectOutput, table: &AssetOutput) -> Self {
        let mut partial = !inspect.decode_errors.is_empty();
        let rows = table
            .rows
            .iter()
            .map(|row| {
                let fields = row
                    .properties
                    .iter()
                    .map(|property| {
                        if property.value.contains_unsupported() {
                            partial = true;
                        }
                        AuthoringFieldOutput::from_property(property)
                    })
                    .collect();
                AuthoringRowOutput {
                    id: format!("row:{}", row.name),
                    name: row.name.clone(),
                    fields,
                }
            })
            .collect();
        Self {
            contract: AuthoringContractOutput {
                name: "unreal-authoring",
                version: AuthoringVersionOutput { major: 1, minor: 0 },
            },
            authority: AuthoringAuthorityOutput {
                kind: "project_files",
                package_name: inspect.package.name.clone(),
            },
            completeness: if partial { "partial" } else { "complete" },
            table: AuthoringTableOutput {
                kind: if table.kind == "CompositeDataTable" {
                    "composite_data_table"
                } else {
                    "data_table"
                },
                object_path: table.object_path.clone(),
                row_struct: table.row_struct.clone().unwrap_or_default(),
                parent_tables: table.parent_tables.clone(),
                rows,
            },
            diagnostics: inspect
                .decode_errors
                .iter()
                .map(|error| AuthoringDiagnosticOutput {
                    code: error.kind,
                    message: error.message.clone(),
                    path: Some(error.object_path.clone()),
                })
                .collect(),
        }
    }
}

impl AuthoringFieldOutput {
    fn from_property(property: &PropertyOutput) -> Self {
        Self {
            name: property.name.clone(),
            type_name: property.type_name.clone(),
            value: AuthoringValueOutput::from_property(&property.value),
        }
    }
}

impl AuthoringValueOutput {
    fn from_property(value: &PropertyValueOutput) -> Self {
        match value {
            PropertyValueOutput::Bool { value } => Self::Bool { value: *value },
            PropertyValueOutput::Int { value } => Self::Int {
                value: value.to_string(),
            },
            PropertyValueOutput::Uint { value } => Self::Uint {
                value: value.to_string(),
            },
            PropertyValueOutput::Float { value } => Self::Float {
                value: AuthoringFloatOutput::from_f64(f64::from(*value)),
            },
            PropertyValueOutput::Double { value } => Self::Double {
                value: AuthoringFloatOutput::from_f64(*value),
            },
            PropertyValueOutput::Name { value } => Self::Name {
                value: value.clone(),
            },
            PropertyValueOutput::Enum { value } => Self::Enum {
                value: value.clone(),
            },
            PropertyValueOutput::String { value } => Self::String {
                value: value.clone(),
            },
            PropertyValueOutput::Text { value } => Self::Text {
                value: value.clone(),
            },
            PropertyValueOutput::Vector { x, y, z } => Self::Vector {
                x: *x,
                y: *y,
                z: *z,
            },
            PropertyValueOutput::IntPoint { x, y } => Self::Struct {
                fields: vec![
                    AuthoringFieldOutput {
                        name: "X".to_owned(),
                        type_name: "IntProperty".to_owned(),
                        value: Self::Int {
                            value: x.to_string(),
                        },
                    },
                    AuthoringFieldOutput {
                        name: "Y".to_owned(),
                        type_name: "IntProperty".to_owned(),
                        value: Self::Int {
                            value: y.to_string(),
                        },
                    },
                ],
            },
            PropertyValueOutput::ObjectRef { value } => Self::ObjectRef {
                value: value.clone(),
            },
            PropertyValueOutput::Guid { value } => Self::Guid {
                value: value.clone(),
            },
            PropertyValueOutput::SoftObjectPath { value } => Self::SoftObjectPath {
                value: value.clone(),
            },
            PropertyValueOutput::Array { values } => Self::Array {
                values: values.iter().map(Self::from_property).collect(),
            },
            PropertyValueOutput::Set { values } => Self::Set {
                values: values.iter().map(Self::from_property).collect(),
            },
            PropertyValueOutput::Map { entries } => Self::Map {
                entries: entries
                    .iter()
                    .map(|entry| AuthoringMapEntryOutput {
                        key: Self::from_property(&entry.key),
                        value: Self::from_property(&entry.value),
                    })
                    .collect(),
            },
            PropertyValueOutput::Struct { properties } => Self::Struct {
                fields: properties
                    .iter()
                    .map(AuthoringFieldOutput::from_property)
                    .collect(),
            },
            PropertyValueOutput::Raw { reason, size } => Self::Unsupported {
                reason: reason.clone(),
                byte_size: *size,
            },
        }
    }
}

impl PropertyValueOutput {
    fn contains_unsupported(&self) -> bool {
        match self {
            Self::Raw { .. } => true,
            Self::Array { values } | Self::Set { values } => {
                values.iter().any(Self::contains_unsupported)
            }
            Self::Map { entries } => entries.iter().any(|entry| {
                entry.key.contains_unsupported() || entry.value.contains_unsupported()
            }),
            Self::Struct { properties } => properties
                .iter()
                .any(|property| property.value.contains_unsupported()),
            _ => false,
        }
    }
}

#[cfg(feature = "utrace")]
#[derive(Serialize)]
struct UtraceInspectOutput {
    schema_version: u32,
    status: &'static str,
    path: String,
    trace: TraceInspect,
}

#[cfg(feature = "utrace")]
#[derive(Serialize)]
struct UtraceDashboardOutput {
    schema_version: u32,
    status: &'static str,
    path: String,
    dashboard: TraceDashboard,
}

#[cfg(feature = "utrace")]
#[derive(Serialize)]
struct UtraceDashboardBundleOutput {
    schema_version: u32,
    status: &'static str,
    path: String,
    dashboard: TraceDashboard,
    inventory: TraceInventory,
}

#[cfg(feature = "utrace")]
#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ProgressiveCliEvent {
    Bootstrap {
        protocol_version: u32,
        sequence: u64,
        progress: utrace_parser::utrace_progress::DecodeProgress,
        bootstrap: utrace_parser::utrace_progress::DashboardBootstrap,
    },
    Snapshot {
        protocol_version: u32,
        sequence: u64,
        progress: utrace_parser::utrace_progress::DecodeProgress,
        patch: utrace_parser::utrace_progress::DashboardPatch,
    },
    Complete {
        protocol_version: u32,
        sequence: u64,
        progress: utrace_parser::utrace_progress::DecodeProgress,
        dashboard: Box<UtraceDashboardOutput>,
        inventory: Box<UtraceInventoryOutput>,
        #[serde(skip_serializing_if = "Option::is_none")]
        timeline_index: Option<ProgressiveTimelineIndexOutput>,
        #[serde(skip_serializing_if = "Option::is_none")]
        timeline_index_warning: Option<String>,
    },
    Failed {
        protocol_version: u32,
        sequence: u64,
        error: String,
    },
}

#[cfg(feature = "utrace")]
#[derive(Serialize)]
struct ProgressiveTimelineIndexOutput {
    index_path: String,
    index: CpuTimelineIndexInfo,
}

#[cfg(feature = "utrace")]
#[derive(Serialize)]
struct UtraceInventoryOutput {
    schema_version: u32,
    status: &'static str,
    path: String,
    inventory: TraceInventory,
}

#[cfg(feature = "utrace")]
#[derive(Serialize)]
struct UtraceCoverageOutput {
    schema_version: u32,
    status: &'static str,
    path: String,
    coverage: TraceCoverage,
}

#[cfg(feature = "utrace")]
#[derive(Serialize)]
struct UtraceTimelineIndexOutput {
    schema_version: u32,
    status: &'static str,
    path: String,
    index_path: String,
    index: CpuTimelineIndexInfo,
}

#[cfg(feature = "utrace")]
#[derive(Serialize)]
struct UtraceTimelineQueryOutput {
    schema_version: u32,
    status: &'static str,
    path: String,
    timeline: CpuTimelineQueryResult,
}

impl InspectOutput {
    fn from_summary(path: String, summary: &PackageSummary) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            status: "ok",
            path,
            package: PackageOutput {
                name: summary.package_name.clone(),
                version: VersionOutput {
                    legacy_file: summary.versions.legacy_file_version,
                    legacy_ue3: summary.versions.legacy_ue3,
                    ue4: summary.versions.ue4,
                    ue5: summary.versions.ue5,
                    licensee: summary.versions.licensee,
                },
                package_flags: summary.versions.package_flags.bits(),
                summary_size: summary.span.len(),
                total_header_size: summary.total_header_size,
                names: TableOutput::from(summary.names),
                soft_object_paths: summary
                    .soft_object_paths
                    .map(|table| SoftObjectPathsOutput {
                        count: table.count,
                        offset: table.offset.get(),
                        parsed_count: 0,
                    }),
                imports: TableOutput::from(summary.imports),
                exports: TableOutput::from(summary.exports),
            },
            assets: Vec::new(),
            decode_errors: Vec::new(),
        }
    }

    /// Decodes every export, collecting per-export failures instead of aborting.
    /// A single unsupported or malformed export no longer blanks the whole file;
    /// callers report `status: "partial"` when `decode_errors` is non-empty.
    fn from_package(path: String, source: &[u8], package: &Package) -> Self {
        let mut output = Self::from_summary(path, &package.summary);
        if let Some(table) = &mut output.package.soft_object_paths {
            table.parsed_count = package.soft_object_paths.len();
        }
        let schemas = EmptySchemas;
        let context = AssetDecodeContext {
            source,
            package,
            schemas: &schemas,
        };
        for export in &package.exports {
            match decode_export(export, &context) {
                Ok(Some(decoded)) => {
                    output
                        .assets
                        .push(asset_output_from_decoded(package, decoded));
                }
                Ok(None) => {}
                Err(error) => {
                    output.decode_errors.push(DecodeErrorOutput {
                        object_path: export.object_path.to_string(),
                        class_path: export.class_path.as_ref().map(ToString::to_string),
                        kind: asset_error_kind_name(error.kind()),
                        message: error.message().to_owned(),
                    });
                }
            }
        }
        if !output.decode_errors.is_empty() {
            output.status = "partial";
        }
        output
    }
}

fn asset_error_kind_name(kind: AssetErrorKind) -> &'static str {
    match kind {
        AssetErrorKind::MalformedData => "malformed_data",
        AssetErrorKind::ResourceLimit => "resource_limit",
        AssetErrorKind::UnsupportedFormat => "unsupported_format",
        AssetErrorKind::UnsupportedVersion => "unsupported_version",
        AssetErrorKind::UnsupportedCapability => "unsupported_capability",
    }
}

fn asset_output_from_decoded(package: &Package, decoded: DecodedAsset) -> AssetOutput {
    match decoded {
        DecodedAsset::DataTable(datatable) => AssetOutput {
            tail_bytes: 0,
            bones: Vec::new(),
            kind: match datatable.kind {
                uasset_parser::asset::DataTableKind::Plain => "DataTable",
                uasset_parser::asset::DataTableKind::Composite => "CompositeDataTable",
            },
            object_path: datatable.object_path.to_string(),
            class_path: None,
            object_guid: None,
            row_struct: datatable.row_struct.map(|path| path.to_string()),
            parent_tables: datatable
                .parent_tables
                .iter()
                .map(|path| path.to_string())
                .collect(),
            string_table_namespace: None,
            string_table_entries: Vec::new(),
            enum_cpp_form: None,
            enum_entries: Vec::new(),
            struct_flags: None,
            struct_fields: Vec::new(),
            properties: Vec::new(),
            row_count: datatable.rows.len(),
            curve_rows: Vec::new(),
            rows: datatable
                .rows
                .iter()
                .map(|row| RowOutput {
                    name: resolve_name_or_placeholder(package, row.name),
                    properties: property_outputs(package, &row.properties),
                })
                .collect(),
        },
        DecodedAsset::CurveTable(curve_table) => AssetOutput {
            tail_bytes: 0,
            bones: Vec::new(),
            kind: "CurveTable",
            object_path: curve_table.object_path.to_string(),
            class_path: Some(uasset_parser::asset::CURVETABLE_CLASS.to_owned()),
            object_guid: None,
            row_struct: None,
            parent_tables: Vec::new(),
            string_table_namespace: None,
            string_table_entries: Vec::new(),
            enum_cpp_form: None,
            enum_entries: Vec::new(),
            struct_flags: None,
            struct_fields: Vec::new(),
            properties: property_outputs(package, &curve_table.properties),
            row_count: curve_table.rows.len(),
            curve_rows: curve_table
                .rows
                .iter()
                .map(|row| CurveRowOutput {
                    name: resolve_name_or_placeholder(package, row.name),
                    keys: row
                        .keys
                        .iter()
                        .map(|key| CurveKeyOutput {
                            time: key.time(),
                            value: key.value(),
                        })
                        .collect(),
                })
                .collect(),
            rows: Vec::new(),
        },
        DecodedAsset::StringTable(string_table) => AssetOutput {
            tail_bytes: 0,
            bones: Vec::new(),
            kind: "StringTable",
            object_path: string_table.object_path.to_string(),
            class_path: Some(uasset_parser::asset::STRINGTABLE_CLASS.to_owned()),
            object_guid: None,
            row_struct: None,
            parent_tables: Vec::new(),
            string_table_namespace: Some(string_table.namespace),
            string_table_entries: string_table
                .entries
                .into_iter()
                .map(|entry| StringTableEntryOutput {
                    key: entry.key,
                    source: entry.source,
                })
                .collect(),
            enum_cpp_form: None,
            enum_entries: Vec::new(),
            struct_flags: None,
            struct_fields: Vec::new(),
            properties: Vec::new(),
            row_count: 0,
            curve_rows: Vec::new(),
            rows: Vec::new(),
        },
        DecodedAsset::DataAsset(data_asset) => AssetOutput {
            tail_bytes: 0,
            bones: Vec::new(),
            kind: data_asset_kind(data_asset.class_path.as_str()),
            object_path: data_asset.object_path.to_string(),
            class_path: Some(data_asset.class_path.to_string()),
            object_guid: data_asset.object_guid.map(|guid| guid.to_string()),
            row_struct: None,
            parent_tables: Vec::new(),
            string_table_namespace: None,
            string_table_entries: Vec::new(),
            enum_cpp_form: None,
            enum_entries: Vec::new(),
            struct_flags: None,
            struct_fields: Vec::new(),
            properties: property_outputs(package, &data_asset.properties),
            row_count: 0,
            curve_rows: Vec::new(),
            rows: Vec::new(),
        },
        DecodedAsset::UObject(object) => AssetOutput {
            kind: "UObject",
            object_path: object.object_path.to_string(),
            class_path: Some(object.class_path.to_string()),
            object_guid: object.object_guid.map(|guid| guid.to_string()),
            row_struct: None,
            parent_tables: Vec::new(),
            string_table_namespace: None,
            string_table_entries: Vec::new(),
            enum_cpp_form: None,
            enum_entries: Vec::new(),
            struct_flags: None,
            struct_fields: Vec::new(),
            properties: property_outputs(package, &object.properties),
            tail_bytes: object.tail.len(),
            bones: Vec::new(),
            row_count: 0,
            curve_rows: Vec::new(),
            rows: Vec::new(),
        },
        DecodedAsset::Skeleton(skeleton) => AssetOutput {
            kind: "Skeleton",
            object_path: skeleton.object_path.to_string(),
            class_path: Some(SKELETON_CLASS.to_owned()),
            object_guid: skeleton.object_guid.map(|guid| guid.to_string()),
            row_struct: None,
            parent_tables: Vec::new(),
            string_table_namespace: None,
            string_table_entries: Vec::new(),
            enum_cpp_form: None,
            enum_entries: Vec::new(),
            struct_flags: None,
            struct_fields: Vec::new(),
            properties: property_outputs(package, &skeleton.properties),
            tail_bytes: 0,
            bones: skeleton
                .bones
                .iter()
                .map(|bone| BoneOutput {
                    name: resolve_name_or_placeholder(package, bone.name),
                    parent_index: bone.parent_index,
                })
                .collect(),
            row_count: 0,
            curve_rows: Vec::new(),
            rows: Vec::new(),
        },
        DecodedAsset::Enum(decoded_enum) => AssetOutput {
            tail_bytes: 0,
            bones: Vec::new(),
            kind: "Enum",
            object_path: decoded_enum.object_path.to_string(),
            class_path: Some(USERDEFINEDENUM_CLASS.to_owned()),
            object_guid: None,
            row_struct: None,
            parent_tables: Vec::new(),
            string_table_namespace: None,
            string_table_entries: Vec::new(),
            enum_cpp_form: Some(enum_cpp_form_name(decoded_enum.cpp_form)),
            enum_entries: decoded_enum
                .entries
                .iter()
                .map(|entry| EnumEntryOutput {
                    name: resolve_name_or_placeholder(package, entry.name),
                    value: entry.value,
                    display_name: entry.display_name.clone(),
                })
                .collect(),
            struct_flags: None,
            struct_fields: Vec::new(),
            properties: Vec::new(),
            row_count: decoded_enum.entries.len(),
            curve_rows: Vec::new(),
            rows: Vec::new(),
        },
        DecodedAsset::Struct(decoded_struct) => AssetOutput {
            tail_bytes: 0,
            bones: Vec::new(),
            kind: "Struct",
            object_path: decoded_struct.object_path.to_string(),
            class_path: Some(USERDEFINEDSTRUCT_CLASS.to_owned()),
            object_guid: None,
            row_struct: None,
            parent_tables: Vec::new(),
            string_table_namespace: None,
            string_table_entries: Vec::new(),
            enum_cpp_form: None,
            enum_entries: Vec::new(),
            struct_flags: Some(decoded_struct.struct_flags),
            struct_fields: decoded_struct
                .fields
                .iter()
                .map(|field| StructFieldOutput {
                    name: resolve_name_or_placeholder(package, field.name),
                    type_name: resolve_name_or_placeholder(package, field.type_name),
                    referenced_path: field.referenced_path.as_ref().map(ToString::to_string),
                    display_name: field.display_name.clone(),
                })
                .collect(),
            properties: property_outputs(package, &decoded_struct.default_values),
            row_count: decoded_struct.fields.len(),
            curve_rows: Vec::new(),
            rows: Vec::new(),
        },
    }
}

fn enum_cpp_form_name(cpp_form: EnumCppForm) -> &'static str {
    match cpp_form {
        EnumCppForm::Regular => "Regular",
        EnumCppForm::Namespaced => "Namespaced",
        EnumCppForm::EnumClass => "EnumClass",
    }
}

fn property_outputs(
    package: &Package,
    stream: &uasset_parser::property::PropertyStream,
) -> Vec<PropertyOutput> {
    stream
        .records
        .iter()
        .map(|record| PropertyOutput::from_record(record, package))
        .collect()
}

fn data_asset_kind(class_path: &str) -> &'static str {
    match class_path {
        PRIMARY_DATA_ASSET_CLASS => "PrimaryDataAsset",
        DATA_ASSET_CLASS => "DataAsset",
        _ => "DataAsset",
    }
}

struct EmptySchemas;

impl SchemaProvider for EmptySchemas {
    fn find_struct(&self, _path: &uasset_parser::package::ObjectPath) -> Option<&StructSchema> {
        None
    }

    fn find_class(&self, _path: &uasset_parser::package::ObjectPath) -> Option<&ClassSchema> {
        None
    }
}

#[derive(Serialize)]
struct PackageOutput {
    name: String,
    version: VersionOutput,
    package_flags: u32,
    summary_size: u64,
    total_header_size: u32,
    names: TableOutput,
    #[serde(skip_serializing_if = "Option::is_none")]
    soft_object_paths: Option<SoftObjectPathsOutput>,
    imports: TableOutput,
    exports: TableOutput,
}

#[derive(Serialize)]
struct SoftObjectPathsOutput {
    count: u32,
    offset: u64,
    parsed_count: usize,
}

#[derive(Serialize)]
struct AssetOutput {
    kind: &'static str,
    object_path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    class_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    object_guid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    row_struct: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    parent_tables: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    string_table_namespace: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    string_table_entries: Vec<StringTableEntryOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enum_cpp_form: Option<&'static str>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    enum_entries: Vec<EnumEntryOutput>,
    #[serde(skip_serializing_if = "Option::is_none")]
    struct_flags: Option<u32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    struct_fields: Vec<StructFieldOutput>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    properties: Vec<PropertyOutput>,
    /// Count of unparsed class-specific bytes retained after the property stream
    /// (e.g. a `StaticMesh`/`Texture2D` binary tail). Omitted when zero.
    #[serde(skip_serializing_if = "is_zero_u64")]
    tail_bytes: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    bones: Vec<BoneOutput>,
    row_count: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    curve_rows: Vec<CurveRowOutput>,
    rows: Vec<RowOutput>,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

#[derive(Serialize)]
struct BoneOutput {
    name: String,
    parent_index: i32,
}

#[derive(Serialize)]
struct EnumEntryOutput {
    name: String,
    value: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
}

#[derive(Serialize)]
struct StructFieldOutput {
    name: String,
    #[serde(rename = "type")]
    type_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    referenced_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    display_name: Option<String>,
}

#[derive(Serialize)]
struct RowOutput {
    name: String,
    properties: Vec<PropertyOutput>,
}

#[derive(Serialize)]
struct CurveRowOutput {
    name: String,
    keys: Vec<CurveKeyOutput>,
}

#[derive(Serialize)]
struct CurveKeyOutput {
    time: f32,
    value: f32,
}

#[derive(Serialize)]
struct StringTableEntryOutput {
    key: String,
    source: String,
}

#[derive(Serialize)]
struct PropertyOutput {
    name: String,
    #[serde(rename = "type")]
    type_name: String,
    #[serde(flatten)]
    value: PropertyValueOutput,
}

impl PropertyOutput {
    fn from_record(record: &PropertyRecord, package: &Package) -> Self {
        let value = match &record.value {
            PropertyValue::Bool(value) => PropertyValueOutput::Bool { value: *value },
            PropertyValue::Int(value) => PropertyValueOutput::Int { value: *value },
            PropertyValue::UInt(value) => PropertyValueOutput::Uint { value: *value },
            PropertyValue::Float(value) => PropertyValueOutput::Float { value: *value },
            PropertyValue::Double(value) => PropertyValueOutput::Double { value: *value },
            PropertyValue::Name(name) => PropertyValueOutput::Name {
                value: resolve_name_or_placeholder(package, *name),
            },
            PropertyValue::Enum(name) => PropertyValueOutput::Enum {
                value: resolve_name_or_placeholder(package, *name),
            },
            PropertyValue::String(value) => PropertyValueOutput::String {
                value: value.clone(),
            },
            PropertyValue::Text(text) => PropertyValueOutput::Text {
                value: text.source.clone(),
            },
            PropertyValue::Vector(vector) => PropertyValueOutput::Vector {
                x: vector.x as f32,
                y: vector.y as f32,
                z: vector.z as f32,
            },
            PropertyValue::IntPoint(point) => PropertyValueOutput::IntPoint {
                x: point.x,
                y: point.y,
            },
            PropertyValue::ObjectRef(index) => PropertyValueOutput::ObjectRef {
                value: resolve_object_ref(package, *index),
            },
            PropertyValue::Guid(guid) => PropertyValueOutput::Guid {
                value: guid.to_string(),
            },
            PropertyValue::SoftObjectPath(path) => PropertyValueOutput::SoftObjectPath {
                value: path.clone(),
            },
            PropertyValue::Array(values) => PropertyValueOutput::Array {
                values: values
                    .iter()
                    .map(|value| value_output(package, value))
                    .collect(),
            },
            PropertyValue::Set(values) => PropertyValueOutput::Set {
                values: values
                    .iter()
                    .map(|value| value_output(package, value))
                    .collect(),
            },
            PropertyValue::Map(entries) => PropertyValueOutput::Map {
                entries: entries
                    .iter()
                    .map(|entry| MapEntryOutput {
                        key: value_output(package, &entry.key),
                        value: value_output(package, &entry.value),
                    })
                    .collect(),
            },
            PropertyValue::Struct(stream) => PropertyValueOutput::Struct {
                properties: stream
                    .records
                    .iter()
                    .map(|record| PropertyOutput::from_record(record, package))
                    .collect(),
            },
            PropertyValue::Raw { reason } => PropertyValueOutput::Raw {
                reason: render_raw_reason(reason),
                size: record.payload.len(),
            },
            _ => PropertyValueOutput::Raw {
                reason: "unsupported property value".to_owned(),
                size: record.payload.len(),
            },
        };
        Self {
            name: resolve_name_or_placeholder(package, record.name),
            type_name: resolve_name_or_placeholder(package, record.type_name.name),
            value,
        }
    }
}

#[derive(Serialize)]
struct MapEntryOutput {
    key: PropertyValueOutput,
    value: PropertyValueOutput,
}

#[derive(Serialize)]
#[serde(tag = "value_kind", rename_all = "snake_case")]
enum PropertyValueOutput {
    Bool { value: bool },
    Int { value: i64 },
    Uint { value: u64 },
    Float { value: f32 },
    Double { value: f64 },
    Name { value: String },
    Enum { value: String },
    String { value: String },
    Text { value: String },
    Vector { x: f32, y: f32, z: f32 },
    IntPoint { x: i32, y: i32 },
    ObjectRef { value: Option<String> },
    Guid { value: String },
    SoftObjectPath { value: String },
    Array { values: Vec<PropertyValueOutput> },
    Set { values: Vec<PropertyValueOutput> },
    Map { entries: Vec<MapEntryOutput> },
    Struct { properties: Vec<PropertyOutput> },
    Raw { reason: String, size: u64 },
}

fn value_output(package: &Package, value: &PropertyValue) -> PropertyValueOutput {
    match value {
        PropertyValue::Bool(value) => PropertyValueOutput::Bool { value: *value },
        PropertyValue::Int(value) => PropertyValueOutput::Int { value: *value },
        PropertyValue::UInt(value) => PropertyValueOutput::Uint { value: *value },
        PropertyValue::Float(value) => PropertyValueOutput::Float { value: *value },
        PropertyValue::Double(value) => PropertyValueOutput::Double { value: *value },
        PropertyValue::Name(name) => PropertyValueOutput::Name {
            value: resolve_name_or_placeholder(package, *name),
        },
        PropertyValue::Enum(name) => PropertyValueOutput::Enum {
            value: resolve_name_or_placeholder(package, *name),
        },
        PropertyValue::String(value) => PropertyValueOutput::String {
            value: value.clone(),
        },
        PropertyValue::Text(text) => PropertyValueOutput::Text {
            value: text.source.clone(),
        },
        PropertyValue::Vector(vector) => PropertyValueOutput::Vector {
            x: vector.x as f32,
            y: vector.y as f32,
            z: vector.z as f32,
        },
        PropertyValue::IntPoint(point) => PropertyValueOutput::IntPoint {
            x: point.x,
            y: point.y,
        },
        PropertyValue::ObjectRef(index) => PropertyValueOutput::ObjectRef {
            value: resolve_object_ref(package, *index),
        },
        PropertyValue::Guid(guid) => PropertyValueOutput::Guid {
            value: guid.to_string(),
        },
        PropertyValue::SoftObjectPath(path) => PropertyValueOutput::SoftObjectPath {
            value: path.clone(),
        },
        PropertyValue::Array(values) => PropertyValueOutput::Array {
            values: values
                .iter()
                .map(|value| value_output(package, value))
                .collect(),
        },
        PropertyValue::Set(values) => PropertyValueOutput::Set {
            values: values
                .iter()
                .map(|value| value_output(package, value))
                .collect(),
        },
        PropertyValue::Map(entries) => PropertyValueOutput::Map {
            entries: entries
                .iter()
                .map(|entry| MapEntryOutput {
                    key: value_output(package, &entry.key),
                    value: value_output(package, &entry.value),
                })
                .collect(),
        },
        PropertyValue::Struct(stream) => PropertyValueOutput::Struct {
            properties: stream
                .records
                .iter()
                .map(|record| PropertyOutput::from_record(record, package))
                .collect(),
        },
        PropertyValue::Raw { reason } => PropertyValueOutput::Raw {
            reason: render_raw_reason(reason),
            size: 0,
        },
        _ => PropertyValueOutput::Raw {
            reason: "unsupported property value".to_owned(),
            size: 0,
        },
    }
}

impl PropertyValueOutput {
    fn render(&self) -> String {
        match self {
            Self::Bool { value } => value.to_string(),
            Self::Int { value } => value.to_string(),
            Self::Uint { value } => value.to_string(),
            Self::Float { value } => value.to_string(),
            Self::Double { value } => value.to_string(),
            Self::Name { value } => value.clone(),
            Self::Enum { value } => value.clone(),
            Self::String { value } => format!("{value:?}"),
            Self::Text { value } => format!("{value:?}"),
            Self::Vector { x, y, z } => format!("({x}, {y}, {z})"),
            Self::IntPoint { x, y } => format!("({x}, {y})"),
            Self::ObjectRef { value } => value.clone().unwrap_or_else(|| "null".to_owned()),
            Self::Guid { value } => value.clone(),
            Self::SoftObjectPath { value } => {
                if value.is_empty() {
                    "<none>".to_owned()
                } else {
                    value.clone()
                }
            }
            Self::Array { values } => {
                let rendered: Vec<String> = values.iter().map(Self::render).collect();
                format!("[{}]", rendered.join(", "))
            }
            Self::Set { values } => {
                let rendered: Vec<String> = values.iter().map(Self::render).collect();
                format!("{{{}}}", rendered.join(", "))
            }
            Self::Map { entries } => {
                let rendered: Vec<String> = entries
                    .iter()
                    .map(|entry| format!("{} => {}", entry.key.render(), entry.value.render()))
                    .collect();
                format!("{{{}}}", rendered.join(", "))
            }
            Self::Struct { properties } => {
                let rendered: Vec<String> = properties
                    .iter()
                    .map(|property| format!("{} = {}", property.name, property.value.render()))
                    .collect();
                format!("{{{}}}", rendered.join(", "))
            }
            Self::Raw { reason, size } => format!("<raw {reason}, {size} bytes>"),
        }
    }
}

fn resolve_name_or_placeholder(package: &Package, name: uasset_parser::archive::NameRef) -> String {
    package
        .resolve_name(name)
        .unwrap_or_else(|| "<unresolved>".to_owned())
}

fn resolve_object_ref(package: &Package, index: PackageIndex) -> Option<String> {
    if index == PackageIndex::Null {
        None
    } else {
        package.resolve_index(index).map(|path| path.to_string())
    }
}

fn render_raw_reason(reason: &RawReason) -> String {
    match reason {
        RawReason::UnsupportedType => "unsupported type".to_owned(),
        RawReason::DecoderRejected(detail) => detail.clone(),
    }
}

#[derive(Serialize)]
struct VersionOutput {
    legacy_file: i32,
    legacy_ue3: Option<i32>,
    ue4: i32,
    ue5: i32,
    licensee: i32,
}

#[derive(Serialize)]
struct TableOutput {
    count: u32,
    offset: u64,
}

impl From<TableLocation> for TableOutput {
    fn from(table: TableLocation) -> Self {
        Self {
            count: table.count,
            offset: table.offset.get(),
        }
    }
}

#[derive(Serialize)]
struct ErrorOutput {
    schema_version: u32,
    status: &'static str,
    path: String,
    kind: &'static str,
    message: String,
    field: Option<String>,
    offset: Option<u64>,
}

#[cfg(feature = "utrace")]
#[derive(Serialize)]
struct UtraceErrorOutput {
    schema_version: u32,
    status: &'static str,
    path: String,
    kind: &'static str,
    message: String,
    field: Option<String>,
    offset: Option<u64>,
}

impl ErrorOutput {
    fn io(path: String, message: String) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            status: "error",
            path,
            kind: "io",
            message,
            field: None,
            offset: None,
        }
    }

    fn package(path: String, error: &PackageError) -> Self {
        let kind = match error.kind() {
            PackageErrorKind::MalformedData => "malformed_data",
            PackageErrorKind::ResourceLimit => "resource_limit",
            PackageErrorKind::UnsupportedFormat => "unsupported_format",
            PackageErrorKind::UnsupportedVersion => "unsupported_version",
            PackageErrorKind::UnsupportedCapability => "unsupported_capability",
        };
        Self {
            schema_version: SCHEMA_VERSION,
            status: "error",
            path,
            kind,
            message: error.detail().to_owned(),
            field: Some(error.path().to_owned()),
            offset: error.offset(),
        }
    }
}

#[cfg(feature = "utrace")]
impl UtraceErrorOutput {
    fn io(path: String, message: String) -> Self {
        Self {
            schema_version: UTRACE_SCHEMA_VERSION,
            status: "error",
            path,
            kind: "io",
            message,
            field: None,
            offset: None,
        }
    }

    fn trace(path: String, error: &TraceError) -> Self {
        let kind = match error.kind() {
            TraceErrorKind::MalformedData => "malformed_data",
            TraceErrorKind::ResourceLimit => "resource_limit",
            TraceErrorKind::UnsupportedFormat => "unsupported_format",
        };
        Self {
            schema_version: UTRACE_SCHEMA_VERSION,
            status: "error",
            path,
            kind,
            message: error.detail().to_owned(),
            field: Some(error.path().to_owned()),
            offset: Some(error.offset()),
        }
    }
}

const USAGE: &str = "Usage: uasset <inspect|authoring> <path|-> [--format text|json]";

const HELP: &str = "\
uasset - inspect classic Unreal Engine asset packages

Usage:
  uasset inspect <path|-> [--format text|json]
  uasset authoring <path|-> --format json
  uasset utrace inspect <path|-> [--format text|json]
  uasset utrace dashboard <path|-> [--format text|json] [--frame <n>] [--timeline-limit <n>] [--max-frames <n>] [--gpu-frame <n>] [--gpu-timeline-limit <n>] [--symbol-path <dir>]...
  uasset utrace inventory <path|-> [--format text|json]
  uasset utrace coverage <path|-> [--universe <file>] [--format text|json]
  uasset utrace html <path|-> [--output <file>]
  uasset utrace timeline index <path|-> --output <path> [--max-intervals <n>] [--format text|json]
  uasset utrace timeline query <index> [--start-cycle <n>] [--end-cycle <n>] [--thread <id>] [--search <text>] [--limit <n>] [--format text|json]
  uasset help
  uasset version

Commands:
  inspect    Parse one package summary. Use `-` to read package bytes from stdin.
  authoring  Emit the versioned Unreal authoring snapshot for one DataTable package.
  utrace     Inspect or summarize Unreal Trace (`.utrace`) files when built with `--features utrace`.

Output contract:
  stdout     Successful result only.
  stderr     Diagnostics and structured errors only.
  text       Human-readable output (default).
  json       Stable schema-versioned JSON.

Exit codes:
  0          Success
  2          Malformed package data
  3          Unsupported format, version, or capability
  4          Input/output failure
  5          Internal output failure
  6          Partial authoring or inspection result
  7          Parser resource limit exceeded
  64         Invalid command-line usage
";

const UTRACE_DASHBOARD_HELP: &str = "\
Usage: uasset utrace dashboard <path|-> [options]

Options:
  --format <text|json>        Output format (default: text).
  --frame <n>                 Retain a CPU metadata-frame timeline.
  --timeline-limit <n>        Max retained CPU timeline intervals (default: 500).
  --max-frames <n>            Max retained GPU/correlated frame rows (default: 120).
  --symbol-path <dir>         Local PDB search root (repeatable; requires utrace-symbols).
  --gpu-frame <n>             Retain a queue-local GPU-frame timeline.
  --gpu-timeline-limit <n>    Max retained GPU timeline intervals (default: 500).
";

const UTRACE_TIMELINE_HELP: &str = "\
Usage:
  uasset utrace timeline index <path|-> --output <path> [options]
  uasset utrace timeline query <index> [options]

Index options:
  --output <path>             Destination CPU timeline sidecar (required).
  --max-intervals <n>         Max CPU intervals indexed (default: 1000000).
  --format <text|json>        Output format (default: text).

Query options:
  --start-cycle <n>           Inclusive query start cycle.
  --end-cycle <n>             Inclusive query end cycle.
  --thread <id>               Keep one UE trace thread.
  --search <text>             Case-insensitive scope/rendered-name filter.
  --limit <n>                 Max returned intervals (default: 500; max: 10000).
  --format <text|json>        Output format (default: text).
";

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny_package() -> Package {
        let hex = include_str!("../../tests/fixtures/tiny/minimal-current-summary.uasset.hex");
        let compact = hex
            .lines()
            .filter(|line| !line.trim_start().starts_with('#'))
            .flat_map(str::chars)
            .filter(|ch| !ch.is_whitespace())
            .collect::<String>();
        let bytes = compact
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                u8::from_str_radix(std::str::from_utf8(pair).expect("ASCII hex"), 16)
                    .expect("valid hex fixture")
            })
            .collect::<Vec<_>>();
        Package::parse(&bytes).expect("tiny package")
    }

    fn empty_property_stream() -> uasset_parser::property::PropertyStream {
        uasset_parser::property::PropertyStream {
            class_extensions: None,
            records: Vec::new(),
            terminator: uasset_parser::Span::new(0, 0).expect("empty span"),
        }
    }

    #[test]
    fn maps_asset_output_and_renders_inspect_text() {
        let package = tiny_package();
        let asset = asset_output_from_decoded(
            &package,
            DecodedAsset::StringTable(uasset_parser::asset::DecodedStringTable {
                object_path: uasset_parser::package::ObjectPath::new("/Game/Test.ST_Test"),
                namespace: "TestNamespace".to_owned(),
                entries: vec![uasset_parser::asset::StringTableEntry {
                    key: "Greeting".to_owned(),
                    source: "Hello".to_owned(),
                }],
            }),
        );
        let json = serde_json::to_value(&asset).expect("serialize asset output");
        assert_eq!(json["kind"], "StringTable");
        assert_eq!(json["object_path"], "/Game/Test.ST_Test");
        assert_eq!(json["class_path"], "/Script/Engine.StringTable");
        assert_eq!(json["string_table_namespace"], "TestNamespace");
        assert_eq!(json["string_table_entries"][0]["key"], "Greeting");
        assert_eq!(json["string_table_entries"][0]["source"], "Hello");
        assert_eq!(json["row_count"], 0);
        assert!(json.get("properties").is_none());

        let mut output = InspectOutput::from_summary("test.uasset".to_owned(), &package.summary);
        output.assets.push(asset);
        output.decode_errors.push(DecodeErrorOutput {
            object_path: "/Game/Test.Broken".to_owned(),
            class_path: None,
            kind: "malformed_data",
            message: "bad tail".to_owned(),
        });
        let text = render_text_output(&output);
        assert!(text.contains("path: test.uasset"));
        assert!(text.contains("asset: StringTable /Game/Test.ST_Test rows=0"));
        assert!(text.contains("namespace: TestNamespace"));
        assert!(text.contains("  Greeting = Hello"));
        assert!(text.contains("decode_error: /Game/Test.Broken [malformed_data] bad tail"));
    }

    #[test]
    fn projects_data_tables_to_the_shared_authoring_contract() {
        use uasset_parser::asset::{DataTableKind, DecodedDataTable};
        use uasset_parser::package::ObjectPath;

        let package = tiny_package();
        let asset = asset_output_from_decoded(
            &package,
            DecodedAsset::DataTable(DecodedDataTable {
                kind: DataTableKind::Plain,
                object_path: ObjectPath::new("/Game/Test.DT_Test"),
                row_struct: Some(ObjectPath::new("/Script/Test.Row")),
                parent_tables: Vec::new(),
                properties: empty_property_stream(),
                rows: Vec::new(),
            }),
        );
        let inspect = InspectOutput::from_summary("test.uasset".to_owned(), &package.summary);
        let output = AuthoringSnapshotOutput::from_inspect(&inspect, &asset);
        let json = serde_json::to_value(output).expect("serialize authoring output");

        assert_eq!(json["contract"]["name"], "unreal-authoring");
        assert_eq!(json["contract"]["version"]["major"], 1);
        assert_eq!(json["authority"]["kind"], "project_files");
        assert_eq!(json["table"]["kind"], "data_table");
        assert_eq!(json["table"]["objectPath"], "/Game/Test.DT_Test");
        assert_eq!(json["table"]["rowStruct"], "/Script/Test.Row");
    }

    #[test]
    fn maps_every_decoded_asset_variant_to_a_stable_kind() {
        use uasset_parser::asset::{
            CurveTableMode, DataTableKind, DecodedCurveTable, DecodedDataAsset, DecodedDataTable,
            DecodedEnum, DecodedSkeleton, DecodedStruct, DecodedUObject,
        };
        use uasset_parser::package::ObjectPath;

        let package = tiny_package();
        let path = || ObjectPath::new("/Game/Test.Asset");
        let variants = vec![
            (
                DecodedAsset::DataTable(DecodedDataTable {
                    kind: DataTableKind::Plain,
                    object_path: path(),
                    row_struct: None,
                    parent_tables: Vec::new(),
                    properties: empty_property_stream(),
                    rows: Vec::new(),
                }),
                "DataTable",
            ),
            (
                DecodedAsset::CurveTable(DecodedCurveTable {
                    object_path: path(),
                    mode: CurveTableMode::Empty,
                    properties: empty_property_stream(),
                    rows: Vec::new(),
                }),
                "CurveTable",
            ),
            (
                DecodedAsset::DataAsset(DecodedDataAsset {
                    object_path: path(),
                    class_path: ObjectPath::new(DATA_ASSET_CLASS),
                    object_guid: None,
                    properties: empty_property_stream(),
                }),
                "DataAsset",
            ),
            (
                DecodedAsset::UObject(DecodedUObject {
                    object_path: path(),
                    class_path: ObjectPath::new("/Script/CoreUObject.Object"),
                    object_guid: None,
                    properties: empty_property_stream(),
                    tail: uasset_parser::Span::new(0, 0).expect("empty tail"),
                }),
                "UObject",
            ),
            (
                DecodedAsset::Enum(DecodedEnum {
                    object_path: path(),
                    cpp_form: EnumCppForm::Regular,
                    properties: empty_property_stream(),
                    entries: Vec::new(),
                }),
                "Enum",
            ),
            (
                DecodedAsset::Struct(DecodedStruct {
                    object_path: path(),
                    struct_flags: 0,
                    properties: empty_property_stream(),
                    fields: Vec::new(),
                    default_values: empty_property_stream(),
                }),
                "Struct",
            ),
            (
                DecodedAsset::Skeleton(DecodedSkeleton {
                    object_path: path(),
                    object_guid: None,
                    properties: empty_property_stream(),
                    bones: Vec::new(),
                }),
                "Skeleton",
            ),
        ];

        for (decoded, expected_kind) in variants {
            let output = asset_output_from_decoded(&package, decoded);
            assert_eq!(output.kind, expected_kind);
            assert_eq!(output.object_path, "/Game/Test.Asset");
        }
    }

    #[test]
    fn parses_inspect_contract() {
        assert_eq!(
            Command::parse(vec![
                "inspect".into(),
                "asset.uasset".into(),
                "--format=json".into()
            ])
            .unwrap(),
            Command::Inspect(InspectOptions {
                input: Input::File(PathBuf::from("asset.uasset")),
                format: OutputFormat::Json,
                timeline_frame: None,
                timeline_limit: 500,
                max_frames: 120,
                gpu_timeline_frame: None,
                gpu_timeline_limit: 500,
                timeline_index_output: None,
                timeline_index_max_intervals: utrace_parser::utrace::DEFAULT_MAX_INDEXED_INTERVALS,
                symbol_paths: Vec::new(),
            })
        );
    }

    #[test]
    fn parses_authoring_contract() {
        let command = Command::parse(vec![
            "authoring".into(),
            "table.uasset".into(),
            "--format".into(),
            "json".into(),
        ])
        .expect("authoring command");
        assert!(matches!(command, Command::Authoring(_)));
    }

    #[test]
    fn parses_stdin_contract() {
        assert_eq!(
            Command::parse(vec!["inspect".into(), "-".into()]).unwrap(),
            Command::Inspect(InspectOptions {
                input: Input::Stdin,
                format: OutputFormat::Text,
                timeline_frame: None,
                timeline_limit: 500,
                max_frames: 120,
                gpu_timeline_frame: None,
                gpu_timeline_limit: 500,
                timeline_index_output: None,
                timeline_index_max_intervals: utrace_parser::utrace::DEFAULT_MAX_INDEXED_INTERVALS,
                symbol_paths: Vec::new(),
            })
        );
    }

    #[test]
    fn rejects_multiple_inputs() {
        let error = Command::parse(vec!["inspect".into(), "one".into(), "two".into()]).unwrap_err();
        assert_eq!(error, "inspect accepts exactly one input");
    }

    #[test]
    fn parses_utrace_inspect_contract() {
        assert_eq!(
            Command::parse(vec![
                "utrace".into(),
                "inspect".into(),
                "trace.utrace".into(),
                "--format=json".into()
            ])
            .unwrap(),
            Command::Utrace(UtraceCommand::Inspect(InspectOptions {
                input: Input::File(PathBuf::from("trace.utrace")),
                format: OutputFormat::Json,
                timeline_frame: None,
                timeline_limit: 500,
                max_frames: 120,
                gpu_timeline_frame: None,
                gpu_timeline_limit: 500,
                timeline_index_output: None,
                timeline_index_max_intervals: utrace_parser::utrace::DEFAULT_MAX_INDEXED_INTERVALS,
                symbol_paths: Vec::new(),
            }))
        );
    }

    #[test]
    fn parses_utrace_dashboard_frame_option() {
        assert_eq!(
            Command::parse(vec![
                "utrace".into(),
                "dashboard".into(),
                "trace.utrace".into(),
                "--frame=366400".into(),
                "--format=json".into()
            ])
            .unwrap(),
            Command::Utrace(UtraceCommand::Dashboard(InspectOptions {
                input: Input::File(PathBuf::from("trace.utrace")),
                format: OutputFormat::Json,
                timeline_frame: Some(366400),
                timeline_limit: 500,
                max_frames: 120,
                gpu_timeline_frame: None,
                gpu_timeline_limit: 500,
                timeline_index_output: None,
                timeline_index_max_intervals: utrace_parser::utrace::DEFAULT_MAX_INDEXED_INTERVALS,
                symbol_paths: Vec::new(),
            }))
        );
    }

    #[test]
    fn parses_utrace_dashboard_bounds_and_gpu_timeline_options() {
        assert_eq!(
            Command::parse(vec![
                "utrace".into(),
                "dashboard".into(),
                "trace.utrace".into(),
                "--timeline-limit=25".into(),
                "--max-frames".into(),
                "10".into(),
                "--gpu-frame=42".into(),
                "--gpu-timeline-limit".into(),
                "5".into(),
            ])
            .unwrap(),
            Command::Utrace(UtraceCommand::Dashboard(InspectOptions {
                input: Input::File(PathBuf::from("trace.utrace")),
                format: OutputFormat::Text,
                timeline_frame: None,
                timeline_limit: 25,
                max_frames: 10,
                gpu_timeline_frame: Some(42),
                gpu_timeline_limit: 5,
                timeline_index_output: None,
                timeline_index_max_intervals: utrace_parser::utrace::DEFAULT_MAX_INDEXED_INTERVALS,
                symbol_paths: Vec::new(),
            }))
        );
    }

    #[test]
    fn parses_progressive_timeline_index_options() {
        let command = Command::parse(vec![
            "utrace".into(),
            "dashboard-progress".into(),
            "trace.utrace".into(),
            "--format=json".into(),
            "--timeline-index-output=trace.utix".into(),
            "--timeline-index-max-intervals=17".into(),
        ])
        .unwrap();
        let Command::Utrace(UtraceCommand::DashboardProgress(options)) = command else {
            panic!("expected dashboard-progress command");
        };
        assert_eq!(
            options.timeline_index_output,
            Some(PathBuf::from("trace.utix"))
        );
        assert_eq!(options.timeline_index_max_intervals, 17);
    }

    #[test]
    fn parses_repeatable_utrace_symbol_paths() {
        let command = Command::parse(vec![
            "utrace".into(),
            "dashboard".into(),
            "trace.utrace".into(),
            "--symbol-path".into(),
            "symbols/one".into(),
            "--symbol-path=symbols/two".into(),
        ])
        .unwrap();

        let Command::Utrace(UtraceCommand::Dashboard(options)) = command else {
            panic!("expected dashboard command");
        };
        assert_eq!(
            options.symbol_paths,
            vec![PathBuf::from("symbols/one"), PathBuf::from("symbols/two")]
        );
    }

    #[test]
    fn parses_utrace_dashboard_help() {
        assert_eq!(
            Command::parse(vec!["utrace".into(), "dashboard".into(), "--help".into()]).unwrap(),
            Command::Utrace(UtraceCommand::DashboardHelp)
        );
    }

    #[test]
    fn parses_utrace_dashboard_contract() {
        assert_eq!(
            Command::parse(vec![
                "utrace".into(),
                "dashboard".into(),
                "trace.utrace".into(),
                "--format=json".into()
            ])
            .unwrap(),
            Command::Utrace(UtraceCommand::Dashboard(InspectOptions {
                input: Input::File(PathBuf::from("trace.utrace")),
                format: OutputFormat::Json,
                timeline_frame: None,
                timeline_limit: 500,
                max_frames: 120,
                gpu_timeline_frame: None,
                gpu_timeline_limit: 500,
                timeline_index_output: None,
                timeline_index_max_intervals: utrace_parser::utrace::DEFAULT_MAX_INDEXED_INTERVALS,
                symbol_paths: Vec::new(),
            }))
        );
    }

    #[test]
    fn parses_utrace_timeline_index_and_query_contracts() {
        assert_eq!(
            Command::parse(vec![
                "utrace".into(),
                "timeline".into(),
                "index".into(),
                "trace.utrace".into(),
                "--output".into(),
                "trace.utix".into(),
                "--max-intervals=400".into(),
                "--format=json".into(),
            ])
            .unwrap(),
            Command::Utrace(UtraceCommand::Timeline(TimelineCommand::Index(
                TimelineIndexOptions {
                    input: Input::File(PathBuf::from("trace.utrace")),
                    output: PathBuf::from("trace.utix"),
                    format: OutputFormat::Json,
                    max_intervals: 400,
                },
            )))
        );
        assert_eq!(
            Command::parse(vec![
                "utrace".into(),
                "timeline".into(),
                "query".into(),
                "trace.utix".into(),
                "--start-cycle=10".into(),
                "--end-cycle".into(),
                "20".into(),
                "--thread=7".into(),
                "--search".into(),
                "render".into(),
                "--limit=42".into(),
            ])
            .unwrap(),
            Command::Utrace(UtraceCommand::Timeline(TimelineCommand::Query(
                TimelineQueryOptions {
                    index: PathBuf::from("trace.utix"),
                    format: OutputFormat::Text,
                    start_cycle: Some(10),
                    end_cycle: Some(20),
                    thread_id: Some(7),
                    search: Some("render".to_owned()),
                    limit: 42,
                },
            )))
        );
    }

    #[test]
    fn parses_utrace_inventory_contract() {
        assert_eq!(
            Command::parse(vec![
                "utrace".into(),
                "inventory".into(),
                "trace.utrace".into(),
                "--format=json".into()
            ])
            .unwrap(),
            Command::Utrace(UtraceCommand::Inventory(InspectOptions {
                input: Input::File(PathBuf::from("trace.utrace")),
                format: OutputFormat::Json,
                timeline_frame: None,
                timeline_limit: 500,
                max_frames: 120,
                gpu_timeline_frame: None,
                gpu_timeline_limit: 500,
                timeline_index_output: None,
                timeline_index_max_intervals: utrace_parser::utrace::DEFAULT_MAX_INDEXED_INTERVALS,
                symbol_paths: Vec::new(),
            }))
        );
    }

    #[test]
    fn parses_utrace_coverage_contract() {
        assert_eq!(
            Command::parse(vec![
                "utrace".into(),
                "coverage".into(),
                "trace.utrace".into(),
                "--universe=events.txt".into(),
                "--format=json".into()
            ])
            .unwrap(),
            Command::Utrace(UtraceCommand::Coverage(CoverageOptions {
                input: Input::File(PathBuf::from("trace.utrace")),
                format: OutputFormat::Json,
                universe: Some(PathBuf::from("events.txt")),
            }))
        );
    }

    #[test]
    fn parses_utrace_html_contract() {
        assert_eq!(
            Command::parse(vec![
                "utrace".into(),
                "html".into(),
                "trace.utrace".into(),
                "--output=trace.html".into()
            ])
            .unwrap(),
            Command::Utrace(UtraceCommand::Html(UtraceHtmlOptions {
                input: Input::File(PathBuf::from("trace.utrace")),
                output: Some(PathBuf::from("trace.html")),
            }))
        );
    }

    #[test]
    fn rejects_invalid_utrace_coverage_contracts() {
        assert_eq!(
            Command::parse(vec!["utrace".into(), "coverage".into()]).unwrap_err(),
            "coverage requires a file path or `-`"
        );
        assert_eq!(
            Command::parse(vec![
                "utrace".into(),
                "coverage".into(),
                "--universe".into(),
                "trace.utrace".into()
            ])
            .unwrap_err(),
            "coverage requires a file path or `-`"
        );
        assert_eq!(
            Command::parse(vec![
                "utrace".into(),
                "coverage".into(),
                "trace.utrace".into(),
                "--bad".into()
            ])
            .unwrap_err(),
            "unknown coverage option \"--bad\""
        );
    }
}
