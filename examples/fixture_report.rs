use std::env;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use uasset_parser::PackageSummary;

const CONTRACT_JSON: &str = include_str!("../tests/fixtures/electroswag-v7.json");

#[derive(Deserialize)]
struct FixtureContract {
    contract_version: String,
    default_fixture_dir: PathBuf,
    expected_versions: ExpectedVersions,
    assets: Vec<FixtureAsset>,
}

#[derive(Deserialize)]
struct ExpectedVersions {
    legacy_file: i32,
    ue4: i32,
    ue5: i32,
    licensee: i32,
}

#[derive(Deserialize)]
struct FixtureAsset {
    file: PathBuf,
    package_name: String,
}

struct AssetReport {
    file: String,
    status: String,
    package_name: String,
    legacy_file: Option<i32>,
    ue4: Option<i32>,
    ue5: Option<i32>,
    licensee: Option<i32>,
    package_flags: Option<u32>,
    summary_size: Option<u64>,
    total_header_size: Option<u32>,
    names_count: Option<u32>,
    names_offset: Option<u64>,
    imports_count: Option<u32>,
    imports_offset: Option<u64>,
    exports_count: Option<u32>,
    exports_offset: Option<u64>,
    assertions: Vec<AssertionReport>,
    error: Option<String>,
}

struct AssertionReport {
    name: &'static str,
    expected: String,
    actual: String,
    passed: bool,
}

fn main() {
    let contract: FixtureContract =
        serde_json::from_str(CONTRACT_JSON).expect("fixture contract JSON must parse");
    let fixture_dir = env::var_os("UASSET_FIXTURE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| contract.default_fixture_dir.clone());
    let reports = contract
        .assets
        .iter()
        .map(|asset| inspect_asset(&contract, &fixture_dir, asset))
        .collect::<Vec<_>>();

    let output_path = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("target/fixture-report/electroswag-v7.html"));
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent).expect("report output directory must be creatable");
    }
    std::fs::write(&output_path, render_html(&contract, &fixture_dir, &reports))
        .expect("report must be writable");
    println!("{}", output_path.display());
}

fn inspect_asset(
    contract: &FixtureContract,
    fixture_dir: &Path,
    asset: &FixtureAsset,
) -> AssetReport {
    let path = fixture_dir.join(&asset.file);
    let mut report = AssetReport {
        file: asset.file.to_string_lossy().replace('\\', "/"),
        status: "fail".to_owned(),
        package_name: String::new(),
        legacy_file: None,
        ue4: None,
        ue5: None,
        licensee: None,
        package_flags: None,
        summary_size: None,
        total_header_size: None,
        names_count: None,
        names_offset: None,
        imports_count: None,
        imports_offset: None,
        exports_count: None,
        exports_offset: None,
        assertions: Vec::new(),
        error: None,
    };

    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) => {
            report.error = Some(error.to_string());
            report.assertions.push(assertion(
                "file exists",
                "readable file".to_owned(),
                error.to_string(),
                false,
            ));
            return report;
        }
    };

    let summary = match PackageSummary::parse(&bytes) {
        Ok(summary) => summary,
        Err(error) => {
            report.error = Some(error.to_string());
            report.assertions.push(assertion(
                "package parses",
                "valid PackageSummary".to_owned(),
                error.to_string(),
                false,
            ));
            return report;
        }
    };

    report.package_name = summary.package_name.clone();
    report.legacy_file = Some(summary.versions.legacy_file_version);
    report.ue4 = Some(summary.versions.ue4);
    report.ue5 = Some(summary.versions.ue5);
    report.licensee = Some(summary.versions.licensee);
    report.package_flags = Some(summary.versions.package_flags.bits());
    report.summary_size = Some(summary.span.len());
    report.total_header_size = Some(summary.total_header_size);
    report.names_count = Some(summary.names.count);
    report.names_offset = Some(summary.names.offset.get());
    report.imports_count = Some(summary.imports.count);
    report.imports_offset = Some(summary.imports.offset.get());
    report.exports_count = Some(summary.exports.count);
    report.exports_offset = Some(summary.exports.offset.get());

    report.assertions.extend([
        assertion(
            "package name",
            asset.package_name.clone(),
            summary.package_name.clone(),
            summary.package_name == asset.package_name,
        ),
        assertion(
            "legacy file version",
            contract.expected_versions.legacy_file.to_string(),
            summary.versions.legacy_file_version.to_string(),
            summary.versions.legacy_file_version == contract.expected_versions.legacy_file,
        ),
        assertion(
            "UE4 version",
            contract.expected_versions.ue4.to_string(),
            summary.versions.ue4.to_string(),
            summary.versions.ue4 == contract.expected_versions.ue4,
        ),
        assertion(
            "UE5 version",
            contract.expected_versions.ue5.to_string(),
            summary.versions.ue5.to_string(),
            summary.versions.ue5 == contract.expected_versions.ue5,
        ),
        assertion(
            "licensee version",
            contract.expected_versions.licensee.to_string(),
            summary.versions.licensee.to_string(),
            summary.versions.licensee == contract.expected_versions.licensee,
        ),
        assertion(
            "name map non-empty",
            "> 0".to_owned(),
            summary.names.count.to_string(),
            summary.names.count > 0,
        ),
        assertion(
            "exports non-empty",
            "> 0".to_owned(),
            summary.exports.count.to_string(),
            summary.exports.count > 0,
        ),
    ]);

    report.status = if report.assertions.iter().all(|assertion| assertion.passed) {
        "pass".to_owned()
    } else {
        "fail".to_owned()
    };
    report
}

