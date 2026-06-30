//! Catalog the primary asset class of every `.uasset` under a directory tree and
//! measure how completely the parser decodes each one.
//!
//! Usage: `cargo run --release --example catalog -- <root> [<root> ...]`
//!
//! Walks each root recursively, parses every `.uasset`/`.umap` package, and for
//! each file's "primary" export (those flagged `is_asset`, falling back to
//! top-level exports whose outer is null) records:
//!   - the resolved class path, and
//!   - the decode outcome: fully decoded (and as which `DecodedAsset` kind),
//!     decode-failed, or skipped (no decoder applies / zero payload).
//!
//! Prints a per-class coverage table plus package-level parse-failure counts.
//! This is a coverage-tracking aid, not part of the shipped CLI contract — see
//! `docs/asset-coverage.md`.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use uasset_parser::asset::{AssetDecodeContext, decode_export};
use uasset_parser::package::{ObjectPath, Package, PackageIndex};
use uasset_parser::schema::{ClassSchema, SchemaProvider, StructSchema};

struct EmptySchemas;

impl SchemaProvider for EmptySchemas {
    fn find_struct(&self, _path: &ObjectPath) -> Option<&StructSchema> {
        None
    }
    fn find_class(&self, _path: &ObjectPath) -> Option<&ClassSchema> {
        None
    }
}

#[derive(Default)]
struct Coverage {
    /// Primary exports of this class that fully decoded.
    decoded: u64,
    /// Primary exports that a decoder attempted but rejected (malformed/tail/etc).
    failed: u64,
    /// Primary exports no decoder claimed, or with zero serial payload.
    skipped: u64,
    /// First decode-failure message seen for this class (root-cause sample).
    err_sample: Option<String>,
}

impl Coverage {
    fn total(&self) -> u64 {
        self.decoded + self.failed + self.skipped
    }
}

fn main() {
    let roots: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    if roots.is_empty() {
        eprintln!("usage: catalog <root> [<root> ...]");
        std::process::exit(64);
    }

    let mut by_class: BTreeMap<String, Coverage> = BTreeMap::new();
    let mut files = 0u64;
    let mut parse_ok = 0u64;
    // Keyed by a normalized failure signature -> (count, sample path + message).
    let mut parse_err: BTreeMap<String, (u64, String)> = BTreeMap::new();
    let mut no_primary = 0u64;

    let mut stack: Vec<PathBuf> = roots;
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| e.eq_ignore_ascii_case("uasset") || e.eq_ignore_ascii_case("umap"))
            {
                files += 1;
                catalog_file(
                    &path,
                    &mut by_class,
                    &mut parse_ok,
                    &mut parse_err,
                    &mut no_primary,
                );
            }
        }
    }

    let mut ranked: Vec<(String, Coverage)> = by_class.into_iter().collect();
    ranked.sort_by(|a, b| b.1.total().cmp(&a.1.total()).then(a.0.cmp(&b.0)));

    let dec: u64 = ranked.iter().map(|(_, c)| c.decoded).sum();
    let fail: u64 = ranked.iter().map(|(_, c)| c.failed).sum();
    let skip: u64 = ranked.iter().map(|(_, c)| c.skipped).sum();

    println!("# Primary asset class coverage");
    println!(
        "files: {files}, parsed ok: {parse_ok}, no primary: {no_primary} | \
         primaries decoded: {dec}, failed: {fail}, skipped: {skip}"
    );
    println!();
    println!("{:>6} {:>6} {:>6}  class", "total", "decode", "fail");
    for (class, cov) in &ranked {
        println!(
            "{:>6} {:>6} {:>6}  {class}",
            cov.total(),
            cov.decoded,
            cov.failed,
        );
    }

    println!();
    println!("# Decode-failure root-cause sample (top failing classes)");
    let mut failing: Vec<&(String, Coverage)> =
        ranked.iter().filter(|(_, c)| c.failed > 0).collect();
    failing.sort_by(|a, b| b.1.failed.cmp(&a.1.failed));
    for (class, cov) in failing.into_iter().take(25) {
        let sample = cov.err_sample.as_deref().unwrap_or("(none)");
        let sample: String = sample.chars().take(140).collect();
        println!("{:>6}x {class}\n        {sample}", cov.failed);
    }

    if !parse_err.is_empty() {
        let total: u64 = parse_err.values().map(|(c, _)| c).sum();
        println!();
        println!("# Package-layer parse failures ({total} files), grouped by cause");
        let mut errs: Vec<(String, (u64, String))> = parse_err.into_iter().collect();
        errs.sort_by(|a, b| b.1.0.cmp(&a.1.0));
        for (sig, (count, sample)) in errs {
            println!("{count:>6}x {sig}");
            println!("        e.g. {sample}");
        }
    }
}

/// Collapses a per-file error into a cause signature by masking file-specific
/// numbers (byte offsets, indices) so similar failures group together.
fn error_signature(detail: &str) -> String {
    let mut out = String::with_capacity(detail.len());
    let mut last_was_digit = false;
    for ch in detail.chars() {
        if ch.is_ascii_digit() {
            if !last_was_digit {
                out.push('#');
            }
            last_was_digit = true;
        } else {
            out.push(ch);
            last_was_digit = false;
        }
    }
    out
}

fn catalog_file(
    path: &Path,
    by_class: &mut BTreeMap<String, Coverage>,
    parse_ok: &mut u64,
    parse_err: &mut BTreeMap<String, (u64, String)>,
    no_primary: &mut u64,
) {
    let Ok(bytes) = fs::read(path) else {
        let entry = parse_err.entry("io error".to_string()).or_default();
        entry.0 += 1;
        return;
    };
    let package = match Package::parse(&bytes) {
        Ok(package) => package,
        Err(error) => {
            let sig = format!("{:?}: {}", error.kind(), error_signature(error.detail()));
            let entry = parse_err.entry(sig).or_default();
            entry.0 += 1;
            if entry.1.is_empty() {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("?");
                entry.1 = format!("{name}: {error}");
            }
            return;
        }
    };
    *parse_ok += 1;

    let schemas = EmptySchemas;
    let context = AssetDecodeContext {
        source: &bytes,
        package: &package,
        schemas: &schemas,
    };

    let mut counted = false;
    for export in &package.exports {
        let is_primary = export.is_asset == Some(true)
            || (export.is_asset.is_none() && matches!(export.outer_index, PackageIndex::Null));
        if !is_primary {
            continue;
        }
        counted = true;
        let class = export
            .class_path
            .as_ref()
            .map_or_else(|| "<unresolved>".to_string(), |c| c.as_str().to_string());
        let cov = by_class.entry(class).or_default();
        match decode_export(export, &context) {
            Ok(Some(_decoded)) => cov.decoded += 1,
            Ok(None) => cov.skipped += 1,
            Err(error) => {
                cov.failed += 1;
                if cov.err_sample.is_none() {
                    cov.err_sample = Some(format!("{:?}: {}", error.kind(), error));
                }
            }
        }
    }
    if !counted {
        *no_primary += 1;
    }
}
