use std::path::PathBuf;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_uasset"))
}

fn run_with_stdin(args: &[&str], bytes: &[u8]) -> Output {
    let mut child = binary()
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    {
        use std::io::Write;
        child.stdin.take().unwrap().write_all(bytes).unwrap();
    }

    child.wait_with_output().unwrap()
}

// The StarterContent sample lived alongside the old in-engine project location.
// Resolve an explicit override first, then the historical relative default.
fn fixture() -> PathBuf {
    let path = std::env::var_os("UASSET_STARTER_SAMPLE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
                "../Samples/StarterContent/Content/StarterContent/Architecture/Floor_400x400.uasset",
            )
        });
    if path.is_file() {
        path
    } else {
        panic!(
            "StarterContent sample not found at {}; set UASSET_STARTER_SAMPLE",
            path.display()
        );
    }
}

#[test]
#[ignore = "requires StarterContent Floor_400x400.uasset; set UASSET_STARTER_SAMPLE"]
fn json_success_is_machine_readable_and_stderr_is_empty() {
    let fixture = fixture();
    let output = binary()
        .args(["inspect", fixture.to_str().unwrap(), "--format", "json"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());

    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["schema_version"], 6);
    assert_eq!(json["status"], "ok");
    assert_eq!(json["package"]["version"]["ue4"], 522);
    assert_eq!(json["package"]["version"]["ue5"], 1006);
    assert!(json["package"]["names"]["count"].as_u64().unwrap() > 0);
}

#[test]
fn json_io_error_uses_stderr_and_exit_code_four() {
    let output = binary()
        .args(["inspect", "does-not-exist.uasset", "--format=json"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(4));
    assert!(output.stdout.is_empty());

    let json: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(json["schema_version"], 6);
    assert_eq!(json["status"], "error");
    assert_eq!(json["kind"], "io");
}

#[test]
fn json_malformed_package_uses_stderr_and_exit_code_two() {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&0x9E2A_83C1_u32.to_le_bytes());
    bytes.extend_from_slice(&(-8_i32).to_le_bytes());

    let output = run_with_stdin(&["inspect", "-", "--format=json"], &bytes);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());

    let json: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(json["schema_version"], 6);
    assert_eq!(json["status"], "error");
    assert_eq!(json["path"], "-");
    assert_eq!(json["kind"], "malformed_data");
    assert_eq!(json["field"], "Summary.LegacyUE3Version");
    assert_eq!(json["offset"], 8);
}

#[test]
#[ignore = "requires a partially decodable .uasset; set UASSET_PARTIAL_SAMPLE"]
fn partial_decode_reports_errors_and_exit_six() {
    // A package where the summary parses but at least one export fails to decode
    // must emit the decoded assets, list the failures in `decode_errors`, set
    // status "partial", and exit 6 (not abort the whole file). Gated on a sample
    // since it needs a real such asset.
    let path = std::env::var_os("UASSET_PARTIAL_SAMPLE")
        .map(PathBuf::from)
        .expect("set UASSET_PARTIAL_SAMPLE to a package with an undecodable export");
    let output = binary()
        .args(["inspect", path.to_str().unwrap(), "--format", "json"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(6));
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["schema_version"], 6);
    assert_eq!(json["status"], "partial");
    assert!(
        json["decode_errors"]
            .as_array()
            .is_some_and(|errors| !errors.is_empty())
    );
}

#[test]
#[ignore = "requires StarterContent Floor_400x400.uasset; set UASSET_STARTER_SAMPLE"]
fn stdin_accepts_package_bytes() {
    let fixture = fixture();
    let bytes = std::fs::read(fixture).unwrap();
    let output = run_with_stdin(&["inspect", "-", "--format=json"], &bytes);

    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["path"], "-");
}

#[test]
fn usage_errors_do_not_write_stdout() {
    let output = binary().args(["inspect"]).output().unwrap();

    assert_eq!(output.status.code(), Some(64));
    assert!(output.stdout.is_empty());
    assert!(!output.stderr.is_empty());
}

