use std::collections::BTreeMap;
use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;
use serde_json::Value;
use uasset_parser::asset::{
    AssetDecodeContext, AssetDecoder, DataAssetDecoder, DataTableDecoder, DataTableKind,
    DataTableRow, DecodedAsset, DecodedDataAsset, DecodedDataTable, DecodedUObject,
    COMPOSITE_DATATABLE_CLASS, DATATABLE_CLASS, decode_export,
};
use uasset_parser::package::{Package, PackageIndex};
use uasset_parser::property::{PropertyStream, PropertyValue};
use uasset_parser::schema::{ClassSchema, SchemaProvider, StructSchema};
use uasset_parser::{PackageSummary};

const CONTRACT_JSON: &str = include_str!("fixtures/electroswag-v13.json");

#[derive(Deserialize)]
struct FixtureContract {
    contract_version: String,
    default_fixture_dir: PathBuf,
    expected_versions: ExpectedVersions,
    assets: Vec<FixtureAsset>,
    #[serde(default)]
    datatables: Vec<FixtureDataTable>,
    #[serde(default)]
    data_assets: Vec<FixtureDataAsset>,
    #[serde(default)]
    uobjects: Vec<FixtureUObject>,
}

/// Parser-owned mirror of one `contract.ts` DataTable: object path, ordered row
/// names, the columns present in every row, and typed cell values.
#[derive(Deserialize)]
struct FixtureDataTable {
    file: PathBuf,
    object_path: String,
    row_struct: String,
    #[serde(default)]
    kind: String,
    #[serde(default)]
    parent_tables: Vec<String>,
    rows: Vec<String>,
    columns: Vec<String>,
    #[serde(default)]
    cells: Vec<ExpectedCell>,
}

/// Parser-owned mirror of one plain UObject export (Blueprint CDO or native UObject).
#[derive(Deserialize)]
struct FixtureUObject {
    file: PathBuf,
    object_path: String,
    class_path: String,
    columns: Vec<String>,
    #[serde(default)]
    cells: Vec<ExpectedUObjectCell>,
}

#[derive(Deserialize)]
struct ExpectedUObjectCell {
    column: String,
    value: ExpectedValue,
}

/// Parser-owned mirror of one `contract.ts` Data Asset.
#[derive(Deserialize)]
struct FixtureDataAsset {
    file: PathBuf,
    object_path: String,
    class_path: String,
    columns: Vec<String>,
    #[serde(default)]
    cells: Vec<ExpectedDataAssetCell>,
}

#[derive(Deserialize)]
struct ExpectedDataAssetCell {
    column: String,
    value: ExpectedValue,
}

