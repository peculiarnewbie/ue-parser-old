#![cfg(feature = "utrace")]

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_uasset"))
}

fn fixture() -> PathBuf {
    if let Some(path) = std::env::var_os("UTRACE_FIXTURE").map(PathBuf::from) {
        return require_file_or_skip(path, "UTRACE_FIXTURE");
    }

    if let Some(dir) = std::env::var_os("UTRACE_FIXTURE_DIR").map(PathBuf::from) {
        if let Some(path) = first_utrace_in(&dir) {
            return path;
        }
        missing_fixture(format!(
            "UTRACE_FIXTURE_DIR did not contain a .utrace file: {}",
            dir.display()
        ));
    }

    let default_dir = PathBuf::from("D:/Perforce/Arif_Fixtures/Traces");
    if let Some(path) = first_utrace_in(&default_dir) {
        return path;
    }

    missing_fixture(
        "set UTRACE_FIXTURE to a .utrace file or UTRACE_FIXTURE_DIR to a directory containing one"
            .to_owned(),
    )
}

fn require_file_or_skip(path: PathBuf, source: &str) -> PathBuf {
    if path.is_file() {
        path
    } else {
        missing_fixture(format!(
            "{source} does not point to a file: {}",
            path.display()
        ))
    }
}

fn first_utrace_in(dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut traces = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("utrace"))
        })
        .collect::<Vec<_>>();
    traces.sort();
    traces.into_iter().next()
}

fn targeted_fixtures() -> Vec<PathBuf> {
    if let Some(path) = std::env::var_os("UTRACE_TARGETED_FIXTURE").map(PathBuf::from) {
        return vec![require_file_or_skip(path, "UTRACE_TARGETED_FIXTURE")];
    }
    if let Some(dir) = std::env::var_os("UTRACE_TARGETED_FIXTURE_DIR").map(PathBuf::from) {
        let mut traces = std::fs::read_dir(&dir)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", dir.display()))
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file()
                    && path
                        .extension()
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("utrace"))
            })
            .collect::<Vec<_>>();
        traces.sort();
        if !traces.is_empty() {
            return traces;
        }
        missing_fixture(format!(
            "UTRACE_TARGETED_FIXTURE_DIR did not contain a .utrace file: {}",
            dir.display()
        ));
    }
    let studio_capture =
        PathBuf::from("D:/Perforce/Arif_Fixtures/Traces/targeted-providers.utrace");
    if studio_capture.is_file() {
        return vec![studio_capture];
    }
    missing_fixture(
        "set UTRACE_TARGETED_FIXTURE or UTRACE_TARGETED_FIXTURE_DIR to targeted provider captures"
            .to_owned(),
    )
}

fn iostore_fixture() -> PathBuf {
    std::env::var_os("UTRACE_IOSTORE_FIXTURE")
        .map(PathBuf::from)
        .map(|path| require_file_or_skip(path, "UTRACE_IOSTORE_FIXTURE"))
        .unwrap_or_else(|| {
            missing_fixture(
                "set UTRACE_IOSTORE_FIXTURE to a cooked trace with IoStore request traffic"
                    .to_owned(),
            )
        })
}

fn missing_fixture(message: String) -> ! {
    panic!("{message}");
}