#[cfg(feature = "utrace")]
#[test]
fn utrace_json_malformed_trace_uses_stderr_and_exit_code_two() {
    let bytes = b"2CRT\0\0\x04";

    let output = run_with_stdin(&["utrace", "inspect", "-", "--format=json"], bytes);

    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty());

    let json: Value = serde_json::from_slice(&output.stderr).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["status"], "error");
    assert_eq!(json["path"], "-");
    assert_eq!(json["kind"], "malformed_data");
    assert_eq!(json["field"], "Header.ProtocolVersion");
    assert_eq!(json["offset"], 7);
}

#[cfg(feature = "utrace")]
#[test]
fn utrace_json_success_decodes_prologue_and_threads() {
    let path = std::env::temp_dir().join(format!(
        "uasset-parser-utrace-{}-{}.utrace",
        std::process::id(),
        "phase4"
    ));
    std::fs::write(&path, synthetic_utrace()).unwrap();

    let output = binary()
        .args(["utrace", "inspect", path.to_str().unwrap(), "--format=json"])
        .output()
        .unwrap();
    let _ = std::fs::remove_file(&path);

    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());

    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["status"], "ok");
    assert_eq!(json["trace"]["prologue"]["start_cycle"], 100);
    assert_eq!(json["trace"]["prologue"]["cycle_frequency"], 1_000_000);
    assert_eq!(json["trace"]["thread_info"][0]["thread_id"], 2);
    assert_eq!(json["trace"]["thread_info"][0]["name"], "GameThread");
}