#[derive(Deserialize)]
struct ExpectedCell {
    row: String,
    column: String,
    value: ExpectedValue,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ExpectedValue {
    Int(i64),
    Uint(u64),
    Float(f64),
    Double(f64),
    Bool(bool),
    String(String),
    Name(String),
    Enum(String),
    Text(String),
    Names(Vec<String>),
    Ints(Vec<i64>),
    Vector([f64; 3]),
    ObjectPath(String),
    SoftObjectPath(String),
    RowHandle(ExpectedRowHandle),
    StructFields(BTreeMap<String, ExpectedValue>),
    MapEntries(Vec<ExpectedMapEntry>),
    SetValues(Vec<ExpectedValue>),
    OneOf(Vec<ExpectedValue>),
}

#[derive(Debug, Deserialize)]
struct ExpectedMapEntry {
    key: ExpectedValue,
    value: ExpectedValue,
}

#[derive(Debug, Deserialize)]
struct ExpectedRowHandle {
    data_table: String,
    row_name: String,
}

impl ExpectedValue {
    fn matches(&self, package: &Package, actual: &PropertyValue) -> bool {
        match (self, actual) {
            (Self::Int(expected), PropertyValue::Int(actual)) => expected == actual,
            (Self::Uint(expected), PropertyValue::UInt(actual)) => expected == actual,
            (Self::Float(expected), PropertyValue::Float(actual)) => f64::from(*actual) == *expected,
            (Self::Double(expected), PropertyValue::Double(actual)) => *actual == *expected,
            (Self::Bool(expected), PropertyValue::Bool(actual)) => expected == actual,
            (Self::String(expected), PropertyValue::String(actual)) => expected == actual,
            (Self::Name(expected), PropertyValue::Name(name)) => {
                package.resolve_name(*name).as_deref() == Some(expected.as_str())
            }
            (Self::Enum(expected), PropertyValue::Enum(name)) => {
                package.resolve_name(*name).as_deref() == Some(expected.as_str())
            }
            (Self::Text(expected), PropertyValue::Text(text)) => expected == &text.source,
            (Self::Names(expected), PropertyValue::Array(actual)) => {
                if expected.len() != actual.len() {
                    return false;
                }
                actual.iter().zip(expected.iter()).all(|(value, expected_name)| {
                    matches!(
                        value,
                        PropertyValue::Name(name)
                            if package.resolve_name(*name).as_deref() == Some(expected_name.as_str())
                    )
                })
            }
            (Self::Ints(expected), PropertyValue::Array(actual)) => {
                if expected.len() != actual.len() {
                    return false;
                }
                actual.iter().zip(expected.iter()).all(|(value, expected_int)| {
                    matches!(value, PropertyValue::Int(actual) if actual == expected_int)
                })
            }
            (Self::Vector(expected), PropertyValue::Vector(actual)) => {
                f64::from(actual.x) == expected[0]
                    && f64::from(actual.y) == expected[1]
                    && f64::from(actual.z) == expected[2]
            }
            (Self::ObjectPath(expected), PropertyValue::ObjectRef(actual)) => package
                .resolve_index(*actual)
                .is_some_and(|path| path.as_str() == expected),
            (Self::SoftObjectPath(expected), PropertyValue::SoftObjectPath(actual)) => {
                expected == actual
            }
            (Self::RowHandle(expected), PropertyValue::Struct(stream)) => {
                row_handle_matches(package, stream, expected)
            }
            (Self::StructFields(expected), PropertyValue::Struct(stream)) => expected
                .iter()
                .all(|(field, expected_value)| {
                    struct_field(package, stream, field)
                        .is_some_and(|actual| expected_value.matches(package, actual))
                }),
            (Self::MapEntries(expected), PropertyValue::Map(actual)) => {
                expected.len() == actual.len()
                    && expected.iter().all(|expected_entry| {
                        actual.iter().any(|actual_entry| {
                            expected_entry.key.matches(package, &actual_entry.key)
                                && expected_entry.value.matches(package, &actual_entry.value)
                        })
                    })
            }
            (Self::SetValues(expected), PropertyValue::Set(actual)) => {
                expected.len() == actual.len()
                    && expected.iter().all(|expected_value| {
                        actual
                            .iter()
                            .any(|actual_value| expected_value.matches(package, actual_value))
                    })
            }
            (Self::OneOf(expected), actual) => expected
                .iter()
                .any(|expected_value| expected_value.matches(package, actual)),
            _ => false,
        }
    }
}

fn struct_field<'a>(
    package: &Package,
    stream: &'a PropertyStream,
    field: &str,
) -> Option<&'a PropertyValue> {
    stream.records.iter().find_map(|record| {
        (package.resolve_name(record.name).as_deref() == Some(field)).then_some(&record.value)
    })
}

fn row_handle_matches(
    package: &Package,
    stream: &PropertyStream,
    expected: &ExpectedRowHandle,
) -> bool {
    let Some(PropertyValue::ObjectRef(data_table)) = struct_field(package, stream, "DataTable")
    else {
        return false;
    };
    let Some(PropertyValue::Name(row_name)) = struct_field(package, stream, "RowName") else {
        return false;
    };
    let Some(resolved_table) = resolve_object_ref(package, *data_table) else {
        return false;
    };
    package.resolve_name(*row_name).as_deref() == Some(expected.row_name.as_str())
        && resolved_table == expected.data_table
}

fn resolve_object_ref(package: &Package, index: PackageIndex) -> Option<String> {
    if index == PackageIndex::Null {
        None
    } else {
        package.resolve_index(index).map(|path| path.to_string())
    }
}