#[test]
#[ignore = "requires a real .utrace fixture; set UTRACE_FIXTURE or UTRACE_FIXTURE_DIR"]
fn real_utrace_fixture_exposes_header_prologue_threads_and_registry() {
    let fixture = fixture();

    let output = binary()
        .args([
            "utrace",
            "inspect",
            fixture.to_str().unwrap(),
            "--format=json",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    assert!(
        output.stderr.is_empty(),
        "stderr was not empty: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["schema_version"], 2);
    assert_eq!(json["status"], "ok");
    assert_eq!(json["trace"]["header"]["transport"], 4);
    assert!(
        json["trace"]["header"]["protocol"].as_u64().unwrap() <= 7,
        "protocol should be supported"
    );
    assert!(
        json["trace"]["packets"]["count"].as_u64().unwrap() > 0,
        "fixture should contain packets"
    );

    let events = json["trace"]["events"].as_array().unwrap();
    assert!(
        events
            .iter()
            .any(|event| event["logger"] == "$Trace" && event["event"] == "NewTrace"),
        "fixture should declare $Trace.NewTrace"
    );
    assert!(
        events
            .iter()
            .any(|event| event["logger"] == "$Trace" && event["event"] == "ThreadInfo"),
        "fixture should declare $Trace.ThreadInfo"
    );

    let prologue = &json["trace"]["prologue"];
    assert!(
        prologue["cycle_frequency"].as_u64().unwrap() > 0,
        "cycle frequency should be positive"
    );
    assert!(
        matches!(prologue["pointer_size"].as_u64(), Some(4 | 8)),
        "pointer size should be 4 or 8"
    );

    let threads = json["trace"]["thread_info"].as_array().unwrap();
    assert!(!threads.is_empty(), "fixture should contain thread info");
    assert!(
        threads
            .iter()
            .any(|thread| thread["name"].as_str().is_some_and(|name| !name.is_empty())),
        "at least one thread should have a non-empty name"
    );
}

#[test]
#[ignore = "requires a real .utrace fixture; set UTRACE_FIXTURE or UTRACE_FIXTURE_DIR"]
fn real_utrace_fixture_exposes_cpu_dashboard_summary() {
    let fixture = fixture();

    let output = binary()
        .args([
            "utrace",
            "dashboard",
            fixture.to_str().unwrap(),
            "--format=json",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    assert!(
        output.stderr.is_empty(),
        "stderr was not empty: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["schema_version"], 2);
    assert_eq!(json["status"], "ok");
    assert!(
        json["dashboard"]["cpu"]["specs"].as_array().unwrap().len() > 100,
        "fixture should expose CPU event specs"
    );
    assert!(
        json["dashboard"]["cpu"]["batches"]["count"]
            .as_u64()
            .unwrap()
            > 0,
        "fixture should expose CPU event batches"
    );
    assert!(
        json["dashboard"]["cpu"]["batches"]["intervals"]
            .as_u64()
            .unwrap()
            > 0,
        "fixture should expose closed CPU scope intervals"
    );
    assert!(
        json["dashboard"]["cpu"]["batches"]["metadata_scopes"]
            .as_u64()
            .unwrap()
            > 0,
        "fixture should expose CPU metadata scope records"
    );
    let cpu_metadata = &json["dashboard"]["cpu"]["metadata"];
    assert!(
        cpu_metadata["specs"].as_u64().unwrap() > 0,
        "fixture should expose CPU metadata specs"
    );
    assert!(
        cpu_metadata["records"].as_u64().unwrap() > 0,
        "fixture should expose CPU metadata records"
    );
    assert!(
        cpu_metadata["metadata_bytes"].as_u64().unwrap() > 0,
        "fixture should count CPU metadata payload bytes"
    );
    assert!(
        cpu_metadata["decoded_records"].as_u64().unwrap() > 0,
        "fixture should decode CPU metadata payload records"
    );
    assert!(
        cpu_metadata["decoded_values"].as_u64().unwrap() > 0,
        "fixture should decode CPU metadata payload values"
    );
    assert_eq!(
        cpu_metadata["metadata_bytes"].as_u64().unwrap(),
        cpu_metadata["decoded_metadata_bytes"].as_u64().unwrap()
            + cpu_metadata["undecoded_metadata_bytes"].as_u64().unwrap(),
        "CPU metadata byte accounting should balance"
    );
    assert!(
        cpu_metadata["samples"]
            .as_array()
            .unwrap()
            .iter()
            .any(|sample| sample["fields"]
                .as_object()
                .is_some_and(|fields| !fields.is_empty())),
        "fixture should expose representative decoded CPU metadata samples"
    );
    assert!(
        cpu_metadata["spec_summaries"]
            .as_array()
            .unwrap()
            .iter()
            .any(
                |summary| summary["records"].as_u64().is_some_and(|count| count > 0)
                    && summary["decoded_values"]
                        .as_u64()
                        .is_some_and(|count| count > 0)
                    && summary["sample"]["fields"]
                        .as_object()
                        .is_some_and(|fields| !fields.is_empty())
            ),
        "fixture should expose representative decoded CPU metadata summaries per spec"
    );
    assert!(
        cpu_metadata["spec_summaries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|summary| summary["rendered_names"]
                .as_array()
                .is_some_and(|names| names
                    .iter()
                    .any(|name| name.as_str().is_some_and(|name| name.starts_with("Frame "))))),
        "fixture should render representative CPU metadata names from NameFormat"
    );
    assert!(
        cpu_metadata["rendered_scopes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|scope| scope["rendered_name"]
                .as_str()
                .is_some_and(|name| name.contains("RenderGraphExecute"))
                && scope["count"].as_u64().is_some_and(|count| count > 0)
                && scope["total_cycles"]
                    .as_u64()
                    .is_some_and(|cycles| cycles > 0)),
        "fixture should aggregate CPU metadata scopes by rendered name"
    );
    assert!(
        cpu_metadata["interval_samples"]
            .as_array()
            .unwrap()
            .iter()
            .any(|sample| sample["rendered_name"]
                .as_str()
                .is_some_and(|name| !name.is_empty())
                && sample["duration_cycles"]
                    .as_u64()
                    .is_some_and(|duration| duration > 0)
                && sample["attribution"].as_str() == Some("inline")),
        "fixture should expose bounded CPU metadata interval samples"
    );
    assert!(
        json["dashboard"]["cpu"]["batches"]["restored_metadata_scopes"]
            .as_u64()
            .is_some(),
        "fixture dashboard should expose restored metadata scope accounting"
    );
    assert!(
        cpu_metadata["field_names"]
            .as_array()
            .unwrap()
            .iter()
            .any(|name| name.as_str() == Some("Frame Number")),
        "fixture should decode CPU metadata field-name payloads"
    );
    assert!(
        cpu_metadata["resolved_scopes"].as_u64().unwrap() > 0,
        "fixture should resolve CPU metadata-backed scopes"
    );
    assert_eq!(
        cpu_metadata["scopes"].as_u64().unwrap(),
        cpu_metadata["resolved_scopes"].as_u64().unwrap()
            + cpu_metadata["unresolved_scopes"].as_u64().unwrap(),
        "CPU metadata scope summary should balance"
    );
    assert!(
        cpu_metadata["top"]
            .as_array()
            .unwrap()
            .iter()
            .any(
                |scope| scope["name"].as_str().is_some_and(|name| !name.is_empty())
                    && scope["count"].as_u64().is_some_and(|count| count > 0)
            ),
        "fixture should expose named CPU metadata scope summaries"
    );
    let named_cpu_events = json["dashboard"]["cpu"]["named_events"].as_array().unwrap();
    assert!(
        named_cpu_events
            .iter()
            .any(|event| event["event"] == "Frame"
                && event["observed_count"]
                    .as_u64()
                    .is_some_and(|count| count > 0)
                && event["sample"]["fields"]["Name"].as_str().is_some()),
        "fixture should expose generic named Cpu.Frame events"
    );
    assert!(
        named_cpu_events
            .iter()
            .any(|event| event["event"] == "FRDGBufferPool_CreateBuffer"
                && event["sample"]["fields"]["SizeInBytes"].as_u64().is_some()),
        "fixture should expose scalar fields on generic Cpu.* events"
    );
    assert!(
        json["dashboard"]["cpu"]["scopes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|scope| scope["name"].as_str().is_some_and(|name| !name.is_empty())),
        "fixture should expose named CPU scope summaries"
    );
    // Global inclusive totals must stay within capture_span * thread_count.
    // thread_count is a documented fudge: inclusive time is summed across
    // threads, so a busy scope can exceed wall capture span, but not by
    // orders of magnitude (the pre-fix bug was ~442× on a single thread).
    {
        let prologue = &json["dashboard"]["prologue"];
        let freq = prologue["cycle_frequency"].as_u64().unwrap() as f64;
        let frames = json["dashboard"]["frames"].as_array().unwrap();
        let begin_cycles: Vec<u64> = frames
            .iter()
            .filter(|frame| frame["kind"] == "begin")
            .filter_map(|frame| frame["cycle"].as_u64())
            .collect();
        let end_cycles: Vec<u64> = frames
            .iter()
            .filter(|frame| frame["kind"] == "end")
            .filter_map(|frame| frame["cycle"].as_u64())
            .collect();
        assert!(
            !begin_cycles.is_empty() && !end_cycles.is_empty(),
            "fixture should expose begin/end frame markers for capture span"
        );
        let capture_span = (end_cycles.iter().copied().max().unwrap()
            - begin_cycles.iter().copied().min().unwrap()) as f64
            / freq;
        let thread_count = json["dashboard"]["cpu"]["threads"]
            .as_array()
            .unwrap()
            .len()
            .max(1) as f64;
        let limit = capture_span * thread_count;
        for scope in json["dashboard"]["cpu"]["scopes"]
            .as_array()
            .unwrap()
            .iter()
            .take(20)
        {
            let total = scope["total_seconds"].as_f64().unwrap();
            assert!(
                total <= limit,
                "top scope {} total_seconds {total} exceeds capture_span ({capture_span}) * thread_count ({thread_count})",
                scope["name"]
            );
        }
    }
    assert_eq!(
        json["dashboard"]["unmodeled"]["event_types"]
            .as_u64()
            .unwrap(),
        0,
        "current fixture has no raw declared families, but the unmodeled dashboard should be present"
    );
    let threads = json["dashboard"]["cpu"]["threads"].as_array().unwrap();
    assert!(
        !threads.is_empty(),
        "fixture should expose per-thread CPU summaries"
    );
    assert!(
        threads
            .iter()
            .any(|thread| thread["name"].as_str().is_some_and(|name| !name.is_empty())),
        "fixture should expose named CPU threads"
    );
    assert!(
        threads.iter().any(|thread| thread["scopes"]
            .as_array()
            .is_some_and(|scopes| !scopes.is_empty())),
        "fixture should expose per-thread CPU scopes"
    );
    let counters = &json["dashboard"]["counters"];
    assert!(
        counters["specs"].as_u64().unwrap() > 0,
        "fixture should expose counter specs"
    );
    assert_eq!(
        counters["samples"].as_u64().unwrap(),
        counters["int_samples"].as_u64().unwrap() + counters["float_samples"].as_u64().unwrap(),
        "counter sample summary should balance"
    );
    assert!(
        counters["counters"]
            .as_array()
            .unwrap()
            .iter()
            .any(|counter| counter["name"]
                .as_str()
                .is_some_and(|name| !name.is_empty())
                && counter["kind"].as_str().is_some()
                && counter["display_hint"].as_str().is_some()),
        "fixture should expose named counter specs"
    );
    assert!(
        counters["counters"]
            .as_array()
            .unwrap()
            .iter()
            .any(|counter| counter["display_hint"] == "memory"),
        "fixture should expose counter display hints"
    );
    let stats = &json["dashboard"]["stats"];
    assert!(
        stats["specs"].as_u64().unwrap() > 100,
        "fixture should expose stat specs"
    );
    assert_eq!(
        stats["sample_events"].as_u64().unwrap(),
        0,
        "current fixture should make absent stat sample timelines explicit"
    );
    assert!(
        stats["memory_specs"].as_u64().unwrap() > 0,
        "fixture should expose memory stat flags"
    );
    assert!(
        stats["groups"]
            .as_array()
            .unwrap()
            .iter()
            .any(
                |group| group["name"].as_str().is_some_and(|name| !name.is_empty())
                    && group["specs"].as_u64().is_some_and(|count| count > 0)
            ),
        "fixture should expose stat groups"
    );
    assert!(
        stats["stats"]
            .as_array()
            .unwrap()
            .iter()
            .any(|stat| stat["name"].as_str() == Some("STAT_TotalLLM")
                && stat["description"]
                    .as_str()
                    .is_some_and(|name| !name.is_empty())
                && stat["group"].as_str() == Some("STATGROUP_LLMFULL")),
        "fixture should decode stat spec strings and groups"
    );
    let csv = &json["dashboard"]["csv"];
    assert!(
        csv["categories"].as_u64().unwrap() > 0,
        "fixture should expose CSV profiler categories"
    );
    assert!(
        csv["stats"].as_u64().unwrap() > 0,
        "fixture should expose CSV profiler stat definitions"
    );
    assert_eq!(
        csv["sample_events"].as_u64().unwrap(),
        0,
        "current fixture should make absent CSV sample timelines explicit"
    );
    assert!(
        csv["top_categories"]
            .as_array()
            .unwrap()
            .iter()
            .any(
                |category| category["name"].as_str() == Some("IoDispatcherFileBackend")
                    && category["stats"].as_u64().is_some_and(|count| count > 0)
            ),
        "fixture should resolve CSV stat definitions to categories"
    );
    assert!(
        csv["stat_defs"]
            .as_array()
            .unwrap()
            .iter()
            .any(
                |stat| stat["name"].as_str() == Some("FrameBytesScatteredKB")
                    && stat["category"].as_str() == Some("IoDispatcherFileBackend")
                    && stat["kind"].as_str() == Some("declared")
            ),
        "fixture should decode declared CSV stats"
    );
    let loading = &json["dashboard"]["loading"];
    assert!(
        loading["class_count"].as_u64().unwrap() > 0,
        "fixture should expose load-time class declarations"
    );
    assert!(
        loading["classes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|class| class["name"].as_str() == Some("Object")
                && class["class"].as_u64().is_some()),
        "fixture should decode LoadTime.ClassInfo names and class pointers"
    );
    assert!(
        loading["package_count"].as_u64().is_some(),
        "fixture dashboard should expose LoadTime package summary fields"
    );
    assert!(
        loading["requests"]["begun"].as_u64().is_some(),
        "fixture dashboard should expose LoadTime request lifecycle counters"
    );
    let io_store = &json["dashboard"]["io_store"];
    assert_eq!(
        io_store["requests_created"].as_u64().unwrap(),
        0,
        "current CPU-frame fixture has no IoStore traffic; keep the decoder surface explicit"
    );
    assert!(
        io_store["backends"].as_array().is_some(),
        "fixture dashboard should expose IoStore backend catalog field"
    );
    let dispatch = &json["dashboard"]["dispatch"];
    assert_eq!(
        dispatch["serial_ordered"].as_bool(),
        Some(true),
        "fixture dashboard should dispatch normal events in serial order"
    );
    assert!(
        dispatch["dispatched_event_count"].as_u64().unwrap() > 0,
        "fixture should dispatch normal events"
    );
    assert!(
        dispatch["synced_event_count"].as_u64().unwrap() > 0,
        "fixture should retain synced serials"
    );
    let submission_latency = &json["dashboard"]["gpu"]["submission_latency"];
    assert!(
        submission_latency["sample_count"].as_u64().unwrap() > 0,
        "fixture should collect GPU submission-latency samples from EventBeginWork"
    );
    assert!(
        submission_latency["median_delay_cycles"].as_i64().is_some(),
        "fixture should expose median GPU-start vs CPU-submit delay"
    );
    let breadcrumbs = &json["dashboard"]["gpu"]["breadcrumbs"];
    assert!(
        breadcrumbs["negative_durations"].as_u64().is_some(),
        "fixture should report GPU breadcrumb anomaly counters"
    );
    assert!(
        breadcrumbs["unmatched_ends"].as_u64().is_some(),
        "fixture should report unmatched GPU breadcrumb ends"
    );
    assert!(
        breadcrumbs["unterminated_scopes"].as_u64().is_some(),
        "fixture should report unterminated GPU breadcrumbs"
    );
    // Targeted captures for IoStore traffic, LoadTime request lifecycles, counter
    // samples, memory allocs, and metadata restore are still outstanding; this
    // fixture only characterizes the surfaces that are present (or explicitly empty).
    let frame_number = json["dashboard"]["frame_correlation"]["frames"][1]["frame_number"]
        .as_u64()
        .expect("fixture should expose correlated frames for timeline sampling")
        as u32;
    let timeline_output = binary()
        .args([
            "utrace",
            "dashboard",
            fixture.to_str().unwrap(),
            "--format=json",
            &format!("--frame={frame_number}"),
        ])
        .output()
        .expect("timeline dashboard should run");
    assert_eq!(
        timeline_output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&timeline_output.stderr)
    );
    let timeline_json: Value = serde_json::from_slice(&timeline_output.stdout).unwrap();
    let timeline = &timeline_json["dashboard"]["cpu"]["timeline"];
    assert_eq!(
        timeline["frame_number"].as_u64(),
        Some(u64::from(frame_number))
    );
    assert!(
        timeline["interval_count"].as_u64().unwrap() > 0,
        "selected correlated frame should retain CPU intervals"
    );
    assert!(
        timeline["intervals"].as_array().unwrap().len() <= 500,
        "timeline must stay within the default 500-interval cap"
    );
    if timeline["interval_count"].as_u64().unwrap() > 500 {
        assert_eq!(timeline["truncated"].as_bool(), Some(true));
    }
    let begin = timeline["begin_cycle"].as_u64().unwrap();
    let end = timeline["end_cycle"].as_u64().unwrap();
    assert!(end >= begin, "timeline window bounds must be ordered");
    for interval in timeline["intervals"].as_array().unwrap() {
        assert!(interval["thread_id"].as_u64().is_some());
        assert!(interval["spec_id"].as_u64().is_some());
        assert!(interval["duration"].as_u64().is_some());
        let start = interval["start_cycle"].as_u64().unwrap();
        let stop = interval["end_cycle"].as_u64().unwrap();
        let lo = start.min(stop);
        let hi = start.max(stop);
        assert!(
            lo >= begin && hi <= end,
            "retained interval must lie within the reported frame window"
        );
    }

    let absent_output = binary()
        .args([
            "utrace",
            "dashboard",
            fixture.to_str().unwrap(),
            "--format=json",
            "--frame=1",
        ])
        .output()
        .expect("absent-frame dashboard should run");
    assert_eq!(absent_output.status.code(), Some(0));
    let absent_json: Value = serde_json::from_slice(&absent_output.stdout).unwrap();
    let absent = &absent_json["dashboard"]["cpu"]["timeline"];
    assert_eq!(absent["frame_number"].as_u64(), Some(1));
    assert_eq!(absent["interval_count"].as_u64(), Some(0));
    assert!(absent["intervals"].as_array().unwrap().is_empty());

    let channels = &json["dashboard"]["channels"];
    assert!(
        channels["count"].as_u64().unwrap() > 0,
        "fixture should expose trace channels"
    );
    assert!(
        channels["toggles"].as_u64().unwrap() > 0,
        "fixture should count channel toggles"
    );
    assert!(
        channels["channels"]
            .as_array()
            .unwrap()
            .iter()
            .any(|channel| channel["name"].as_str() == Some("Concert")
                && channel["is_enabled"].as_bool().is_some()
                && channel["read_only"].as_bool().is_some()),
        "fixture should decode announced trace channel metadata"
    );
    let thread_groups = &json["dashboard"]["thread_groups"];
    assert!(
        thread_groups["begin_events"].as_u64().unwrap() > 0,
        "fixture should expose thread group begins"
    );
    assert_eq!(
        thread_groups["begin_events"].as_u64().unwrap(),
        thread_groups["end_events"].as_u64().unwrap(),
        "fixture thread group begin/end counts should balance"
    );
    assert!(
        thread_groups["groups"]
            .as_array()
            .unwrap()
            .iter()
            .any(
                |group| group["name"].as_str() == Some("BackgroundThreadPool")
                    && group["balanced"].as_bool() == Some(true)
            ),
        "fixture should decode named thread groups"
    );
    let bookmarks = &json["dashboard"]["annotations"]["bookmarks"];
    assert!(
        bookmarks["specs"].as_u64().unwrap() > 0,
        "fixture should expose bookmark specs"
    );
    assert!(
        bookmarks["bookmarks"]
            .as_array()
            .unwrap()
            .iter()
            .any(|bookmark| bookmark["format_string"]
                .as_str()
                .is_some_and(|format| !format.is_empty())),
        "fixture should expose bookmark format strings"
    );
    assert_eq!(
        json["dashboard"]["annotations"]["regions"]["completed"]
            .as_u64()
            .unwrap(),
        0,
        "current fixture does not emit completed regions"
    );
    let frames = json["dashboard"]["frames"].as_array().unwrap();
    assert!(!frames.is_empty(), "fixture should expose frame markers");
    assert!(
        frames.iter().any(|frame| frame["kind"] == "begin"),
        "fixture should expose frame begin markers"
    );
    assert!(
        frames.iter().any(|frame| frame["kind"] == "end"),
        "fixture should expose frame end markers"
    );

    let gpu = &json["dashboard"]["gpu"];
    assert_eq!(gpu["version"].as_u64(), Some(2));
    assert!(
        gpu["work"]["intervals"].as_u64().unwrap() > 0,
        "fixture should expose paired GPU work intervals"
    );
    assert!(
        gpu["work"]["queues"].as_u64().unwrap() > 0,
        "fixture should expose GPU queues"
    );
    let gpu_queues = gpu["queues"].as_array().unwrap();
    assert!(
        gpu_queues
            .iter()
            .any(|queue| queue["name"].as_str().is_some_and(|name| !name.is_empty())),
        "fixture should decode GPU queue names from QueueSpec"
    );
    assert!(
        gpu_queues.iter().any(
            |queue| queue["work_count"].as_u64().is_some_and(|count| count > 0)
                && queue["min_gpu_timestamp"].as_u64().is_some()
                && queue["max_gpu_timestamp"].as_u64().is_some()
        ),
        "fixture should expose per-queue GPU work timestamp bounds"
    );
    assert!(
        gpu_queues.iter().any(|queue| queue["frame_boundary_count"]
            .as_u64()
            .is_some_and(|count| count > 0)),
        "fixture should expose GPU frame boundaries"
    );
    assert!(
        gpu["frames"]
            .as_array()
            .unwrap()
            .iter()
            .any(|frame| frame["boundary_count"]
                .as_u64()
                .is_some_and(|count| count > 0)
                && frame["work_count"].as_u64().is_some()
                && frame["breadcrumb_count"].as_u64().is_some()
                && frame["top_breadcrumbs"]
                    .as_array()
                    .is_some_and(|breadcrumbs| breadcrumbs.iter().any(|breadcrumb| {
                        breadcrumb["name"]
                            .as_str()
                            .is_some_and(|name| !name.is_empty())
                    }))),
        "fixture should expose bounded queue-local GPU frame timeline summaries"
    );
    assert!(
        json["dashboard"]["frame_correlation"]["frames"]
            .as_array()
            .unwrap()
            .iter()
            .any(|frame| frame["frame_number"].as_u64().is_some()
                && frame["cpu_metadata_count"]
                    .as_u64()
                    .is_some_and(|count| count > 0)
                && frame["gpu_queue_count"]
                    .as_u64()
                    .is_some_and(|count| count > 0)
                && frame["gpu_breadcrumb_count"].as_u64().is_some()
                && frame["cpu_metadata_seconds"].as_f64().is_some()
                && frame["top_cpu_scopes"]
                    .as_array()
                    .is_some_and(|scopes| scopes.iter().any(|scope| {
                        scope["name"].as_str().is_some_and(|name| !name.is_empty())
                            && scope["total_cycles"]
                                .as_u64()
                                .is_some_and(|cycles| cycles > 0)
                    }))
                && frame["top_gpu_breadcrumbs"]
                    .as_array()
                    .is_some_and(|breadcrumbs| breadcrumbs.iter().any(|breadcrumb| {
                        breadcrumb["name"]
                            .as_str()
                            .is_some_and(|name| !name.is_empty())
                    }))),
        "fixture should expose bounded CPU/GPU frame correlation summaries"
    );
    assert!(
        gpu["breadcrumbs"]["specs"].as_u64().unwrap() > 0,
        "fixture should expose GPU breadcrumb specs"
    );
    assert!(
        gpu["breadcrumbs"]["intervals"].as_u64().unwrap() > 0,
        "fixture should expose paired GPU breadcrumb intervals"
    );
    assert!(
        gpu["breadcrumbs"]["field_names"]
            .as_array()
            .unwrap()
            .iter()
            .any(|name| name.as_str() == Some("Frame Number")),
        "fixture should decode GPU breadcrumb field-name payloads"
    );
    assert!(
        gpu["breadcrumbs"]["metadata_bytes"].as_u64().unwrap() > 0,
        "fixture should count GPU breadcrumb metadata bytes"
    );
    assert_eq!(
        gpu["breadcrumbs"]["metadata_bytes"].as_u64().unwrap(),
        gpu["breadcrumbs"]["decoded_metadata_bytes"]
            .as_u64()
            .unwrap()
            + gpu["breadcrumbs"]["undecoded_metadata_bytes"]
                .as_u64()
                .unwrap(),
        "GPU breadcrumb metadata byte accounting should balance"
    );
    assert!(
        gpu["breadcrumbs"]["metadata_strings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|name| name
                .as_str()
                .is_some_and(|name| name.contains("BuildRenderingCommands"))),
        "fixture should decode representative GPU breadcrumb metadata strings"
    );
    assert!(
        gpu["breadcrumbs"]["metadata_samples"]
            .as_array()
            .unwrap()
            .iter()
            .any(|sample| sample["rendered_name"]
                .as_str()
                .is_some_and(|name| name.contains("RenderGraphExecute"))
                && sample["fields"]
                    .as_object()
                    .is_some_and(|fields| !fields.is_empty())),
        "fixture should expose typed GPU breadcrumb metadata samples"
    );
    assert!(
        gpu["breadcrumbs"]["top"]
            .as_array()
            .unwrap()
            .iter()
            .any(|breadcrumb| breadcrumb["name"]
                .as_str()
                .is_some_and(|name| !name.is_empty())
                && breadcrumb["count"].as_u64().is_some_and(|count| count > 0)),
        "fixture should expose named GPU breadcrumb summaries"
    );
    assert!(
        gpu_queues.iter().any(|queue| queue["breadcrumb_count"]
            .as_u64()
            .is_some_and(|count| count > 0)),
        "fixture should expose per-queue GPU breadcrumb counts"
    );
}

#[test]
#[ignore = "requires a real .utrace fixture; set UTRACE_FIXTURE or UTRACE_FIXTURE_DIR"]
fn real_utrace_fixture_reports_decode_coverage() {
    let fixture = fixture();

    let output = binary()
        .args([
            "utrace",
            "coverage",
            fixture.to_str().unwrap(),
            "--format=json",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    assert!(
        output.stderr.is_empty(),
        "stderr was not empty: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    let coverage = &json["coverage"];
    let summary = &coverage["summary"];
    let declared = summary["declared_event_types"].as_u64().unwrap();
    assert!(declared > 0, "fixture should declare event types");
    assert_eq!(
        declared,
        summary["decoded_event_types"].as_u64().unwrap()
            + summary["partial_event_types"].as_u64().unwrap()
            + summary["raw_event_types"].as_u64().unwrap(),
        "coverage type counts should partition the declared events"
    );
    assert!(
        summary["decoded_event_types"].as_u64().unwrap() > 0,
        "fixture should have fully decoded event types"
    );
    assert_eq!(
        summary["raw_event_types"].as_u64().unwrap(),
        0,
        "fixture should have no raw declared event types"
    );
    assert_eq!(
        summary["raw_observed_events"].as_u64().unwrap(),
        0,
        "fixture should not contain observed events from raw families"
    );

    let events = coverage["events"].as_array().unwrap();
    assert!(
        events.iter().any(|entry| entry["status"] == "partial"
            && entry["note"].as_str().is_some_and(|note| !note.is_empty())),
        "partial events should carry a coverage note"
    );
    assert!(
        events.iter().all(|entry| entry["status"] != "raw"),
        "all fixture-declared event types should be classified"
    );
}

#[test]
#[ignore = "requires a real .utrace fixture; set UTRACE_FIXTURE or UTRACE_FIXTURE_DIR"]
fn real_utrace_fixture_cross_references_event_universe() {
    let fixture = fixture();

    // Minimal synthetic universe: one event the fixture declares, plus one it does not.
    let dir = std::env::temp_dir();
    let universe_path = dir.join("uasset_utrace_universe_fixture.txt");
    std::fs::write(&universe_path, "CpuProfiler.EventBatchV3\nMadeUp.Event\n").unwrap();

    let output = binary()
        .args([
            "utrace",
            "coverage",
            fixture.to_str().unwrap(),
            "--universe",
            universe_path.to_str().unwrap(),
            "--format=json",
        ])
        .output()
        .unwrap();

    let _ = std::fs::remove_file(&universe_path);
    assert_eq!(output.status.code(), Some(0));

    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    let universe = &json["coverage"]["universe"];
    assert_eq!(universe["total"].as_u64().unwrap(), 2);
    assert_eq!(
        universe["declared_in_trace"].as_u64().unwrap(),
        1,
        "one universe event is declared in the fixture"
    );
    assert!(
        universe["unseen"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event == "MadeUp.Event"),
        "universe events absent from the trace should be reported as unseen"
    );
}

#[test]
#[ignore = "requires a real .utrace fixture; set UTRACE_FIXTURE or UTRACE_FIXTURE_DIR"]
fn real_utrace_fixture_exposes_logging_dashboard() {
    let fixture = fixture();

    let output = binary()
        .args([
            "utrace",
            "dashboard",
            fixture.to_str().unwrap(),
            "--format=json",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    assert!(
        output.stderr.is_empty(),
        "stderr was not empty: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    let logging = &json["dashboard"]["logging"];
    assert!(
        logging["categories"].as_u64().unwrap() > 0,
        "fixture should expose log categories"
    );
    assert!(
        logging["message_specs"].as_u64().unwrap() > 0,
        "fixture should expose log message specs"
    );
    assert!(
        logging["verbosity"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["message_specs"]
                .as_u64()
                .is_some_and(|count| count > 0)),
        "fixture should expose a log verbosity breakdown"
    );
    assert!(
        logging["top_categories"]
            .as_array()
            .unwrap()
            .iter()
            .any(|category| category["name"]
                .as_str()
                .is_some_and(|name| !name.is_empty())
                && category["message_specs"]
                    .as_u64()
                    .is_some_and(|count| count > 0)),
        "fixture should expose named log categories with message specs"
    );
    assert!(
        logging["top_messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["file"]
                .as_str()
                .is_some_and(|file| !file.is_empty())
                && message["category"].as_str().is_some_and(|c| !c.is_empty())),
        "fixture should resolve log message specs to file and category"
    );
    assert!(
        logging["top_messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|message| message["sample_message"]
                .as_str()
                .is_some_and(|sample| sample.contains("Writing trace"))),
        "fixture should render the observed log format argument sample"
    );

    let session = &json["dashboard"]["session"];
    assert!(
        session["platform"]
            .as_str()
            .is_some_and(|platform| !platform.is_empty()),
        "fixture should decode the session platform"
    );
    assert!(
        session["app_name"]
            .as_str()
            .is_some_and(|app| !app.is_empty()),
        "fixture should decode the session app name"
    );
    assert!(
        session["configuration"].as_str().is_some(),
        "fixture should decode the session build configuration"
    );
    assert!(
        session["target_type"].as_str().is_some(),
        "fixture should decode the session target type"
    );
    assert!(
        session["instance_id"]
            .as_str()
            .is_some_and(|instance_id| instance_id.len() == 36 && instance_id.contains('-')),
        "fixture should format the session instance id as a GUID"
    );
}

#[test]
#[ignore = "requires a real .utrace fixture; set UTRACE_FIXTURE or UTRACE_FIXTURE_DIR"]
fn real_utrace_fixture_exposes_event_inventory() {
    let fixture = fixture();

    let output = binary()
        .args([
            "utrace",
            "inventory",
            fixture.to_str().unwrap(),
            "--format=json",
        ])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    assert!(
        output.stderr.is_empty(),
        "stderr was not empty: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["schema_version"], 2);
    assert_eq!(json["status"], "ok");
    assert!(
        json["inventory"]["summary"]["declared_event_types"]
            .as_u64()
            .unwrap()
            > 0,
        "fixture should declare event types"
    );
    assert!(
        json["inventory"]["summary"]["observed_events"]
            .as_u64()
            .unwrap()
            > 0,
        "fixture should contain observed events"
    );
    let events = json["inventory"]["events"].as_array().unwrap();
    assert!(
        events.iter().any(|event| event["logger"] == "CpuProfiler"
            && event["event"] == "EventBatchV3"
            && event["observed_count"].as_u64().unwrap() > 0),
        "inventory should count CpuProfiler.EventBatchV3"
    );
    assert!(
        events.iter().any(|event| event["logger"] == "Counters"
            && event["observed_count"].as_u64().unwrap_or(0) > 0),
        "inventory should surface raw non-CPU event families"
    );
    let gpu_begin_work = events
        .iter()
        .find(|event| event["logger"] == "GpuProfiler" && event["event"] == "EventBeginWork")
        .expect("inventory should include GpuProfiler.EventBeginWork");
    assert!(
        gpu_begin_work["samples"][0]["fields"]["QueueId"]
            .as_u64()
            .is_some(),
        "GPU sample should decode scalar fields"
    );
    assert!(
        gpu_begin_work["samples"][0]["fields"]["CPUTimestamp"]
            .as_u64()
            .is_some(),
        "GPU sample should decode CPU timestamp"
    );

    let counter_spec = events
        .iter()
        .find(|event| event["logger"] == "Counters" && event["event"] == "Spec")
        .expect("inventory should include Counters.Spec");
    assert!(
        counter_spec["samples"][0]["fields"]["Name"]
            .as_str()
            .is_some_and(|name| !name.is_empty()),
        "counter spec sample should decode aux string fields"
    );
}

#[test]
#[ignore = "requires targeted .utrace captures; set UTRACE_TARGETED_FIXTURE or UTRACE_TARGETED_FIXTURE_DIR"]
fn targeted_utrace_fixtures_exercise_provider_lifecycles() {
    let mut observed = BTreeSet::<String>::new();
    let mut counter_samples = 0_u64;
    let mut load_packages = 0_u64;
    let mut load_requests = 0_u64;
    let mut memory_scopes = 0_usize;
    let mut metadata_restores = 0_usize;
    for fixture in targeted_fixtures() {
        let output = binary()
            .args([
                "utrace",
                "inventory",
                fixture.to_str().unwrap(),
                "--format=json",
            ])
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}: {}",
            fixture.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        let json: Value = serde_json::from_slice(&output.stdout).unwrap();
        for event in json["inventory"]["events"].as_array().unwrap() {
            if event["observed_count"].as_u64().unwrap_or(0) == 0 {
                continue;
            }
            let logger = event["logger"].as_str().unwrap();
            let name = event["event"].as_str().unwrap();
            observed.insert(format!("{logger}.{name}"));
        }

        let output = binary()
            .args([
                "utrace",
                "dashboard",
                fixture.to_str().unwrap(),
                "--format=json",
            ])
            .output()
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(0),
            "{}: {}",
            fixture.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        let json: Value = serde_json::from_slice(&output.stdout).unwrap();
        let dashboard = &json["dashboard"];
        counter_samples += dashboard["counters"]["samples"].as_u64().unwrap();
        load_packages += dashboard["loading"]["package_count"].as_u64().unwrap();
        load_requests += dashboard["loading"]["requests"]["completed"]
            .as_u64()
            .unwrap();
        memory_scopes += dashboard["memory"]["scopes"].as_array().unwrap().len();
        metadata_restores += dashboard["metadata_stack"]["restored_stacks"]
            .as_array()
            .unwrap()
            .len();
    }

    for required in [
        "LoadTime.PackageSummary",
        "LoadTime.BeginRequest",
        "LoadTime.EndRequest",
        "Counters.SetValueInt",
        "Memory.MemoryScope",
        "MetadataStack.RestoreStack",
    ] {
        assert!(
            observed.contains(required),
            "targeted fixture corpus did not observe {required}; observed={observed:?}"
        );
    }
    assert!(
        counter_samples > 0,
        "counter values must reach the dashboard"
    );
    assert!(
        load_packages > 0,
        "package summaries must reach the dashboard"
    );
    assert!(
        load_requests > 0,
        "load requests must pair in the dashboard"
    );
    assert!(memory_scopes > 0, "memory scopes must reach the dashboard");
    assert!(
        metadata_restores > 0,
        "metadata restores must reach the dashboard"
    );
}

#[test]
#[ignore = "requires a cooked IoStore trace; set UTRACE_IOSTORE_FIXTURE"]
fn iostore_utrace_fixture_exercises_request_lifecycle() {
    let fixture = iostore_fixture();
    let output = binary()
        .args([
            "utrace",
            "dashboard",
            fixture.to_str().unwrap(),
            "--format=json",
        ])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    let io_store = &json["dashboard"]["io_store"];
    assert!(io_store["requests_created"].as_u64().unwrap() > 0);
    assert!(io_store["requests_started"].as_u64().unwrap() > 0);
    assert!(
        io_store["requests_completed"].as_u64().unwrap()
            + io_store["requests_failed"].as_u64().unwrap()
            + io_store["requests_unresolved"].as_u64().unwrap()
            > 0
    );
    assert!(!io_store["request_samples"].as_array().unwrap().is_empty());
}
