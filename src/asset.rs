//! Asset-level decoders.

use std::fmt;

use crate::archive::{ArchiveError, ArchiveErrorKind, Guid, NameRef};
use crate::codec::{DecodeContext, decode_property_stream_values};
use crate::package::{Export, ObjectPath, Package, PackageError, PackageIndex};
use crate::property::{
    PropertyError, PropertyErrorKind, PropertyStream, PropertyValue, read_tagged_property_stream,
    read_uobject_tagged_property_stream,
};
use crate::schema::SchemaProvider;

pub struct AssetDecodeContext<'a> {
    pub source: &'a [u8],
    pub package: &'a Package,
    pub schemas: &'a dyn SchemaProvider,
}

pub const DATATABLE_CLASS: &str = "/Script/Engine.DataTable";
pub const COMPOSITE_DATATABLE_CLASS: &str = "/Script/Engine.CompositeDataTable";
pub const CURVETABLE_CLASS: &str = "/Script/Engine.CurveTable";
pub const DATA_ASSET_CLASS: &str = "/Script/Engine.DataAsset";
pub const PRIMARY_DATA_ASSET_CLASS: &str = "/Script/Engine.PrimaryDataAsset";

/// Package/meta exports that share the package file but are not inspectable assets.
const SKIP_UOBJECT_DECODE_CLASSES: &[&str] = &[
    "/Script/CoreUObject.Package",
    "/Script/CoreUObject.MetaData",
    "/Script/Engine.AssetImportData",
];

/// Returns whether `class_path` names a UObject Data Asset type.
///
/// Matches engine base classes and native subclasses whose UClass name ends in
/// `DataAsset` (for example `/Script/E2EFixtures.E2EFixtureScalarsDataAsset`).
pub fn is_data_asset_class(class_path: &str) -> bool {
    matches!(class_path, DATA_ASSET_CLASS | PRIMARY_DATA_ASSET_CLASS)
        || class_path
            .rsplit('.')
            .next()
            .is_some_and(|class_name| class_name.ends_with("DataAsset"))
}