fn contains_raw_value(value: &PropertyValue) -> bool {
    match value {
        PropertyValue::Raw { .. } => true,
        PropertyValue::Array(values) => values.iter().any(contains_raw_value),
        PropertyValue::Set(values) => values.iter().any(contains_raw_value),
        PropertyValue::Map(entries) => entries
            .iter()
            .any(|entry| contains_raw_value(&entry.key) || contains_raw_value(&entry.value)),
        PropertyValue::Struct(stream) => stream.records.iter().any(|record| {
            contains_raw_value(&record.value)
        }),
        _ => false,
    }
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

fn skip_missing_fixture_asset(root: &Path, file: &Path) -> bool {
    let path = root.join(file);
    if path.is_file() {
        return false;
    }
    eprintln!(
        "skipping {}; asset not on disk — run fixture bootstrap",
        path.display()
    );
    true
}

/// Reads, parses, and decodes a DataTable or CompositeDataTable export from a fixture
/// file, then runs `check` against the package and decoded table.
fn with_decoded_datatable(root: &Path, relative: &str, check: impl FnOnce(&Package, &DecodedDataTable)) {
    let path = root.join(relative);
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    let package = Package::parse(&bytes)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()));
    let export = package
        .exports
        .iter()
        .find(|export| {
            export.class_path.as_ref().is_some_and(|class| {
                matches!(
                    class.as_str(),
                    DATATABLE_CLASS | COMPOSITE_DATATABLE_CLASS
                )
            })
        })
        .unwrap_or_else(|| {
            panic!(
                "no DataTable or CompositeDataTable export in {}",
                path.display()
            )
        });
    let schemas = EmptySchemas;
    let context = AssetDecodeContext {
        source: &bytes,
        package: &package,
        schemas: &schemas,
    };
    let datatable = match DataTableDecoder
        .decode(export, &context)
        .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()))
    {
        DecodedAsset::DataTable(datatable) => datatable,
        DecodedAsset::DataAsset(_) => panic!("expected DataTable in {}", path.display()),
        DecodedAsset::UObject(_) => panic!("expected DataTable in {}", path.display()),
    };
    check(&package, &datatable);
}

fn with_decoded_data_asset(
    root: &Path,
    relative: &str,
    check: impl FnOnce(&Package, &DecodedDataAsset),
) {
    let path = root.join(relative);
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    let package = Package::parse(&bytes)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()));
    let export = package
        .exports
        .iter()
        .find(|export| {
            export.class_path.as_ref().is_some_and(|class| {
                uasset_parser::asset::is_data_asset_class(class.as_str())
            })
        })
        .unwrap_or_else(|| {
            panic!(
                "no DataAsset export in {}",
                path.display()
            )
        });
    let schemas = EmptySchemas;
    let context = AssetDecodeContext {
        source: &bytes,
        package: &package,
        schemas: &schemas,
    };
    let data_asset = match DataAssetDecoder
        .decode(export, &context)
        .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()))
    {
        DecodedAsset::DataAsset(data_asset) => data_asset,
        DecodedAsset::DataTable(_) => panic!("expected DataAsset in {}", path.display()),
        DecodedAsset::UObject(_) => panic!("expected DataAsset in {}", path.display()),
    };
    check(&package, &data_asset);
}

fn row<'a>(package: &Package, datatable: &'a DecodedDataTable, name: &str) -> &'a DataTableRow {
    datatable
        .rows
        .iter()
        .find(|row| package.resolve_name(row.name).as_deref() == Some(name))
        .unwrap_or_else(|| panic!("missing row {name}"))
}

fn cell(package: &Package, row: &DataTableRow, column: &str) -> PropertyValue {
    row.properties
        .records
        .iter()
        .find(|record| package.resolve_name(record.name).as_deref() == Some(column))
        .unwrap_or_else(|| panic!("missing column {column}"))
        .value
        .clone()
}

fn data_asset_property(package: &Package, data_asset: &DecodedDataAsset, column: &str) -> PropertyValue {
    data_asset
        .properties
        .records
        .iter()
        .find(|record| package.resolve_name(record.name).as_deref() == Some(column))
        .unwrap_or_else(|| panic!("missing column {column}"))
        .value
        .clone()
}

