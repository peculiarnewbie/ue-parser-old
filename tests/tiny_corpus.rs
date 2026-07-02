use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;
#[cfg(feature = "utrace")]
use uasset_parser::utrace::{self, TraceErrorKind};
use uasset_parser::{Package, PackageErrorKind, PackageSummary};

const MINIMAL_CURRENT_SUMMARY: &str =
    include_str!("fixtures/tiny/minimal-current-summary.uasset.hex");
#[cfg(feature = "utrace")]
const MINIMAL_PROLOGUE_TRACE: &str = include_str!("fixtures/tiny/minimal-prologue.utrace.hex");

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_uasset"))
}

fn fixture_bytes(hex: &str) -> Vec<u8> {
    let digits = hex
        .lines()
        .flat_map(|line| line.split_once('#').map_or(line, |(hex, _)| hex).chars())
        .filter(|ch| !ch.is_whitespace())
        .map(|ch| {
            ch.to_digit(16)
                .unwrap_or_else(|| panic!("fixture contains non-hex character {ch:?}"))
        })
        .collect::<Vec<_>>();
    assert_eq!(digits.len() % 2, 0, "fixture hex must contain whole bytes");
    digits
        .chunks_exact(2)
        .map(|byte| ((byte[0] << 4) | byte[1]) as u8)
        .collect()
}

fn temp_fixture_path(extension: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before UNIX_EPOCH")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "uasset-parser-tiny-corpus-{}-{nonce}.{extension}",
        std::process::id()
    ))
}

#[test]
fn committed_minimal_package_fixture_parses() {
    let bytes = fixture_bytes(MINIMAL_CURRENT_SUMMARY);

    let summary = PackageSummary::parse(&bytes).expect("summary parses");
    assert_eq!(summary.versions.legacy_file_version, -9);
    assert_eq!(summary.versions.ue4, 522);
    assert_eq!(summary.versions.ue5, 1018);
    assert_eq!(summary.total_header_size as usize, bytes.len());
    assert_eq!(summary.names.count, 0);
    assert_eq!(summary.imports.count, 0);
    assert_eq!(summary.exports.count, 0);

    let package = Package::parse(&bytes).expect("package parses");
    assert!(package.names.is_empty());
    assert!(package.imports.is_empty());
    assert!(package.exports.is_empty());
}

#[test]
fn package_parse_rejects_corrupt_and_truncated_minimal_fixture() {
    let bytes = fixture_bytes(MINIMAL_CURRENT_SUMMARY);

    let truncated = &bytes[..bytes.len() - 1];
    let error = Package::parse(truncated).expect_err("truncated package must fail");
    assert_eq!(error.kind(), PackageErrorKind::MalformedData);
    assert!(!error.path().is_empty());

    let mut corrupt = bytes;
    corrupt[0] ^= 0xff;
    let error = Package::parse(&corrupt).expect_err("corrupt package magic must fail");
    assert_eq!(error.kind(), PackageErrorKind::UnsupportedFormat);
    assert_eq!(error.path(), "Summary.Tag");
}

#[test]
fn cli_inspects_committed_minimal_package_fixture() {
    let path = temp_fixture_path("uasset");
    std::fs::write(&path, fixture_bytes(MINIMAL_CURRENT_SUMMARY)).unwrap();

    let output = binary()
        .args(["inspect", path.to_str().unwrap(), "--format=json"])
        .output()
        .unwrap();
    let _ = std::fs::remove_file(&path);

    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());

    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["schema_version"], 6);
    assert_eq!(json["status"], "ok");
    assert_eq!(json["package"]["version"]["ue5"], 1018);
    assert_eq!(json["package"]["names"]["count"], 0);
    assert_eq!(json["package"]["exports"]["count"], 0);
    assert_eq!(json["assets"].as_array().unwrap().len(), 0);
}

#[cfg(feature = "utrace")]
#[test]
fn cli_inspects_committed_minimal_utrace_fixture() {
    let path = temp_fixture_path("utrace");
    std::fs::write(&path, fixture_bytes(MINIMAL_PROLOGUE_TRACE)).unwrap();

    let output = binary()
        .args(["utrace", "inspect", path.to_str().unwrap(), "--format=json"])
        .output()
        .unwrap();
    let _ = std::fs::remove_file(&path);

    assert_eq!(
        output.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
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
fn utrace_inspect_rejects_corrupt_and_truncated_minimal_fixture() {
    let bytes = fixture_bytes(MINIMAL_PROLOGUE_TRACE);

    let truncated = &bytes[..bytes.len() - 1];
    let error = utrace::inspect(truncated).expect_err("truncated trace must fail");
    assert_eq!(error.kind(), TraceErrorKind::MalformedData);
    assert!(!error.path().is_empty());

    let mut corrupt = bytes;
    corrupt[0] ^= 0xff;
    let error = utrace::inspect(&corrupt).expect_err("corrupt trace magic must fail");
    assert_eq!(error.kind(), TraceErrorKind::MalformedData);
    assert_eq!(error.path(), "Header.Magic");
}