#[cfg(feature = "utrace")]
#[test]
fn utrace_coverage_json_contract_cross_references_universe() {
    let dir = std::env::temp_dir();
    let trace_path = dir.join(format!(
        "uasset-parser-utrace-{}-coverage.utrace",
        std::process::id()
    ));
    let universe_path = dir.join(format!(
        "uasset-parser-utrace-{}-universe.txt",
        std::process::id()
    ));
    std::fs::write(&trace_path, synthetic_utrace()).unwrap();
    std::fs::write(&universe_path, "$Trace.NewTrace\nMadeUp.Event\n").unwrap();

    let output = binary()
        .args([
            "utrace",
            "coverage",
            trace_path.to_str().unwrap(),
            "--universe",
            universe_path.to_str().unwrap(),
            "--format=json",
        ])
        .output()
        .unwrap();
    let _ = std::fs::remove_file(&trace_path);
    let _ = std::fs::remove_file(&universe_path);

    assert_eq!(output.status.code(), Some(0));
    assert!(
        output.stderr.is_empty(),
        "stderr was not empty: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["schema_version"], 1);
    assert_eq!(json["status"], "ok");
    assert_eq!(json["coverage"]["summary"]["declared_event_types"], 2);
    assert_eq!(json["coverage"]["summary"]["decoded_event_types"], 2);
    assert_eq!(json["coverage"]["summary"]["raw_event_types"], 0);
    assert_eq!(json["coverage"]["summary"]["raw_observed_events"], 0);
    assert!(
        json["coverage"]["events"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event["logger"] == "$Trace"
                && event["event"] == "NewTrace"
                && event["status"] == "decoded"
                && event["note"].as_str().is_some())
    );
    assert_eq!(json["coverage"]["universe"]["total"], 2);
    assert_eq!(json["coverage"]["universe"]["declared_in_trace"], 1);
    assert!(
        json["coverage"]["universe"]["unseen"]
            .as_array()
            .unwrap()
            .iter()
            .any(|event| event == "MadeUp.Event")
    );
}

#[cfg(feature = "utrace")]
fn synthetic_utrace() -> Vec<u8> {
    const UINT8: u8 = 0x00;
    const UINT16: u8 = 0x01;
    const UINT32: u8 = 0x02;
    const UINT64: u8 = 0x03;
    const INT32: u8 = 0x12;
    const FLOAT64: u8 = 0x43;
    const ANSI_STRING: u8 = 0x88;

    let new_trace_uid = 10;
    let thread_info_uid = 11;
    let new_trace_decl = new_event(
        new_trace_uid,
        0x05,
        "$Trace",
        "NewTrace",
        &[
            field(0, 8, UINT64, "StartCycle"),
            field(8, 8, UINT64, "CycleFrequency"),
            field(16, 2, UINT16, "Endian"),
            field(18, 1, UINT8, "PointerSize"),
            field(19, 8, FLOAT64, "StartDateTime"),
        ],
    );
    let thread_info_decl = new_event(
        thread_info_uid,
        0x07,
        "$Trace",
        "ThreadInfo",
        &[
            field(0, 4, UINT32, "ThreadId"),
            field(4, 4, UINT32, "SystemId"),
            field(8, 4, INT32, "SortHint"),
            field(12, 0, ANSI_STRING, "Name"),
        ],
    );

    let mut new_trace_data = Vec::new();
    new_trace_data.extend_from_slice(&100_u64.to_le_bytes());
    new_trace_data.extend_from_slice(&1_000_000_u64.to_le_bytes());
    new_trace_data.extend_from_slice(&0x524d_u16.to_le_bytes());
    new_trace_data.push(8);
    new_trace_data.extend_from_slice(&1234.5_f64.to_le_bytes());

    let mut thread_data = Vec::new();
    thread_data.extend_from_slice(&2_u32.to_le_bytes());
    thread_data.extend_from_slice(&99_u32.to_le_bytes());
    thread_data.extend_from_slice(&(-7_i32).to_le_bytes());
    thread_data.extend_from_slice(&aux(3, b"GameThread"));
    thread_data.push(3);

    trace_with_events(&[
        important_event(0, &new_trace_decl),
        important_event(0, &thread_info_decl),
        important_event(new_trace_uid, &new_trace_data),
        important_event(thread_info_uid, &thread_data),
    ])
}

#[cfg(feature = "utrace")]
#[derive(Clone)]
struct TraceField {
    offset: u16,
    size: u16,
    type_info: u8,
    name: &'static str,
}

#[cfg(feature = "utrace")]
fn field(offset: u16, size: u16, type_info: u8, name: &'static str) -> TraceField {
    TraceField {
        offset,
        size,
        type_info,
        name,
    }
}

#[cfg(feature = "utrace")]
fn new_event(uid: u16, flags: u8, logger: &str, event: &str, fields: &[TraceField]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&uid.to_le_bytes());
    bytes.push(u8::try_from(fields.len()).unwrap());
    bytes.push(flags);
    bytes.push(u8::try_from(logger.len()).unwrap());
    bytes.push(u8::try_from(event.len()).unwrap());
    for field in fields {
        bytes.push(0);
        bytes.push(0);
        bytes.extend_from_slice(&field.offset.to_le_bytes());
        bytes.extend_from_slice(&field.size.to_le_bytes());
        bytes.push(field.type_info);
        bytes.push(u8::try_from(field.name.len()).unwrap());
    }
    bytes.extend_from_slice(logger.as_bytes());
    bytes.extend_from_slice(event.as_bytes());
    for field in fields {
        bytes.extend_from_slice(field.name.as_bytes());
    }
    bytes
}

#[cfg(feature = "utrace")]
fn important_event(uid: u16, data: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&uid.to_le_bytes());
    bytes.extend_from_slice(&u16::try_from(data.len()).unwrap().to_le_bytes());
    bytes.extend_from_slice(data);
    bytes
}

#[cfg(feature = "utrace")]
fn aux(field_index: u8, payload: &[u8]) -> Vec<u8> {
    let pack =
        1_u32 | (u32::from(field_index) << 8) | (u32::try_from(payload.len()).unwrap() << 13);
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&pack.to_le_bytes());
    bytes.extend_from_slice(payload);
    bytes
}

#[cfg(feature = "utrace")]
fn trace_with_events(events: &[Vec<u8>]) -> Vec<u8> {
    let packet_payload = events.concat();
    let packet_size = u16::try_from(packet_payload.len() + 4).unwrap();
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"2CRT");
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.push(4);
    bytes.push(7);
    bytes.extend_from_slice(&packet_size.to_le_bytes());
    bytes.extend_from_slice(&0_u16.to_le_bytes());
    bytes.extend_from_slice(&packet_payload);
    bytes
}