fn assertion(
    name: &'static str,
    expected: String,
    actual: String,
    passed: bool,
) -> AssertionReport {
    AssertionReport {
        name,
        expected,
        actual,
        passed,
    }
}

fn render_html(contract: &FixtureContract, fixture_dir: &Path, reports: &[AssetReport]) -> String {
    let pass_count = reports
        .iter()
        .filter(|report| report.status == "pass")
        .count();
    let assertion_count = reports.iter().flat_map(|report| &report.assertions).count();
    let assertion_pass_count = reports
        .iter()
        .flat_map(|report| &report.assertions)
        .filter(|assertion| assertion.passed)
        .count();
    let mut html = String::new();
    write!(
        html,
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>UAsset fixture parse report</title>
<style>
:root {{ color-scheme: dark; font-family: Inter, Segoe UI, Arial, sans-serif; background: #0f1115; color: #e8eaf0; }}
body {{ margin: 32px; }}
h1 {{ margin-bottom: 4px; }}
.muted {{ color: #a4adbd; }}
.summary {{ display: grid; grid-template-columns: repeat(4, max-content); gap: 12px; margin: 24px 0; }}
.card {{ background: #171b24; border: 1px solid #30384a; border-radius: 10px; padding: 12px 16px; }}
.card .value {{ font-size: 24px; font-weight: 700; }}
table {{ width: 100%; border-collapse: collapse; margin: 16px 0 28px; }}
th, td {{ border-bottom: 1px solid #30384a; padding: 8px 10px; text-align: left; vertical-align: top; }}
th {{ color: #c8d0df; background: #171b24; position: sticky; top: 0; }}
code {{ background: #11141b; padding: 2px 5px; border-radius: 5px; }}
.pass {{ color: #74d99f; font-weight: 700; }}
.fail {{ color: #ff807d; font-weight: 700; }}
details {{ margin: 8px 0 18px; border: 1px solid #30384a; border-radius: 10px; background: #151923; }}
summary {{ cursor: pointer; padding: 12px 14px; }}
.details-body {{ padding: 0 14px 14px; }}
.json {{ white-space: pre-wrap; font-family: ui-monospace, SFMono-Regular, Consolas, monospace; background: #0b0d12; border-radius: 8px; padding: 12px; overflow-x: auto; }}
</style>
</head>
<body>
<h1>UAsset fixture parse report</h1>
<div class="muted">Contract <code>{}</code> · Fixture root <code>{}</code></div>
<div class="summary">
<div class="card"><div class="value">{}/{}</div><div class="muted">assets passed</div></div>
<div class="card"><div class="value">{}/{}</div><div class="muted">assertions passed</div></div>
<div class="card"><div class="value">{}</div><div class="muted">expected UE5 version</div></div>
<div class="card"><div class="value">PackageSummary</div><div class="muted">schema source</div></div>
</div>
"#,
        escape(&contract.contract_version),
        escape(&fixture_dir.display().to_string()),
        pass_count,
        reports.len(),
        assertion_pass_count,
        assertion_count,
        contract.expected_versions.ue5
    )
    .unwrap();

    html.push_str(
        "<table><thead><tr><th>Status</th><th>Asset</th><th>Package</th><th>Versions</th><th>Tables</th><th>Header</th></tr></thead><tbody>",
    );
    for report in reports {
        write!(
            html,
            "<tr><td class=\"{}\">{}</td><td><code>{}</code></td><td><code>{}</code></td><td>legacy={} UE4={} UE5={}</td><td>names={} imports={} exports={}</td><td>summary={} total={}</td></tr>",
            escape(&report.status),
            escape(&report.status),
            escape(&report.file),
            escape(&report.package_name),
            opt_i32(report.legacy_file),
            opt_i32(report.ue4),
            opt_i32(report.ue5),
            opt_u32(report.names_count),
            opt_u32(report.imports_count),
            opt_u32(report.exports_count),
            opt_u64(report.summary_size),
            opt_u32(report.total_header_size),
        )
        .unwrap();
    }
    html.push_str("</tbody></table>");

    for report in reports {
        write!(
            html,
            "<details><summary><span class=\"{}\">{}</span> <code>{}</code></summary><div class=\"details-body\">",
            escape(&report.status),
            escape(&report.status),
            escape(&report.file)
        )
        .unwrap();
        html.push_str("<table><thead><tr><th>Assertion</th><th>Expected</th><th>Actual</th><th>Status</th></tr></thead><tbody>");
        for assertion in &report.assertions {
            write!(
                html,
                "<tr><td>{}</td><td><code>{}</code></td><td><code>{}</code></td><td class=\"{}\">{}</td></tr>",
                escape(assertion.name),
                escape(&assertion.expected),
                escape(&assertion.actual),
                if assertion.passed { "pass" } else { "fail" },
                if assertion.passed { "pass" } else { "fail" },
            )
            .unwrap();
        }
        html.push_str("</tbody></table>");
        write!(
            html,
            "<div class=\"json\">{}</div>",
            escape(&jsonish_report(report))
        )
        .unwrap();
        if let Some(error) = &report.error {
            write!(html, "<p class=\"fail\">{}</p>", escape(error)).unwrap();
        }
        html.push_str("</div></details>");
    }
    html.push_str("</body></html>");
    html
}

fn jsonish_report(report: &AssetReport) -> String {
    format!(
        r#"{{
  "file": "{}",
  "package_name": "{}",
  "legacy_file": {},
  "ue4": {},
  "ue5": {},
  "licensee": {},
  "package_flags": {},
  "summary_size": {},
  "total_header_size": {},
  "names": {{ "count": {}, "offset": {} }},
  "imports": {{ "count": {}, "offset": {} }},
  "exports": {{ "count": {}, "offset": {} }}
}}"#,
        report.file,
        report.package_name,
        opt_i32(report.legacy_file),
        opt_i32(report.ue4),
        opt_i32(report.ue5),
        opt_i32(report.licensee),
        opt_u32(report.package_flags),
        opt_u64(report.summary_size),
        opt_u32(report.total_header_size),
        opt_u32(report.names_count),
        opt_u64(report.names_offset),
        opt_u32(report.imports_count),
        opt_u64(report.imports_offset),
        opt_u32(report.exports_count),
        opt_u64(report.exports_offset)
    )
}

fn escape(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn opt_i32(value: Option<i32>) -> String {
    value.map_or_else(|| "null".to_owned(), |value| value.to_string())
}

fn opt_u32(value: Option<u32>) -> String {
    value.map_or_else(|| "null".to_owned(), |value| value.to_string())
}

fn opt_u64(value: Option<u64>) -> String {
    value.map_or_else(|| "null".to_owned(), |value| value.to_string())
}
