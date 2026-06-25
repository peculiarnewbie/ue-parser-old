use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;
use serde_json::Value;
use uasset_parser::asset::{AssetDecodeContext, AssetDecoder, DataTableDecoder, DecodedAsset};
use uasset_parser::schema::{ClassSchema, SchemaProvider, StructSchema};
use uasset_parser::{Package, PackageSummary};

const CONTRACT_JSON: &str = include_str!("fixtures/electroswag-v7.json");

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

struct EmptySchemas;

impl SchemaProvider for EmptySchemas {
    fn find_struct(&self, _path: &uasset_parser::package::ObjectPath) -> Option<&StructSchema> {
        None
    }

    fn find_class(&self, _path: &uasset_parser::package::ObjectPath) -> Option<&ClassSchema> {
        None
    }
}

fn contract() -> FixtureContract {
    serde_json::from_str(CONTRACT_JSON).expect("fixture contract JSON must be valid")
}

fn fixture_root(contract: &FixtureContract) -> Option<PathBuf> {
    let configured = env::var_os("UASSET_FIXTURE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| contract.default_fixture_dir.clone());
    if configured.join("E2EFixtures.uproject").is_file() {
        Some(configured)
    } else if env::var_os("UASSET_REQUIRE_FIXTURE").is_some() {
        panic!(
            "{} fixture project not found at {}",
            contract.contract_version,
            configured.display()
        );
    } else {
        eprintln!(
            "skipping {} fixture validation; set UASSET_FIXTURE_DIR or UASSET_REQUIRE_FIXTURE=1",
            contract.contract_version
        );
        None
    }
}

fn asset_path(root: &Path, asset: &FixtureAsset) -> PathBuf {
    root.join(&asset.file)
}

#[test]
fn shared_fixture_corpus_matches_the_parser_contract() {
    let contract = contract();
    let Some(root) = fixture_root(&contract) else {
        return;
    };

    for asset in &contract.assets {
        let path = asset_path(&root, asset);
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let summary = PackageSummary::parse(&bytes)
            .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()));

        assert_eq!(
            summary.package_name,
            asset.package_name,
            "{}",
            path.display()
        );
        assert_eq!(
            summary.versions.legacy_file_version,
            contract.expected_versions.legacy_file,
            "{}",
            path.display()
        );
        assert_eq!(
            summary.versions.ue4,
            contract.expected_versions.ue4,
            "{}",
            path.display()
        );
        assert_eq!(
            summary.versions.ue5,
            contract.expected_versions.ue5,
            "{}",
            path.display()
        );
        assert_eq!(
            summary.versions.licensee,
            contract.expected_versions.licensee,
            "{}",
            path.display()
        );
        assert!(summary.names.count > 0, "{}", path.display());
        assert!(summary.exports.count > 0, "{}", path.display());
    }
}

#[test]
fn shared_fixture_corpus_resolves_datatable_exports() {
    let contract = contract();
    let Some(root) = fixture_root(&contract) else {
        return;
    };

    for asset in contract
        .assets
        .iter()
        .filter(|asset| asset.package_name.contains("/DT_"))
    {
        let path = asset_path(&root, asset);
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let package = Package::parse(&bytes)
            .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()));

        let datatable_export = package
            .exports
            .iter()
            .find(|export| {
                export
                    .class_path
                    .as_ref()
                    .is_some_and(|path| path.as_str() == "/Script/Engine.DataTable")
                    && export.object_path.as_str().starts_with(&asset.package_name)
            })
            .unwrap_or_else(|| {
                panic!(
                    "failed to find expected export {} in {}",
                    asset.package_name,
                    path.display()
                )
            });

        assert_eq!(
            datatable_export
                .class_path
                .as_ref()
                .map(|path| path.as_str()),
            Some("/Script/Engine.DataTable"),
            "{}",
            path.display()
        );

        let export_reader = package
            .export_reader(&bytes, datatable_export)
            .unwrap_or_else(|error| panic!("failed to bound export {}: {error}", path.display()));
        assert_eq!(
            export_reader.span().len(),
            datatable_export.serial_size,
            "{}",
            path.display()
        );
    }
}

#[test]
fn shared_fixture_datatables_decode_row_names() {
    let contract = contract();
    let Some(root) = fixture_root(&contract) else {
        return;
    };

    for asset in contract
        .assets
        .iter()
        .filter(|asset| asset.package_name.contains("/DT_"))
    {
        let path = asset_path(&root, asset);
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let package = Package::parse(&bytes)
            .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()));
        let datatable_export = package
            .exports
            .iter()
            .find(|export| {
                export
                    .class_path
                    .as_ref()
                    .is_some_and(|path| path.as_str() == "/Script/Engine.DataTable")
                    && export.object_path.as_str().starts_with(&asset.package_name)
            })
            .unwrap_or_else(|| {
                panic!(
                    "failed to find expected export {} in {}",
                    asset.package_name,
                    path.display()
                )
            });

        let schemas = EmptySchemas;
        let context = AssetDecodeContext {
            source: &bytes,
            package: &package,
            schemas: &schemas,
        };
        let DecodedAsset::DataTable(datatable) = DataTableDecoder
            .decode(datatable_export, &context)
            .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()));

        let Some(row_struct_path) = datatable.row_struct.as_ref() else {
            panic!("missing RowStruct for {}", path.display());
        };
        assert!(
            row_struct_path.as_str().contains("E2EFixture"),
            "{} resolved RowStruct to {}",
            path.display(),
            row_struct_path
        );

        for row in &datatable.rows {
            let row_name = package
                .resolve_name(row.name)
                .unwrap_or_else(|| panic!("failed to resolve row name for {}", path.display()));
            assert!(!row_name.is_empty(), "{}", path.display());
        }
    }
}

#[test]
fn shared_fixture_corpus_matches_the_spawned_cli_contract() {
    let contract = contract();
    let Some(root) = fixture_root(&contract) else {
        return;
    };

    for asset in &contract.assets {
        let path = asset_path(&root, asset);
        let output = Command::new(env!("CARGO_BIN_EXE_uasset"))
            .args(["inspect"])
            .arg(&path)
            .args(["--format", "json"])
            .output()
            .unwrap_or_else(|error| panic!("failed to spawn CLI for {}: {error}", path.display()));

        assert_eq!(
            output.status.code(),
            Some(0),
            "{}\nstderr: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty(), "{}", path.display());

        let json: Value = serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|error| panic!("invalid JSON for {}: {error}", path.display()));
        assert_eq!(json["schema_version"], 2, "{}", path.display());
        assert_eq!(json["status"], "ok", "{}", path.display());
        assert_eq!(
            json["package"]["name"],
            asset.package_name,
            "{}",
            path.display()
        );
        assert_eq!(
            json["package"]["version"]["ue5"],
            contract.expected_versions.ue5,
            "{}",
            path.display()
        );
    }
}
