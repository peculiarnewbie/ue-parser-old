use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde_json::Value;

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_uasset"))
}

// The StarterContent sample lived alongside the old in-engine project
// location. Resolve an explicit override first, then the historical relative
// default; tests that need it skip when neither is present so portable builds
// outside that layout still pass.
fn fixture() -> Option<PathBuf> {
    let path = std::env::var_os("UASSET_STARTER_SAMPLE")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(
                "../Samples/StarterContent/Content/StarterContent/Architecture/Floor_400x400.uasset",
            )
        });
    if path.is_file() {
        Some(path)
    } else {
        eprintln!(
            "skipping StarterContent CLI check; set UASSET_STARTER_SAMPLE to a Floor_400x400.uasset to run it"
        );
        None
    }
}

#[test]
fn json_success_is_machine_readable_and_stderr_is_empty() {
    let Some(fixture) = fixture() else {
        return;
    };
    let output = binary()
        .args(["inspect", fixture.to_str().unwrap(), "--format", "json"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(0));
    assert!(output.stderr.is_empty());

    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["schema_version"], 3);
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
    assert_eq!(json["schema_version"], 3);
    assert_eq!(json["status"], "error");
    assert_eq!(json["kind"], "io");
}

#[test]
fn stdin_accepts_package_bytes() {
    let Some(fixture) = fixture() else {
        return;
    };
    let bytes = std::fs::read(fixture).unwrap();
    let mut child = binary()
        .args(["inspect", "-", "--format=json"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();

    {
        use std::io::Write;
        child.stdin.take().unwrap().write_all(&bytes).unwrap();
    }
    let output = child.wait_with_output().unwrap();

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
