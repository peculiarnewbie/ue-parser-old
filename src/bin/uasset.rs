use std::env;
use std::ffi::OsString;
use std::fmt::Write as _;
use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use serde::Serialize;
use uasset_parser::asset::{
    AssetDecodeContext, AssetError, AssetErrorKind, DecodedAsset, EnumCppForm, decode_export,
};
use uasset_parser::asset::{DATA_ASSET_CLASS, PRIMARY_DATA_ASSET_CLASS, USERDEFINEDENUM_CLASS};
use uasset_parser::package::{PackageError, PackageErrorKind, PackageIndex, TableLocation};
use uasset_parser::property::{PropertyRecord, PropertyValue, RawReason};
use uasset_parser::schema::{ClassSchema, SchemaProvider, StructSchema};
use uasset_parser::{Package, PackageSummary};

const SCHEMA_VERSION: u32 = 3;
const EXIT_SUCCESS: u8 = 0;
const EXIT_MALFORMED: u8 = 2;
const EXIT_UNSUPPORTED: u8 = 3;
const EXIT_IO: u8 = 4;
const EXIT_INTERNAL: u8 = 5;
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
    Help,
    Version,
}

impl Command {
    fn parse(arguments: Vec<OsString>) -> Result<Self, String> {
        let mut arguments = arguments.into_iter();
        let Some(command) = arguments.next() else {
            return Err("missing command".to_owned());
        };
        match command.to_str() {
            Some("inspect") => Self::parse_inspect(arguments.collect()),
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
        }))
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

    let output = match InspectOutput::from_package(input_name.clone(), &bytes, &package) {
        Ok(output) => output,
        Err(error) => {
            let exit_code = exit_code_for_asset_error(&error);
            write_error(options.format, ErrorOutput::asset(input_name, &error));
            return exit_code;
        }
    };
    let rendered = match render_output(options.format, &output) {
        Ok(rendered) => rendered,
        Err(error) => {
            eprintln!("uasset: failed to serialize output: {error}");
            return EXIT_INTERNAL;
        }
    };
    write_stdout(&rendered)
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

fn exit_code_for_package_error(error: &PackageError) -> u8 {
    match error.kind() {
        PackageErrorKind::MalformedData => EXIT_MALFORMED,
        PackageErrorKind::UnsupportedFormat
        | PackageErrorKind::UnsupportedVersion
        | PackageErrorKind::UnsupportedCapability => EXIT_UNSUPPORTED,
    }
}

fn exit_code_for_asset_error(error: &AssetError) -> u8 {
    match error.kind() {
        AssetErrorKind::MalformedData => EXIT_MALFORMED,
        AssetErrorKind::UnsupportedFormat
        | AssetErrorKind::UnsupportedVersion
        | AssetErrorKind::UnsupportedCapability => EXIT_UNSUPPORTED,
    }
}

#[derive(Serialize)]
struct InspectOutput {
    schema_version: u32,
    status: &'static str,
    path: String,
    package: PackageOutput,
    assets: Vec<AssetOutput>,
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
        }
    }

    fn from_package(path: String, source: &[u8], package: &Package) -> Result<Self, AssetError> {
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
                Err(error) if error.kind() == AssetErrorKind::UnsupportedCapability => {}
                Err(error) => return Err(error),
            }
        }
        Ok(output)
    }
}

fn asset_output_from_decoded(package: &Package, decoded: DecodedAsset) -> AssetOutput {
    match decoded {
        DecodedAsset::DataTable(datatable) => AssetOutput {
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
            properties: Vec::new(),
            row_count: 0,
            curve_rows: Vec::new(),
            rows: Vec::new(),
        },
        DecodedAsset::DataAsset(data_asset) => AssetOutput {
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
            properties: property_outputs(package, &object.properties),
            row_count: 0,
            curve_rows: Vec::new(),
            rows: Vec::new(),
        },
        DecodedAsset::Enum(decoded_enum) => AssetOutput {
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
            properties: Vec::new(),
            row_count: decoded_enum.entries.len(),
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    properties: Vec<PropertyOutput>,
    row_count: usize,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    curve_rows: Vec<CurveRowOutput>,
    rows: Vec<RowOutput>,
}

#[derive(Serialize)]
struct EnumEntryOutput {
    name: String,
    value: i64,
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
                x: vector.x,
                y: vector.y,
                z: vector.z,
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
            x: vector.x,
            y: vector.y,
            z: vector.z,
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

    fn asset(path: String, error: &AssetError) -> Self {
        let kind = match error.kind() {
            AssetErrorKind::MalformedData => "malformed_data",
            AssetErrorKind::UnsupportedFormat => "unsupported_format",
            AssetErrorKind::UnsupportedVersion => "unsupported_version",
            AssetErrorKind::UnsupportedCapability => "unsupported_capability",
        };
        Self {
            schema_version: SCHEMA_VERSION,
            status: "error",
            path,
            kind,
            message: error.message().to_owned(),
            field: None,
            offset: None,
        }
    }
}

const USAGE: &str = "Usage: uasset inspect <path|-> [--format text|json]";

const HELP: &str = "\
uasset - inspect classic Unreal Engine asset packages

Usage:
  uasset inspect <path|-> [--format text|json]
  uasset help
  uasset version

Commands:
  inspect    Parse one package summary. Use `-` to read package bytes from stdin.

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
  64         Invalid command-line usage
";

#[cfg(test)]
mod tests {
    use super::*;

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
            })
        );
    }

    #[test]
    fn parses_stdin_contract() {
        assert_eq!(
            Command::parse(vec!["inspect".into(), "-".into()]).unwrap(),
            Command::Inspect(InspectOptions {
                input: Input::Stdin,
                format: OutputFormat::Text,
            })
        );
    }

    #[test]
    fn rejects_multiple_inputs() {
        let error = Command::parse(vec!["inspect".into(), "one".into(), "two".into()]).unwrap_err();
        assert_eq!(error, "inspect accepts exactly one input");
    }
}