fn uobject_property(package: &Package, object: &DecodedUObject, column: &str) -> PropertyValue {
    object
        .properties
        .records
        .iter()
        .find(|record| package.resolve_name(record.name).as_deref() == Some(column))
        .unwrap_or_else(|| panic!("missing column {column}"))
        .value
        .clone()
}

fn find_uobject_export<'a>(package: &'a Package, expected: &FixtureUObject) -> &'a uasset_parser::package::Export {
    package
        .exports
        .iter()
        .find(|export| export.object_path.as_str() == expected.object_path)
        .or_else(|| {
            package.exports.iter().find(|export| {
                export.class_path.as_ref().is_some_and(|class| {
                    uasset_parser::asset::is_generic_uobject_class(class.as_str())
                        && class.as_str().contains(expected.class_path.as_str())
                })
            })
        })
        .unwrap_or_else(|| {
            panic!(
                "no generic UObject export matching {} in {:?}",
                expected.object_path,
                package
                    .exports
                    .iter()
                    .map(|export| (&export.object_path, &export.class_path))
                    .collect::<Vec<_>>()
            )
        })
}

fn with_decoded_uobject(
    root: &Path,
    expected: &FixtureUObject,
    check: impl FnOnce(&Package, &DecodedUObject),
) {
    let path = root.join(&expected.file);
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    let package = Package::parse(&bytes)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()));
    let export = find_uobject_export(&package, expected);
    let schemas = EmptySchemas;
    let context = AssetDecodeContext {
        source: &bytes,
        package: &package,
        schemas: &schemas,
    };
    let object = match decode_export(export, &context)
        .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()))
    {
        Some(DecodedAsset::UObject(object)) => object,
        Some(DecodedAsset::DataTable(_)) => panic!("expected UObject in {}", path.display()),
        Some(DecodedAsset::DataAsset(_)) => panic!("expected UObject in {}", path.display()),
        None => panic!("no decoder matched export in {}", path.display()),
    };
    check(&package, &object);
}

