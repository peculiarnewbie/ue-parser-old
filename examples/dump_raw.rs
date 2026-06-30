//! Throwaway: dump the name map + raw payload bytes for unsupported row props.
//! Usage: cargo run --example dump_raw -- <path.uasset> [Column]

use std::env;

use uasset_parser::Package;
use uasset_parser::asset::{AssetDecodeContext, DecodedAsset, decode_export};
use uasset_parser::package::ObjectPath;
use uasset_parser::property::PropertyValue;
use uasset_parser::schema::{ClassSchema, SchemaProvider, StructSchema};

struct EmptySchemas;
impl SchemaProvider for EmptySchemas {
    fn find_struct(&self, _p: &ObjectPath) -> Option<&StructSchema> {
        None
    }
    fn find_class(&self, _p: &ObjectPath) -> Option<&ClassSchema> {
        None
    }
}

fn main() {
    let path = env::args().nth(1).expect("need a .uasset path");
    let filter = env::args().nth(2);
    let bytes = std::fs::read(&path).expect("read file");
    let package = Package::parse(&bytes).expect("parse");

    println!(
        "--- soft object paths ({} entries @ offset {}) ---",
        package.soft_object_paths.len(),
        package
            .summary
            .soft_object_paths
            .as_ref()
            .map(|table| table.offset.get())
            .unwrap_or(0)
    );
    for (i, path) in package.soft_object_paths.iter().enumerate() {
        println!("  [{i}] {path:?}");
    }

    println!("--- name map ---");
    for (i, name) in package.names.iter().enumerate() {
        println!("  [{i}] {name}");
    }

    for export in &package.exports {
        let schemas = EmptySchemas;
        let context = AssetDecodeContext {
            source: &bytes,
            package: &package,
            schemas: &schemas,
        };
        let Some(decoded) = decode_export(export, &context).expect("decode export") else {
            continue;
        };
        match decoded {
            DecodedAsset::DataTable(datatable) => {
                for row in &datatable.rows {
                    dump_properties(
                        &package,
                        &bytes,
                        filter.as_deref(),
                        &package.resolve_name(row.name).unwrap_or_default(),
                        &row.properties.records,
                    );
                }
            }
            DecodedAsset::CurveTable(_) => {}
            DecodedAsset::StringTable(_) => {}
            DecodedAsset::Enum(decoded_enum) => {
                dump_properties(
                    &package,
                    &bytes,
                    filter.as_deref(),
                    "<root>",
                    &decoded_enum.properties.records,
                );
            }
            DecodedAsset::Struct(decoded_struct) => {
                dump_properties(
                    &package,
                    &bytes,
                    filter.as_deref(),
                    "<defaults>",
                    &decoded_struct.default_values.records,
                );
            }
            DecodedAsset::DataAsset(data_asset) => {
                dump_properties(
                    &package,
                    &bytes,
                    filter.as_deref(),
                    "<root>",
                    &data_asset.properties.records,
                );
            }
            DecodedAsset::UObject(object) => {
                dump_properties(
                    &package,
                    &bytes,
                    filter.as_deref(),
                    "<root>",
                    &object.properties.records,
                );
            }
            DecodedAsset::Skeleton(skeleton) => {
                dump_properties(
                    &package,
                    &bytes,
                    filter.as_deref(),
                    "<root>",
                    &skeleton.properties.records,
                );
            }
        }
    }
}

fn dump_properties(
    package: &Package,
    bytes: &[u8],
    filter: Option<&str>,
    scope: &str,
    records: &[uasset_parser::property::PropertyRecord],
) {
    for rec in records {
        let name = package.resolve_name(rec.name).unwrap_or_default();
        if filter.is_some_and(|f| f != name) {
            continue;
        }
        let ty = package.resolve_name(rec.type_name.name).unwrap_or_default();
        if matches!(rec.value, PropertyValue::Raw { .. }) || filter.is_some() {
            let raw = &bytes[rec.payload.offset() as usize..rec.payload.end() as usize];
            let hex: Vec<String> = raw.iter().map(|b| format!("{b:02x}")).collect();
            println!("[{scope}] {name} ({ty}) {}b: {}", raw.len(), hex.join(" "));
        }
    }
}
