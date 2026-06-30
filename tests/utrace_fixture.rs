#![cfg(feature = "utrace")]

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_uasset"))
}

fn fixture() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("UTRACE_FIXTURE").map(PathBuf::from) {
        return require_file_or_skip(path, "UTRACE_FIXTURE");
    }

    if let Some(dir) = std::env::var_os("UTRACE_FIXTURE_DIR").map(PathBuf::from) {
        if let Some(path) = first_utrace_in(&dir) {
            return Some(path);
        }
        return missing_fixture(format!(
            "UTRACE_FIXTURE_DIR did not contain a .utrace file: {}",
            dir.display()
        ));
    }

    let default_dir = PathBuf::from("D:/Perforce/Arif_Fixtures/Traces");
    if let Some(path) = first_utrace_in(&default_dir) {
        return Some(path);
    }

    missing_fixture(
        "set UTRACE_FIXTURE to a .utrace file or UTRACE_FIXTURE_DIR to a directory containing one"
            .to_owned(),
    )
}

fn require_file_or_skip(path: PathBuf, source: &str) -> Option<PathBuf> {
    if path.is_file() {
        Some(path)
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

fn missing_fixture(message: String) -> Option<PathBuf> {
    if std::env::var_os("UTRACE_REQUIRE_FIXTURE").is_some() {
        panic!("{message}");
    }
    eprintln!("skipping UTrace fixture check; {message}");
    None
}

#[test]
fn real_utrace_fixture_exposes_header_prologue_threads_and_registry() {
    let Some(fixture) = fixture() else {
        return;
    };

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
    assert_eq!(json["schema_version"], 1);
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
fn real_utrace_fixture_exposes_cpu_dashboard_summary() {
    let Some(fixture) = fixture() else {
        return;
    };

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
    assert_eq!(json["schema_version"], 1);
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
        json["dashboard"]["cpu"]["scopes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|scope| scope["name"].as_str().is_some_and(|name| !name.is_empty())),
        "fixture should expose named CPU scope summaries"
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
}