#[test]
fn shared_fixture_corpus_matches_the_parser_contract() {
    let contract = contract();
    let Some(root) = fixture_root(&contract) else {
        return;
    };

    for asset in &contract.assets {
        if skip_missing_fixture_asset(&root, &asset.file) {
            continue;
        }
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
fn shared_fixture_composite_datatable_resolves_export() {
    let contract = contract();
    let Some(root) = fixture_root(&contract) else {
        return;
    };

    let asset = contract
        .assets
        .iter()
        .find(|asset| asset.package_name.contains("CDT_E2EFixture"))
        .expect("CDT fixture asset");
    let path = asset_path(&root, asset);
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    let package = Package::parse(&bytes)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()));

    let composite_export = package
        .exports
        .iter()
        .find(|export| {
            export
                .class_path
                .as_ref()
                .is_some_and(|path| path.as_str() == COMPOSITE_DATATABLE_CLASS)
                && export.object_path.as_str().starts_with(&asset.package_name)
        })
        .unwrap_or_else(|| {
            panic!(
                "failed to find expected CompositeDataTable export {} in {}",
                asset.package_name,
                path.display()
            )
        });

    assert_eq!(
        composite_export
            .class_path
            .as_ref()
            .map(|path| path.as_str()),
        Some(COMPOSITE_DATATABLE_CLASS),
        "{}",
        path.display()
    );
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
        if skip_missing_fixture_asset(&root, &asset.file) {
            continue;
        }
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
        if skip_missing_fixture_asset(&root, &asset.file) {
            continue;
        }
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
        let datatable = match DataTableDecoder
            .decode(datatable_export, &context)
            .unwrap_or_else(|error| panic!("failed to decode {}: {error}", path.display()))
        {
            DecodedAsset::DataTable(datatable) => datatable,
            DecodedAsset::DataAsset(_) => panic!("expected DataTable in {}", path.display()),
        DecodedAsset::UObject(_) => panic!("expected DataTable in {}", path.display()),
        };

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
fn shared_fixture_datatables_match_contract_mirror() {
    let contract = contract();
    let Some(root) = fixture_root(&contract) else {
        return;
    };
    assert!(
        !contract.datatables.is_empty(),
        "contract mirror must define DataTables"
    );

    for table in &contract.datatables {
        if skip_missing_fixture_asset(&root, &table.file) {
            continue;
        }
        let relative = table.file.to_string_lossy().replace('\\', "/");
        with_decoded_datatable(&root, &relative, |package, datatable| {
            let context = &table.object_path;

            assert_eq!(
                datatable.object_path.as_str(),
                table.object_path,
                "object path for {context}"
            );

            let expected_kind = if table.kind.is_empty() {
                "plain"
            } else {
                table.kind.as_str()
            };
            let actual_kind = match datatable.kind {
                DataTableKind::Plain => "plain",
                DataTableKind::Composite => "composite",
            };
            assert_eq!(actual_kind, expected_kind, "kind for {context}");

            if !table.parent_tables.is_empty() {
                let resolved_parents: Vec<String> = datatable
                    .parent_tables
                    .iter()
                    .map(|path| path.to_string())
                    .collect();
                assert_eq!(
                    resolved_parents, table.parent_tables,
                    "parent tables for {context}"
                );
            }

            let row_struct = datatable
                .row_struct
                .as_ref()
                .unwrap_or_else(|| panic!("missing RowStruct for {context}"));
            assert!(
                row_struct.as_str().contains(&table.row_struct),
                "{context} resolved RowStruct {row_struct}, expected to contain {}",
                table.row_struct
            );

            // Row names must match the contract exactly, in order.
            let decoded_rows: Vec<String> = datatable
                .rows
                .iter()
                .map(|row| {
                    package
                        .resolve_name(row.name)
                        .unwrap_or_else(|| panic!("unresolved row name in {context}"))
                })
                .collect();
            assert_eq!(decoded_rows, table.rows, "row names for {context}");

            // Every contract column must be present as a property in every row.
            for decoded_row in &datatable.rows {
                let row_name = package.resolve_name(decoded_row.name).unwrap_or_default();
                let columns: Vec<String> = decoded_row
                    .properties
                    .records
                    .iter()
                    .filter_map(|record| package.resolve_name(record.name))
                    .collect();
                for column in &table.columns {
                    assert!(
                        columns.contains(column),
                        "{context} row {row_name} missing column {column}; found {columns:?}"
                    );
                }
            }

            // Typed cell values must decode to the contract-mirrored values.
            for expected in &table.cells {
                let target = row(package, datatable, &expected.row);
                let actual = cell(package, target, &expected.column);
                assert!(
                    expected.value.matches(package, &actual),
                    "{context} {}.{} expected {:?}, decoded {actual:?}",
                    expected.row,
                    expected.column,
                    expected.value
                );
            }
        });
    }
}

#[test]
fn shared_fixture_datatables_decode_without_raw_properties() {
    let contract = contract();
    let Some(root) = fixture_root(&contract) else {
        return;
    };

    for table in &contract.datatables {
        if skip_missing_fixture_asset(&root, &table.file) {
            continue;
        }
        let relative = table.file.to_string_lossy().replace('\\', "/");
        with_decoded_datatable(&root, &relative, |package, datatable| {
            let context = &table.object_path;
            for decoded_row in &datatable.rows {
                let row_name = package.resolve_name(decoded_row.name).unwrap_or_default();
                for record in &decoded_row.properties.records {
                    let column = package
                        .resolve_name(record.name)
                        .unwrap_or_else(|| "<unresolved>".to_owned());
                    assert!(
                        !contains_raw_value(&record.value),
                        "{context} row {row_name} column {column} decoded as raw: {:?}",
                        record.value
                    );
                }
            }
        });
    }
}

#[test]
fn shared_fixture_data_assets_match_contract_mirror() {
    let contract = contract();
    let Some(root) = fixture_root(&contract) else {
        return;
    };
    assert!(
        !contract.data_assets.is_empty(),
        "contract mirror must define Data Assets"
    );

    for asset in &contract.data_assets {
        if skip_missing_fixture_asset(&root, &asset.file) {
            continue;
        }
        let relative = asset.file.to_string_lossy().replace('\\', "/");
        with_decoded_data_asset(&root, &relative, |package, data_asset| {
            let context = &asset.object_path;

            assert_eq!(
                data_asset.object_path.as_str(),
                asset.object_path,
                "object path for {context}"
            );
            assert!(
                data_asset.class_path.as_str().contains(&asset.class_path),
                "{context} resolved class {}, expected to contain {}",
                data_asset.class_path,
                asset.class_path
            );

            let columns: Vec<String> = data_asset
                .properties
                .records
                .iter()
                .filter_map(|record| package.resolve_name(record.name))
                .collect();
            for column in &asset.columns {
                if asset.cells.iter().any(|cell| cell.column == *column) {
                    assert!(
                        columns.contains(column),
                        "{context} missing column {column}; found {columns:?}"
                    );
                }
            }

            for expected in &asset.cells {
                let actual = data_asset_property(package, data_asset, &expected.column);
                assert!(
                    expected.value.matches(package, &actual),
                    "{context} {} expected {:?}, decoded {actual:?}",
                    expected.column,
                    expected.value
                );
            }
        });
    }
}

#[test]
fn shared_fixture_data_assets_decode_without_raw_properties() {
    let contract = contract();
    let Some(root) = fixture_root(&contract) else {
        return;
    };

    for asset in &contract.data_assets {
        if skip_missing_fixture_asset(&root, &asset.file) {
            continue;
        }
        let relative = asset.file.to_string_lossy().replace('\\', "/");
        with_decoded_data_asset(&root, &relative, |package, data_asset| {
            let context = &asset.object_path;
            for record in &data_asset.properties.records {
                let column = package
                    .resolve_name(record.name)
                    .unwrap_or_else(|| "<unresolved>".to_owned());
                assert!(
                    !contains_raw_value(&record.value),
                    "{context} column {column} decoded as raw: {:?}",
                    record.value
                );
            }
        });
    }
}

#[test]
fn shared_fixture_uobjects_match_contract_mirror() {
    let contract = contract();
    let Some(root) = fixture_root(&contract) else {
        return;
    };
    assert!(
        !contract.uobjects.is_empty(),
        "contract mirror must define plain UObject fixtures"
    );

    for asset in &contract.uobjects {
        if skip_missing_fixture_asset(&root, &asset.file) {
            continue;
        }
        with_decoded_uobject(&root, asset, |package, object| {
            let context = &asset.object_path;

            assert_eq!(
                object.object_path.as_str(),
                asset.object_path,
                "object path for {context}"
            );
            assert!(
                object.class_path.as_str().contains(&asset.class_path),
                "{context} resolved class {}, expected to contain {}",
                object.class_path,
                asset.class_path
            );

            let columns: Vec<String> = object
                .properties
                .records
                .iter()
                .filter_map(|record| package.resolve_name(record.name))
                .collect();
            for column in &asset.columns {
                if asset.cells.iter().any(|cell| cell.column == *column) {
                    assert!(
                        columns.contains(column),
                        "{context} missing column {column}; found {columns:?}"
                    );
                }
            }

            for expected in &asset.cells {
                let actual = uobject_property(package, object, &expected.column);
                assert!(
                    expected.value.matches(package, &actual),
                    "{context} {} expected {:?}, decoded {actual:?}",
                    expected.column,
                    expected.value
                );
            }
        });
    }
}

#[test]
fn shared_fixture_uobjects_decode_without_raw_properties() {
    let contract = contract();
    let Some(root) = fixture_root(&contract) else {
        return;
    };

    for asset in &contract.uobjects {
        if skip_missing_fixture_asset(&root, &asset.file) {
            continue;
        }
        with_decoded_uobject(&root, asset, |package, object| {
            let context = &asset.object_path;
            for record in &object.properties.records {
                let column = package
                    .resolve_name(record.name)
                    .unwrap_or_else(|| "<unresolved>".to_owned());
                assert!(
                    !contains_raw_value(&record.value),
                    "{context} column {column} decoded as raw: {:?}",
                    record.value
                );
            }
        });
    }
}

#[test]
fn shared_fixture_corpus_matches_the_spawned_cli_contract() {
    let contract = contract();
    let Some(root) = fixture_root(&contract) else {
        return;
    };

    for asset in &contract.assets {
        if skip_missing_fixture_asset(&root, &asset.file) {
            continue;
        }
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