/// Returns whether `class_path` should use the generic UObject property decoder.
pub fn is_generic_uobject_class(class_path: &str) -> bool {
    !DataTableDecoder::supports_class(class_path)
        && class_path != CURVETABLE_CLASS
        && !is_data_asset_class(class_path)
        && !SKIP_UOBJECT_DECODE_CLASSES.contains(&class_path)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataTableKind {
    Plain,
    Composite,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DecodedAsset {
    DataTable(DecodedDataTable),
    CurveTable(DecodedCurveTable),
    DataAsset(DecodedDataAsset),
    UObject(DecodedUObject),
}

#[derive(Clone, Debug, PartialEq)]
pub struct DecodedDataAsset {
    pub object_path: ObjectPath,
    pub class_path: ObjectPath,
    pub object_guid: Option<Guid>,
    pub properties: PropertyStream,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DecodedUObject {
    pub object_path: ObjectPath,
    pub class_path: ObjectPath,
    pub object_guid: Option<Guid>,
    pub properties: PropertyStream,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DecodedDataTable {
    pub kind: DataTableKind,
    pub object_path: ObjectPath,
    pub row_struct: Option<ObjectPath>,
    pub parent_tables: Vec<ObjectPath>,
    pub properties: PropertyStream,
    pub rows: Vec<DataTableRow>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DataTableRow {
    pub name: NameRef,
    pub properties: PropertyStream,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DecodedCurveTable {
    pub object_path: ObjectPath,
    pub mode: CurveTableMode,
    pub properties: PropertyStream,
    pub rows: Vec<CurveTableRow>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CurveTableMode {
    Empty,
    SimpleCurves,
    RichCurves,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CurveTableRow {
    pub name: NameRef,
    pub keys: Vec<SimpleCurveKey>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SimpleCurveKey {
    pub time: f32,
    pub value: f32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AssetErrorKind {
    MalformedData,
    UnsupportedFormat,
    UnsupportedVersion,
    UnsupportedCapability,
}

#[derive(Debug)]
pub struct AssetError {
    kind: AssetErrorKind,
    message: String,
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl AssetError {
    fn new(kind: AssetErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            source: None,
        }
    }

    #[must_use]
    pub const fn kind(&self) -> AssetErrorKind {
        self.kind
    }

    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for AssetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{:?}: {}", self.kind, self.message)
    }
}

impl std::error::Error for AssetError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|source| source.as_ref() as &(dyn std::error::Error + 'static))
    }
}

impl From<PackageError> for AssetError {
    fn from(source: PackageError) -> Self {
        let kind = match source.kind() {
            crate::package::PackageErrorKind::MalformedData => AssetErrorKind::MalformedData,
            crate::package::PackageErrorKind::UnsupportedFormat => {
                AssetErrorKind::UnsupportedFormat
            }
            crate::package::PackageErrorKind::UnsupportedVersion => {
                AssetErrorKind::UnsupportedVersion
            }
            crate::package::PackageErrorKind::UnsupportedCapability => {
                AssetErrorKind::UnsupportedCapability
            }
        };
        Self {
            kind,
            message: source.to_string(),
            source: Some(Box::new(source)),
        }
    }
}

impl From<PropertyError> for AssetError {
    fn from(source: PropertyError) -> Self {
        let kind = match source.kind() {
            PropertyErrorKind::MalformedData => AssetErrorKind::MalformedData,
            PropertyErrorKind::UnsupportedVersion => AssetErrorKind::UnsupportedVersion,
            PropertyErrorKind::UnsupportedCapability => AssetErrorKind::UnsupportedCapability,
        };
        Self {
            kind,
            message: source.to_string(),
            source: Some(Box::new(source)),
        }
    }
}

impl From<ArchiveError> for AssetError {
    fn from(source: ArchiveError) -> Self {
        let kind = match source.kind() {
            ArchiveErrorKind::OutOfBounds
            | ArchiveErrorKind::InvalidSeek
            | ArchiveErrorKind::InvalidCount
            | ArchiveErrorKind::AllocationLimit
            | ArchiveErrorKind::MissingNullTerminator
            | ArchiveErrorKind::InvalidString
            | ArchiveErrorKind::InvalidNameReference
            | ArchiveErrorKind::IntegerOverflow => AssetErrorKind::MalformedData,
        };
        Self {
            kind,
            message: source.to_string(),
            source: Some(Box::new(source)),
        }
    }
}

pub trait AssetDecoder {
    fn supports(&self, class_path: &ObjectPath) -> bool;

    fn decode(
        &self,
        export: &Export,
        context: &AssetDecodeContext<'_>,
    ) -> Result<DecodedAsset, AssetError>;
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DataTableDecoder;

impl AssetDecoder for DataTableDecoder {
    fn supports(&self, class_path: &ObjectPath) -> bool {
        Self::supports_class(class_path.as_str())
    }

    fn decode(
        &self,
        export: &Export,
        context: &AssetDecodeContext<'_>,
    ) -> Result<DecodedAsset, AssetError> {
        let Some(class_path) = export.class_path.as_ref() else {
            return Err(AssetError::new(
                AssetErrorKind::UnsupportedFormat,
                format!("export {} has no resolved class", export.object_path),
            ));
        };
        let kind = match class_path.as_str() {
            DATATABLE_CLASS => DataTableKind::Plain,
            COMPOSITE_DATATABLE_CLASS => DataTableKind::Composite,
            _ => {
                return Err(AssetError::new(
                    AssetErrorKind::UnsupportedFormat,
                    format!("unsupported asset class {class_path}"),
                ));
            }
        };

        let (properties, mut reader) = decode_uobject_properties(export, context)?;
        let decode_context = DecodeContext {
            package: context.package,
            versions: &context.package.summary.versions,
            schemas: context.schemas,
        };
        let row_struct = row_struct_path(context.package, &properties);
        let parent_tables = parent_tables_paths(context.package, &properties);

        let data_marker_offset = reader.tell();
        let data_marker = reader.read_i32(&format!("{}.Data.Marker", export.object_path))?;
        if data_marker != 0 {
            return Err(AssetError::new(
                AssetErrorKind::MalformedData,
                format!(
                    "expected DataTable data marker 0 at byte {data_marker_offset}, got {data_marker}"
                ),
            ));
        }

        let row_count_offset = reader.tell();
        let row_count = reader.read_i32(&format!("{}.Rows.Count", export.object_path))?;
        if row_count < 0 {
            return Err(AssetError::new(
                AssetErrorKind::MalformedData,
                format!("negative DataTable row count {row_count} at byte {row_count_offset}"),
            ));
        }
        let mut rows = Vec::with_capacity(usize::try_from(row_count).expect("i32 fits in usize"));
        for index in 0..row_count {
            let row_path = format!("{}.Rows[{index}]", export.object_path);
            let name = reader.read_name_ref(&format!("{row_path}.Name"))?;
            let mut row_properties = read_tagged_property_stream(
                &mut reader,
                &context.package.summary.versions,
                &context.package.names,
                &format!("{row_path}.Value"),
            )?;
            decode_property_stream_values(context.source, &mut row_properties, &decode_context)?;
            rows.push(DataTableRow {
                name,
                properties: row_properties,
            });
        }

        if reader.remaining() != 0 {
            return Err(AssetError::new(
                AssetErrorKind::MalformedData,
                format!(
                    "DataTable export {} left {} trailing bytes",
                    export.object_path,
                    reader.remaining()
                ),
            ));
        }

        Ok(DecodedAsset::DataTable(DecodedDataTable {
            kind,
            object_path: export.object_path.clone(),
            row_struct,
            parent_tables,
            properties,
            rows,
        }))
    }
}

impl DataTableDecoder {
    fn supports_class(class_path: &str) -> bool {
        matches!(class_path, DATATABLE_CLASS | COMPOSITE_DATATABLE_CLASS)
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CurveTableDecoder;

impl AssetDecoder for CurveTableDecoder {
    fn supports(&self, class_path: &ObjectPath) -> bool {
        class_path.as_str() == CURVETABLE_CLASS
    }

    fn decode(
        &self,
        export: &Export,
        context: &AssetDecodeContext<'_>,
    ) -> Result<DecodedAsset, AssetError> {
        let Some(class_path) = export.class_path.as_ref() else {
            return Err(AssetError::new(
                AssetErrorKind::UnsupportedFormat,
                format!("export {} has no resolved class", export.object_path),
            ));
        };
        if class_path.as_str() != CURVETABLE_CLASS {
            return Err(AssetError::new(
                AssetErrorKind::UnsupportedFormat,
                format!("unsupported asset class {class_path}"),
            ));
        }

        let (properties, mut reader) = decode_uobject_properties(export, context)?;
        let footer_offset = reader.tell();
        let footer = reader.read_i32(&format!("{}.ExportFooter", export.object_path))?;
        if footer != 0 {
            return Err(AssetError::new(
                AssetErrorKind::MalformedData,
                format!(
                    "expected zero CurveTable UObject footer at byte {footer_offset}, got {footer}"
                ),
            ));
        }

        let row_count_offset = reader.tell();
        let row_count = reader.read_i32(&format!("{}.Rows.Count", export.object_path))?;
        if row_count < 0 {
            return Err(AssetError::new(
                AssetErrorKind::MalformedData,
                format!("negative CurveTable row count {row_count} at byte {row_count_offset}"),
            ));
        }

        let raw_mode = reader.read_u8(&format!("{}.Mode", export.object_path))?;
        let mode = match raw_mode {
            0 => CurveTableMode::Empty,
            1 => CurveTableMode::SimpleCurves,
            2 => CurveTableMode::RichCurves,
            value => {
                return Err(AssetError::new(
                    AssetErrorKind::MalformedData,
                    format!("unsupported CurveTable mode {value}"),
                ));
            }
        };
        if mode != CurveTableMode::SimpleCurves && row_count > 0 {
            return Err(AssetError::new(
                AssetErrorKind::UnsupportedCapability,
                format!("CurveTable mode {mode:?} is not supported yet"),
            ));
        }

        let mut rows = Vec::with_capacity(usize::try_from(row_count).expect("i32 fits in usize"));
        for index in 0..row_count {
            let row_path = format!("{}.Rows[{index}]", export.object_path);
            let name = reader.read_name_ref(&format!("{row_path}.Name"))?;
            let stream = read_tagged_property_stream(
                &mut reader,
                &context.package.summary.versions,
                &context.package.names,
                &format!("{row_path}.Curve"),
            )?;
            let keys =
                decode_simple_curve_keys(context.source, context.package, &stream, &row_path)?;
            rows.push(CurveTableRow { name, keys });
        }

        if reader.remaining() != 0 {
            return Err(AssetError::new(
                AssetErrorKind::MalformedData,
                format!(
                    "CurveTable export {} left {} trailing bytes",
                    export.object_path,
                    reader.remaining()
                ),
            ));
        }

        Ok(DecodedAsset::CurveTable(DecodedCurveTable {
            object_path: export.object_path.clone(),
            mode,
            properties,
            rows,
        }))
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct DataAssetDecoder;

impl AssetDecoder for DataAssetDecoder {
    fn supports(&self, class_path: &ObjectPath) -> bool {
        is_data_asset_class(class_path.as_str())
    }

    fn decode(
        &self,
        export: &Export,
        context: &AssetDecodeContext<'_>,
    ) -> Result<DecodedAsset, AssetError> {
        let Some(class_path) = export.class_path.as_ref() else {
            return Err(AssetError::new(
                AssetErrorKind::UnsupportedFormat,
                format!("export {} has no resolved class", export.object_path),
            ));
        };
        if !is_data_asset_class(class_path.as_str()) {
            return Err(AssetError::new(
                AssetErrorKind::UnsupportedFormat,
                format!("unsupported asset class {class_path}"),
            ));
        }

        let (properties, class_path, object_guid) =
            decode_uobject_asset_properties(export, context)?;

        Ok(DecodedAsset::DataAsset(DecodedDataAsset {
            object_path: export.object_path.clone(),
            class_path,
            object_guid,
            properties,
        }))
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct UObjectDecoder;

impl AssetDecoder for UObjectDecoder {
    fn supports(&self, class_path: &ObjectPath) -> bool {
        is_generic_uobject_class(class_path.as_str())
    }

    fn decode(
        &self,
        export: &Export,
        context: &AssetDecodeContext<'_>,
    ) -> Result<DecodedAsset, AssetError> {
        let Some(class_path) = export.class_path.as_ref() else {
            return Err(AssetError::new(
                AssetErrorKind::UnsupportedFormat,
                format!("export {} has no resolved class", export.object_path),
            ));
        };
        if !is_generic_uobject_class(class_path.as_str()) {
            return Err(AssetError::new(
                AssetErrorKind::UnsupportedFormat,
                format!("unsupported asset class {class_path}"),
            ));
        }

        let (properties, class_path, object_guid) =
            decode_uobject_asset_properties(export, context)?;

        Ok(DecodedAsset::UObject(DecodedUObject {
            object_path: export.object_path.clone(),
            class_path,
            object_guid,
            properties,
        }))
    }
}

/// Attempts to decode one export with the first matching asset adapter.
///
/// Returns `Ok(None)` when the export has no class, zero serial payload, or no
/// adapter applies. Returns an error when a matching adapter rejects malformed
/// payload data.
pub fn decode_export(
    export: &Export,
    context: &AssetDecodeContext<'_>,
) -> Result<Option<DecodedAsset>, AssetError> {
    if export.serial_size == 0 {
        return Ok(None);
    }
    let Some(class_path) = export.class_path.as_ref() else {
        return Ok(None);
    };

    if DataTableDecoder.supports(class_path) {
        return DataTableDecoder.decode(export, context).map(Some);
    }
    if CurveTableDecoder.supports(class_path) {
        return CurveTableDecoder.decode(export, context).map(Some);
    }
    if DataAssetDecoder.supports(class_path) {
        return DataAssetDecoder.decode(export, context).map(Some);
    }
    if UObjectDecoder.supports(class_path) {
        return UObjectDecoder.decode(export, context).map(Some);
    }
    Ok(None)
}

fn decode_uobject_asset_properties(
    export: &Export,
    context: &AssetDecodeContext<'_>,
) -> Result<(PropertyStream, ObjectPath, Option<Guid>), AssetError> {
    let class_path = export.class_path.clone().ok_or_else(|| {
        AssetError::new(
            AssetErrorKind::UnsupportedFormat,
            format!("export {} has no resolved class", export.object_path),
        )
    })?;
    let (properties, mut reader) = decode_uobject_properties(export, context)?;
    let object_guid = consume_uobject_export_footer(&mut reader, &export.object_path)?;
    Ok((properties, class_path, object_guid))
}

fn decode_uobject_properties<'a>(
    export: &'a Export,
    context: &'a AssetDecodeContext<'a>,
) -> Result<(PropertyStream, crate::archive::Reader<'a>), AssetError> {
    let mut reader = context.package.export_reader(context.source, export)?;
    let mut properties = read_uobject_tagged_property_stream(
        &mut reader,
        &context.package.summary.versions,
        &context.package.names,
        export.object_path.as_str(),
    )?;
    let decode_context = DecodeContext {
        package: context.package,
        versions: &context.package.summary.versions,
        schemas: context.schemas,
    };
    decode_property_stream_values(context.source, &mut properties, &decode_context)?;
    Ok((properties, reader))
}

/// UE5 editor exports may append a zero `i32` object-guid slot after tagged
/// properties (see `FLazyObjectPtr::PossiblySerializeObjectGuid`).
fn consume_uobject_export_footer(
    reader: &mut crate::archive::Reader<'_>,
    object_path: &ObjectPath,
) -> Result<Option<Guid>, AssetError> {
    if reader.remaining() == 4 {
        let offset = reader.tell();
        let footer = reader
            .read_i32(&format!("{object_path}.ExportFooter"))
            .map_err(AssetError::from)?;
        if footer != 0 {
            return Err(AssetError::new(
                AssetErrorKind::MalformedData,
                format!("expected zero UObject export footer at byte {offset}, got {footer}"),
            ));
        }
        return Ok(None);
    }
    if reader.remaining() == 20 {
        let offset = reader.tell();
        let has_guid = reader
            .read_i32(&format!("{object_path}.ExportFooter.HasObjectGuid"))
            .map_err(AssetError::from)?;
        if has_guid != 1 {
            return Err(AssetError::new(
                AssetErrorKind::MalformedData,
                format!(
                    "expected UObject export footer object-guid marker 1 at byte {offset}, got {has_guid}"
                ),
            ));
        }
        let guid = reader
            .read_guid(&format!("{object_path}.ExportFooter.ObjectGuid"))
            .map_err(AssetError::from)?;
        return Ok(Some(guid));
    }
    if reader.remaining() != 0 {
        return Err(AssetError::new(
            AssetErrorKind::MalformedData,
            format!(
                "UObject export {object_path} left {} trailing bytes",
                reader.remaining()
            ),
        ));
    }
    Ok(None)
}

fn decode_simple_curve_keys(
    source: &[u8],
    package: &Package,
    stream: &PropertyStream,
    path: &str,
) -> Result<Vec<SimpleCurveKey>, AssetError> {
    let Some(record) = stream
        .records
        .iter()
        .find(|record| package.resolve_name(record.name).as_deref() == Some("Keys"))
    else {
        return Ok(Vec::new());
    };
    if package.resolve_name(record.type_name.name).as_deref() != Some("ArrayProperty") {
        return Err(AssetError::new(
            AssetErrorKind::MalformedData,
            format!("{path}.Keys is not an ArrayProperty"),
        ));
    }

    let reader = crate::archive::Reader::new(source);
    let mut payload = reader
        .bounded(record.payload, &format!("{path}.Keys.Payload"))
        .map_err(AssetError::from)?;
    let count = payload.read_i32(&format!("{path}.Keys.Count"))?;
    if count < 0 {
        return Err(AssetError::new(
            AssetErrorKind::MalformedData,
            format!("negative SimpleCurve key count {count}"),
        ));
    }
    let mut keys = Vec::with_capacity(usize::try_from(count).expect("i32 fits in usize"));
    for index in 0..count {
        keys.push(SimpleCurveKey {
            time: payload.read_f32(&format!("{path}.Keys[{index}].Time"))?,
            value: payload.read_f32(&format!("{path}.Keys[{index}].Value"))?,
        });
    }
    if payload.remaining() != 0 {
        return Err(AssetError::new(
            AssetErrorKind::MalformedData,
            format!(
                "{path}.Keys left {} trailing bytes after SimpleCurve key decode",
                payload.remaining()
            ),
        ));
    }
    Ok(keys)
}

fn parent_tables_paths(package: &Package, properties: &PropertyStream) -> Vec<ObjectPath> {
    let Some(record) = properties
        .records
        .iter()
        .find(|record| package.resolve_name(record.name).as_deref() == Some("ParentTables"))
    else {
        return Vec::new();
    };
    let PropertyValue::Array(entries) = &record.value else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|value| {
            let PropertyValue::ObjectRef(index) = value else {
                return None;
            };
            if *index == PackageIndex::Null {
                None
            } else {
                package.resolve_index(*index)
            }
        })
        .collect()
}

fn row_struct_path(package: &Package, properties: &PropertyStream) -> Option<ObjectPath> {
    let record = properties
        .records
        .iter()
        .find(|record| package.resolve_name(record.name).as_deref() == Some("RowStruct"))?;
    let PropertyValue::ObjectRef(index) = record.value else {
        return None;
    };
    if index == PackageIndex::Null {
        None
    } else {
        package.resolve_index(index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::{test_export, test_import, test_package};
    use crate::property::PropertyValue;
    use crate::schema::{ClassSchema, SchemaProvider, StructSchema};
    use crate::test_support::{
        name_ref, write_datatable_export, write_int_property_tag, write_object_array_property_tag,
        write_object_property_tag, write_uobject_export,
    };

    struct EmptySchemas;

    impl SchemaProvider for EmptySchemas {
        fn find_struct(&self, _path: &ObjectPath) -> Option<&StructSchema> {
            None
        }

        fn find_class(&self, _path: &ObjectPath) -> Option<&ClassSchema> {
            None
        }
    }

    fn names() -> Vec<String> {
        vec![
            "None".into(),
            "IntProperty".into(),
            "IntValue".into(),
            "Row_Alpha".into(),
            "ObjectProperty".into(),
            "RowStruct".into(),
            "E2EFixtureScalarsRow".into(),
            "Script/E2EFixtures".into(),
            "ArrayProperty".into(),
            "ParentTables".into(),
            "DT_Scalars".into(),
            "DT_Scalars2".into(),
            "Game/E2EFixture/Data".into(),
        ]
    }

    fn decode_datatable(
        export_bytes: Vec<u8>,
        package: Package,
        export: Export,
    ) -> DecodedDataTable {
        let schemas = EmptySchemas;
        let context = AssetDecodeContext {
            source: &export_bytes,
            package: &package,
            schemas: &schemas,
        };
        let DecodedAsset::DataTable(datatable) = DataTableDecoder
            .decode(&export, &context)
            .expect("decode datatable")
        else {
            panic!("expected DataTable decode");
        };
        datatable
    }

    #[test]
    fn decodes_minimal_datatable_with_one_scalar_row() {
        let mut row_properties = Vec::new();
        write_int_property_tag(&mut row_properties, 2, 1, 4243);
        let export_bytes = write_datatable_export(0, &[], &[(3, row_properties.as_slice())]);
        let package = test_package(names());
        let export = test_export(
            export_bytes.len() as u64,
            "/Game/Test/DT_Test.DT_Test",
            "/Script/Engine.DataTable",
        );

        let datatable = decode_datatable(export_bytes, package, export);

        assert_eq!(datatable.kind, DataTableKind::Plain);
        assert!(datatable.parent_tables.is_empty());
        assert_eq!(datatable.rows.len(), 1);
        assert_eq!(datatable.object_path.as_str(), "/Game/Test/DT_Test.DT_Test");
        assert!(datatable.row_struct.is_none());
        let row = &datatable.rows[0];
        assert_eq!(row.name, name_ref(3, 0));
        let value = &row.properties.records[0].value;
        assert_eq!(value, &PropertyValue::Int(4243));
    }

    #[test]
    fn decodes_datatable_row_struct_from_root_object_ref() {
        let mut root_properties = Vec::new();
        // Import index 0 serializes as package index -1.
        write_object_property_tag(&mut root_properties, 5, 4, -1);

        let mut package = test_package(names());
        package.imports.push(test_import(
            "/Script/E2EFixtures.E2EFixtureScalarsRow",
            "/Script/CoreUObject.ScriptStruct",
            6,
            Some(7),
        ));

        let export_bytes = write_datatable_export(0, &root_properties, &[]);
        let export = test_export(
            export_bytes.len() as u64,
            "/Game/Test/DT_Test.DT_Test",
            "/Script/Engine.DataTable",
        );

        let datatable = decode_datatable(export_bytes, package, export);

        assert_eq!(
            datatable.row_struct.as_ref().map(ObjectPath::as_str),
            Some("/Script/E2EFixtures.E2EFixtureScalarsRow")
        );
        assert!(datatable.rows.is_empty());
    }

    #[test]
    fn rejects_nonzero_datatable_data_marker() {
        let mut export_bytes = write_datatable_export(0, &[], &[]);
        // Layout: u8 extensions, None terminator (8 bytes), then i32 data marker.
        let marker_offset = 1 + 8;
        export_bytes[marker_offset] = 1;

        let package = test_package(names());
        let export = test_export(
            export_bytes.len() as u64,
            "/Game/Test/DT_Test.DT_Test",
            "/Script/Engine.DataTable",
        );
        let schemas = EmptySchemas;
        let context = AssetDecodeContext {
            source: &export_bytes,
            package: &package,
            schemas: &schemas,
        };

        let error = DataTableDecoder
            .decode(&export, &context)
            .expect_err("nonzero marker");
        assert_eq!(error.kind(), AssetErrorKind::MalformedData);
        assert!(error.message().contains("data marker"));
    }

    #[test]
    fn rejects_trailing_datatable_export_bytes() {
        let mut export_bytes = write_datatable_export(0, &[], &[]);
        export_bytes.push(0xFF);

        let package = test_package(names());
        let export = test_export(
            export_bytes.len() as u64,
            "/Game/Test/DT_Test.DT_Test",
            "/Script/Engine.DataTable",
        );
        let schemas = EmptySchemas;
        let context = AssetDecodeContext {
            source: &export_bytes,
            package: &package,
            schemas: &schemas,
        };

        let error = DataTableDecoder
            .decode(&export, &context)
            .expect_err("trailing bytes");
        assert_eq!(error.kind(), AssetErrorKind::MalformedData);
        assert!(error.message().contains("trailing bytes"));
    }

    #[test]
    fn decodes_composite_datatable_parent_tables() {
        let mut root_properties = Vec::new();
        write_object_property_tag(&mut root_properties, 5, 4, -1);
        write_object_array_property_tag(&mut root_properties, 9, 8, 4, &[-2, -3]);

        let mut package = test_package(names());
        package.imports.push(test_import(
            "/Script/E2EFixtures.E2EFixtureScalarsRow",
            "/Script/CoreUObject.ScriptStruct",
            6,
            Some(7),
        ));
        package.imports.push(test_import(
            "/Game/E2EFixture/Data/DT_Scalars.DT_Scalars",
            "/Script/Engine.DataTable",
            10,
            Some(12),
        ));
        package.imports.push(test_import(
            "/Game/E2EFixture/Data/DT_Scalars2.DT_Scalars2",
            "/Script/Engine.DataTable",
            11,
            Some(12),
        ));

        let export_bytes = write_datatable_export(0, &root_properties, &[]);
        let export = test_export(
            export_bytes.len() as u64,
            "/Game/E2EFixture/Data/CDT_E2EFixture.CDT_E2EFixture",
            "/Script/Engine.CompositeDataTable",
        );

        let datatable = decode_datatable(export_bytes, package, export);

        assert_eq!(datatable.kind, DataTableKind::Composite);
        assert_eq!(datatable.parent_tables.len(), 2);
        assert!(
            datatable
                .parent_tables
                .iter()
                .any(|path| path.as_str().contains("DT_Scalars")),
            "expected DT_Scalars parent, got {:?}",
            datatable.parent_tables
        );
        assert!(
            datatable
                .parent_tables
                .iter()
                .any(|path| path.as_str().contains("DT_Scalars2")),
            "expected DT_Scalars2 parent, got {:?}",
            datatable.parent_tables
        );
    }

    #[test]
    fn rejects_unsupported_asset_class() {
        let export_bytes = write_datatable_export(0, &[], &[]);
        let package = test_package(names());
        let export = test_export(
            export_bytes.len() as u64,
            "/Game/Test/BP_Test.BP_Test",
            "/Script/Engine.Blueprint",
        );
        let schemas = EmptySchemas;
        let context = AssetDecodeContext {
            source: &export_bytes,
            package: &package,
            schemas: &schemas,
        };

        let error = DataTableDecoder
            .decode(&export, &context)
            .expect_err("unsupported class");
        assert_eq!(error.kind(), AssetErrorKind::UnsupportedFormat);
    }

    #[test]
    fn recognizes_data_asset_class_paths() {
        assert!(is_data_asset_class(DATA_ASSET_CLASS));
        assert!(is_data_asset_class(PRIMARY_DATA_ASSET_CLASS));
        assert!(is_data_asset_class(
            "/Script/E2EFixtures.E2EFixtureScalarsDataAsset"
        ));
        assert!(!is_data_asset_class(DATATABLE_CLASS));
        assert!(!is_data_asset_class("/Script/Engine.Blueprint"));
    }

    #[test]
    fn recognizes_generic_uobject_class_paths() {
        assert!(is_generic_uobject_class("/Script/Engine.Blueprint"));
        assert!(is_generic_uobject_class(
            "/Script/SWAG_RemoteControlDataTable.SWAG_RemoteControlDataTableLibrary"
        ));
        assert!(!is_generic_uobject_class(DATATABLE_CLASS));
        assert!(!is_generic_uobject_class(
            "/Script/E2EFixtures.E2EFixtureScalarsDataAsset"
        ));
        assert!(!is_generic_uobject_class("/Script/CoreUObject.Package"));
    }

    fn decode_uobject(export_bytes: Vec<u8>, package: Package, export: Export) -> DecodedUObject {
        let schemas = EmptySchemas;
        let context = AssetDecodeContext {
            source: &export_bytes,
            package: &package,
            schemas: &schemas,
        };
        let DecodedAsset::UObject(object) = UObjectDecoder
            .decode(&export, &context)
            .expect("decode uobject")
        else {
            panic!("expected UObject decode");
        };
        object
    }

    #[test]
    fn decodes_generic_uobject_with_scalar_properties() {
        let mut properties = Vec::new();
        write_int_property_tag(&mut properties, 2, 1, 4243);
        let export_bytes = write_uobject_export(0, &properties);
        let package = test_package(vec!["None".into(), "IntProperty".into(), "IntValue".into()]);
        let export = test_export(
            export_bytes.len() as u64,
            "/Game/Test/BP_Test.Default__BP_Test_C",
            "/Script/Engine.BlueprintGeneratedClass",
        );

        let object = decode_uobject(export_bytes, package, export);

        assert_eq!(
            object.object_path.as_str(),
            "/Game/Test/BP_Test.Default__BP_Test_C"
        );
        assert_eq!(
            object.class_path.as_str(),
            "/Script/Engine.BlueprintGeneratedClass"
        );
        assert_eq!(object.properties.records.len(), 1);
        assert_eq!(object.properties.records[0].value, PropertyValue::Int(4243));
    }

    #[test]
    fn decode_export_prefers_datatable_over_uobject() {
        let export_bytes = write_datatable_export(0, &[], &[]);
        let package = test_package(names());
        let export = test_export(
            export_bytes.len() as u64,
            "/Game/Test/DT_Test.DT_Test",
            "/Script/Engine.DataTable",
        );
        let schemas = EmptySchemas;
        let context = AssetDecodeContext {
            source: &export_bytes,
            package: &package,
            schemas: &schemas,
        };

        let decoded = decode_export(&export, &context)
            .expect("decode export")
            .expect("matched decoder");
        assert!(matches!(decoded, DecodedAsset::DataTable(_)));
    }

    fn decode_data_asset(
        export_bytes: Vec<u8>,
        package: Package,
        export: Export,
    ) -> DecodedDataAsset {
        let schemas = EmptySchemas;
        let context = AssetDecodeContext {
            source: &export_bytes,
            package: &package,
            schemas: &schemas,
        };
        let DecodedAsset::DataAsset(data_asset) = DataAssetDecoder
            .decode(&export, &context)
            .expect("decode data asset")
        else {
            panic!("expected DataAsset decode");
        };
        data_asset
    }

    #[test]
    fn decodes_minimal_primary_data_asset_with_scalar_properties() {
        let mut properties = Vec::new();
        write_int_property_tag(&mut properties, 2, 1, 4243);
        let export_bytes = write_uobject_export(0, &properties);
        let package = test_package(vec!["None".into(), "IntProperty".into(), "IntValue".into()]);
        let export = test_export(
            export_bytes.len() as u64,
            "/Game/Test/DA_Test.DA_Test",
            "/Script/E2EFixtures.E2EFixtureScalarsDataAsset",
        );

        let data_asset = decode_data_asset(export_bytes, package, export);

        assert_eq!(
            data_asset.object_path.as_str(),
            "/Game/Test/DA_Test.DA_Test"
        );
        assert_eq!(
            data_asset.class_path.as_str(),
            "/Script/E2EFixtures.E2EFixtureScalarsDataAsset"
        );
        assert_eq!(data_asset.properties.records.len(), 1);
        assert_eq!(
            data_asset.properties.records[0].value,
            PropertyValue::Int(4243)
        );
    }

    #[test]
    fn rejects_trailing_data_asset_export_bytes() {
        let mut export_bytes = write_uobject_export(0, &[]);
        export_bytes.push(0xFF);

        let package = test_package(vec!["None".into()]);
        let export = test_export(
            export_bytes.len() as u64,
            "/Game/Test/DA_Test.DA_Test",
            PRIMARY_DATA_ASSET_CLASS,
        );
        let schemas = EmptySchemas;
        let context = AssetDecodeContext {
            source: &export_bytes,
            package: &package,
            schemas: &schemas,
        };

        let error = DataAssetDecoder
            .decode(&export, &context)
            .expect_err("trailing bytes");
        assert_eq!(error.kind(), AssetErrorKind::MalformedData);
        assert!(error.message().contains("trailing bytes"));
    }
}
