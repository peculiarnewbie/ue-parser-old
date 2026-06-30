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
pub const STRINGTABLE_CLASS: &str = "/Script/Engine.StringTable";
pub const USERDEFINEDENUM_CLASS: &str = "/Script/Engine.UserDefinedEnum";

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
        && class_path != STRINGTABLE_CLASS
        && class_path != USERDEFINEDENUM_CLASS
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
    StringTable(DecodedStringTable),
    DataAsset(DecodedDataAsset),
    UObject(DecodedUObject),
    Enum(DecodedEnum),
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

/// A decoded `UUserDefinedEnum` export.
///
/// The `DisplayNameMap` (`TMap<FName, FText>`) rides in the tagged-property
/// stream and is retained in `properties`; each entry's `display_name` is the
/// resolved value from that map, when present.
#[derive(Clone, Debug, PartialEq)]
pub struct DecodedEnum {
    pub object_path: ObjectPath,
    pub cpp_form: EnumCppForm,
    pub properties: PropertyStream,
    pub entries: Vec<EnumEntry>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EnumEntry {
    /// Qualified `FName` for the entry, e.g. `MyEnum::Entry0`.
    pub name: NameRef,
    pub value: i64,
    pub display_name: Option<String>,
}

/// How a `UEnum` was originally declared (`UEnum::ECppForm`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnumCppForm {
    Regular,
    Namespaced,
    EnumClass,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DecodedStringTable {
    pub object_path: ObjectPath,
    pub namespace: String,
    pub entries: Vec<StringTableEntry>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StringTableEntry {
    pub key: String,
    pub source: String,
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
    pub keys: Vec<CurveKey>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CurveKey {
    Simple(SimpleCurveKey),
    Rich(RichCurveKey),
}

impl CurveKey {
    #[must_use]
    pub const fn time(self) -> f32 {
        match self {
            Self::Simple(key) => key.time,
            Self::Rich(key) => key.time,
        }
    }

    #[must_use]
    pub const fn value(self) -> f32 {
        match self {
            Self::Simple(key) => key.value,
            Self::Rich(key) => key.value,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SimpleCurveKey {
    pub time: f32,
    pub value: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RichCurveKey {
    pub interp_mode: u8,
    pub tangent_mode: u8,
    pub tangent_weight_mode: u8,
    pub time: f32,
    pub value: f32,
    pub arrive_tangent: f32,
    pub arrive_tangent_weight: f32,
    pub leave_tangent: f32,
    pub leave_tangent_weight: f32,
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
            let keys = match mode {
                CurveTableMode::Empty => Vec::new(),
                CurveTableMode::SimpleCurves => {
                    decode_simple_curve_keys(context.source, context.package, &stream, &row_path)?
                }
                CurveTableMode::RichCurves => {
                    decode_rich_curve_keys(context.source, context.package, &stream, &row_path)?
                }
            };
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
pub struct StringTableDecoder;

impl AssetDecoder for StringTableDecoder {
    fn supports(&self, class_path: &ObjectPath) -> bool {
        class_path.as_str() == STRINGTABLE_CLASS
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
        if class_path.as_str() != STRINGTABLE_CLASS {
            return Err(AssetError::new(
                AssetErrorKind::UnsupportedFormat,
                format!("unsupported asset class {class_path}"),
            ));
        }

        let (_properties, mut reader) = decode_uobject_properties(export, context)?;
        let footer_offset = reader.tell();
        let footer = reader.read_i32(&format!("{}.ExportFooter", export.object_path))?;
        if footer != 0 {
            return Err(AssetError::new(
                AssetErrorKind::MalformedData,
                format!(
                    "expected zero StringTable UObject footer at byte {footer_offset}, got {footer}"
                ),
            ));
        }

        let namespace = reader.read_fstring(&format!("{}.Namespace", export.object_path))?;
        let entry_count_offset = reader.tell();
        let entry_count = reader.read_i32(&format!("{}.Entries.Count", export.object_path))?;
        if entry_count < 0 {
            return Err(AssetError::new(
                AssetErrorKind::MalformedData,
                format!(
                    "negative StringTable entry count {entry_count} at byte {entry_count_offset}"
                ),
            ));
        }
        let mut entries =
            Vec::with_capacity(usize::try_from(entry_count).expect("i32 fits in usize"));
        for index in 0..entry_count {
            let entry_path = format!("{}.Entries[{index}]", export.object_path);
            let key = reader.read_fstring(&format!("{entry_path}.Key"))?;
            let source = reader.read_fstring(&format!("{entry_path}.SourceString"))?;
            entries.push(StringTableEntry { key, source });
        }

        let metadata_count = reader.read_i32(&format!("{}.MetaData.Count", export.object_path))?;
        if metadata_count != 0 {
            return Err(AssetError::new(
                AssetErrorKind::UnsupportedCapability,
                format!(
                    "StringTable metadata map with {metadata_count} entries is not supported yet"
                ),
            ));
        }

        if reader.remaining() != 0 {
            return Err(AssetError::new(
                AssetErrorKind::MalformedData,
                format!(
                    "StringTable export {} left {} trailing bytes",
                    export.object_path,
                    reader.remaining()
                ),
            ));
        }

        Ok(DecodedAsset::StringTable(DecodedStringTable {
            object_path: export.object_path.clone(),
            namespace,
            entries,
        }))
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct EnumDecoder;

impl AssetDecoder for EnumDecoder {
    fn supports(&self, class_path: &ObjectPath) -> bool {
        class_path.as_str() == USERDEFINEDENUM_CLASS
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
        if class_path.as_str() != USERDEFINEDENUM_CLASS {
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
                format!("expected zero Enum UObject footer at byte {footer_offset}, got {footer}"),
            ));
        }

        // `UEnum::Serialize` writes the names as `int32 Num` followed by
        // `Num` × (`FName`, `int64`) pairs, then a `uint8 CppForm`.
        let count_offset = reader.tell();
        let count = reader.read_i32(&format!("{}.Names.Count", export.object_path))?;
        if count < 0 {
            return Err(AssetError::new(
                AssetErrorKind::MalformedData,
                format!("negative Enum name count {count} at byte {count_offset}"),
            ));
        }
        let mut raw_entries =
            Vec::with_capacity(usize::try_from(count).expect("i32 fits in usize"));
        for index in 0..count {
            let entry_path = format!("{}.Names[{index}]", export.object_path);
            let name = reader.read_name_ref(&format!("{entry_path}.Name"))?;
            let value = reader.read_i64(&format!("{entry_path}.Value"))?;
            raw_entries.push((name, value));
        }

        let cpp_form_offset = reader.tell();
        let raw_form = reader.read_u8(&format!("{}.CppForm", export.object_path))?;
        let cpp_form = match raw_form {
            0 => EnumCppForm::Regular,
            1 => EnumCppForm::Namespaced,
            2 => EnumCppForm::EnumClass,
            value => {
                return Err(AssetError::new(
                    AssetErrorKind::MalformedData,
                    format!("unsupported Enum CppForm {value} at byte {cpp_form_offset}"),
                ));
            }
        };

        if reader.remaining() != 0 {
            return Err(AssetError::new(
                AssetErrorKind::MalformedData,
                format!(
                    "Enum export {} left {} trailing bytes",
                    export.object_path,
                    reader.remaining()
                ),
            ));
        }

        let display_names = display_name_map(context.package, &properties);
        let entries = raw_entries
            .into_iter()
            .map(|(name, value)| EnumEntry {
                name,
                value,
                display_name: display_names
                    .iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, source)| source.clone()),
            })
            .collect();

        Ok(DecodedAsset::Enum(DecodedEnum {
            object_path: export.object_path.clone(),
            cpp_form,
            properties,
            entries,
        }))
    }
}

/// Collects the `DisplayNameMap` (`TMap<FName, FText>`) entries from a decoded
/// `UUserDefinedEnum` property stream as `(qualified name, display string)`
/// pairs. Returns empty when the map is absent or carries unexpected value types.
fn display_name_map(package: &Package, properties: &PropertyStream) -> Vec<(NameRef, String)> {
    let Some(record) = properties
        .records
        .iter()
        .find(|record| package.resolve_name(record.name).as_deref() == Some("DisplayNameMap"))
    else {
        return Vec::new();
    };
    let PropertyValue::Map(entries) = &record.value else {
        return Vec::new();
    };
    entries
        .iter()
        .filter_map(|entry| {
            let PropertyValue::Name(name) = entry.key else {
                return None;
            };
            let PropertyValue::Text(text) = &entry.value else {
                return None;
            };
            Some((name, text.source.clone()))
        })
        .collect()
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
    if StringTableDecoder.supports(class_path) {
        return StringTableDecoder.decode(export, context).map(Some);
    }
    if DataAssetDecoder.supports(class_path) {
        return DataAssetDecoder.decode(export, context).map(Some);
    }
    if EnumDecoder.supports(class_path) {
        return EnumDecoder.decode(export, context).map(Some);
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
) -> Result<Vec<CurveKey>, AssetError> {
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
        keys.push(CurveKey::Simple(SimpleCurveKey {
            time: payload.read_f32(&format!("{path}.Keys[{index}].Time"))?,
            value: payload.read_f32(&format!("{path}.Keys[{index}].Value"))?,
        }));
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

fn decode_rich_curve_keys(
    source: &[u8],
    package: &Package,
    stream: &PropertyStream,
    path: &str,
) -> Result<Vec<CurveKey>, AssetError> {
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
            format!("negative RichCurve key count {count}"),
        ));
    }
    let mut keys = Vec::with_capacity(usize::try_from(count).expect("i32 fits in usize"));
    for index in 0..count {
        keys.push(CurveKey::Rich(RichCurveKey {
            interp_mode: payload.read_u8(&format!("{path}.Keys[{index}].InterpMode"))?,
            tangent_mode: payload.read_u8(&format!("{path}.Keys[{index}].TangentMode"))?,
            tangent_weight_mode: payload
                .read_u8(&format!("{path}.Keys[{index}].TangentWeightMode"))?,
            time: payload.read_f32(&format!("{path}.Keys[{index}].Time"))?,
            value: payload.read_f32(&format!("{path}.Keys[{index}].Value"))?,
            arrive_tangent: payload.read_f32(&format!("{path}.Keys[{index}].ArriveTangent"))?,
            arrive_tangent_weight: payload
                .read_f32(&format!("{path}.Keys[{index}].ArriveTangentWeight"))?,
            leave_tangent: payload.read_f32(&format!("{path}.Keys[{index}].LeaveTangent"))?,
            leave_tangent_weight: payload
                .read_f32(&format!("{path}.Keys[{index}].LeaveTangentWeight"))?,
        }));
    }
    if payload.remaining() != 0 {
        return Err(AssetError::new(
            AssetErrorKind::MalformedData,
            format!(
                "{path}.Keys left {} trailing bytes after RichCurve key decode",
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
        TypeParam, name_ref, push_f32, push_fstring, push_i32, write_datatable_export,
        write_int_property_tag, write_object_array_property_tag, write_object_property_tag,
        write_property_tag, write_property_terminator, write_uobject_export,
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
            "ArrayProperty".into(),
            "Keys".into(),
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

    fn write_curvetable_export(
        none_name_index: i32,
        root_properties: &[u8],
        mode: u8,
        rows: &[(i32, &[u8])],
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.push(0); // class serialization-control extensions
        bytes.extend_from_slice(root_properties);
        write_property_terminator(&mut bytes, none_name_index);
        push_i32(&mut bytes, 0); // UObject export footer
        push_i32(&mut bytes, i32::try_from(rows.len()).expect("fits in i32"));
        bytes.push(mode);
        for (name_index, row_properties) in rows {
            push_i32(&mut bytes, *name_index);
            push_i32(&mut bytes, 0);
            bytes.extend_from_slice(row_properties);
            write_property_terminator(&mut bytes, none_name_index);
        }
        bytes
    }

    fn decode_curve_table(
        export_bytes: Vec<u8>,
        package: Package,
        export: Export,
    ) -> DecodedCurveTable {
        let schemas = EmptySchemas;
        let context = AssetDecodeContext {
            source: &export_bytes,
            package: &package,
            schemas: &schemas,
        };
        let DecodedAsset::CurveTable(curve_table) = CurveTableDecoder
            .decode(&export, &context)
            .expect("decode curve table")
        else {
            panic!("expected CurveTable decode");
        };
        curve_table
    }

    fn write_stringtable_export(
        none_name_index: i32,
        namespace: &str,
        entries: &[(&str, &str)],
        metadata_count: i32,
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.push(0); // class serialization-control extensions
        write_property_terminator(&mut bytes, none_name_index);
        push_i32(&mut bytes, 0); // UObject export footer
        push_fstring(&mut bytes, namespace);
        push_i32(
            &mut bytes,
            i32::try_from(entries.len()).expect("fits in i32"),
        );
        for (key, source) in entries {
            push_fstring(&mut bytes, key);
            push_fstring(&mut bytes, source);
        }
        push_i32(&mut bytes, metadata_count);
        bytes
    }

    fn decode_string_table(
        export_bytes: Vec<u8>,
        package: Package,
        export: Export,
    ) -> DecodedStringTable {
        let schemas = EmptySchemas;
        let context = AssetDecodeContext {
            source: &export_bytes,
            package: &package,
            schemas: &schemas,
        };
        let DecodedAsset::StringTable(string_table) = StringTableDecoder
            .decode(&export, &context)
            .expect("decode string table")
        else {
            panic!("expected StringTable decode");
        };
        string_table
    }

    /// Builds a `DisplayNameMap` (`TMap<FName, FText>`) tagged property whose
    /// keys are qualified enum-entry FNames and values are display strings.
    fn write_display_name_map_property(
        bytes: &mut Vec<u8>,
        name_index: i32,
        map_type_index: i32,
        name_type_index: i32,
        text_type_index: i32,
        entries: &[(i32, &str)],
    ) {
        let mut payload = Vec::new();
        push_i32(&mut payload, 0); // KeysToRemove
        push_i32(
            &mut payload,
            i32::try_from(entries.len()).expect("fits in i32"),
        );
        for (entry_name_index, display_name) in entries {
            push_i32(&mut payload, *entry_name_index); // FName index
            push_i32(&mut payload, 0); // FName number
            push_i32(&mut payload, 0); // FText flags
            payload.push(0); // FText history type (Base)
            push_fstring(&mut payload, ""); // namespace
            push_fstring(&mut payload, ""); // key
            push_fstring(&mut payload, display_name); // source string
        }
        write_property_tag(
            bytes,
            name_index,
            &TypeParam {
                type_index: map_type_index,
                parameters: vec![
                    TypeParam {
                        type_index: name_type_index,
                        parameters: Vec::new(),
                    },
                    TypeParam {
                        type_index: text_type_index,
                        parameters: Vec::new(),
                    },
                ],
            },
            0,
            &payload,
        );
    }

    /// Builds a synthetic `UUserDefinedEnum` export: extensions byte, optional
    /// tagged properties, terminator, zero UObject footer, then the `UEnum` tail
    /// (`int32 Num`, `Num` × (`FName`, `int64`), `uint8 CppForm`).
    fn write_userdefinedenum_export(
        none_name_index: i32,
        properties: &[u8],
        entries: &[(i32, i64)],
        cpp_form: u8,
    ) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.push(0); // class serialization-control extensions
        bytes.extend_from_slice(properties);
        write_property_terminator(&mut bytes, none_name_index);
        push_i32(&mut bytes, 0); // UObject export footer
        push_i32(
            &mut bytes,
            i32::try_from(entries.len()).expect("fits in i32"),
        );
        for (name_index, value) in entries {
            push_i32(&mut bytes, *name_index); // FName index
            push_i32(&mut bytes, 0); // FName number
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.push(cpp_form);
        bytes
    }

    fn decode_enum(export_bytes: Vec<u8>, package: Package, export: Export) -> DecodedEnum {
        let schemas = EmptySchemas;
        let context = AssetDecodeContext {
            source: &export_bytes,
            package: &package,
            schemas: &schemas,
        };
        let DecodedAsset::Enum(decoded_enum) =
            EnumDecoder.decode(&export, &context).expect("decode enum")
        else {
            panic!("expected Enum decode");
        };
        decoded_enum
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
    fn decodes_rich_curve_table_keys() {
        let mut keys_payload = Vec::new();
        push_i32(&mut keys_payload, 1);
        keys_payload.push(3); // RCIM_Cubic
        keys_payload.push(2); // RCTM_Break
        keys_payload.push(3); // RCTWM_WeightedBoth
        push_f32(&mut keys_payload, 1.5);
        push_f32(&mut keys_payload, 42.25);
        push_f32(&mut keys_payload, -0.5);
        push_f32(&mut keys_payload, 0.25);
        push_f32(&mut keys_payload, 0.75);
        push_f32(&mut keys_payload, 0.5);

        let mut curve_properties = Vec::new();
        write_property_tag(
            &mut curve_properties,
            14,
            &TypeParam {
                type_index: 13,
                parameters: Vec::new(),
            },
            0,
            &keys_payload,
        );

        let export_bytes = write_curvetable_export(0, &[], 2, &[(3, curve_properties.as_slice())]);
        let package = test_package(names());
        let export = test_export(
            export_bytes.len() as u64,
            "/Game/Test/CT_Test.CT_Test",
            CURVETABLE_CLASS,
        );

        let curve_table = decode_curve_table(export_bytes, package, export);

        assert_eq!(curve_table.mode, CurveTableMode::RichCurves);
        assert_eq!(curve_table.rows.len(), 1);
        assert_eq!(curve_table.rows[0].name, name_ref(3, 0));
        assert_eq!(
            curve_table.rows[0].keys,
            vec![CurveKey::Rich(RichCurveKey {
                interp_mode: 3,
                tangent_mode: 2,
                tangent_weight_mode: 3,
                time: 1.5,
                value: 42.25,
                arrive_tangent: -0.5,
                arrive_tangent_weight: 0.25,
                leave_tangent: 0.75,
                leave_tangent_weight: 0.5,
            })]
        );
    }

    #[test]
    fn decodes_string_table_entries() {
        let export_bytes = write_stringtable_export(
            0,
            "ST_Simple",
            &[
                ("HELLO", "Hello from string table"),
                ("FAREWELL", "Goodbye from string table"),
            ],
            0,
        );
        let package = test_package(vec!["None".into()]);
        let export = test_export(
            export_bytes.len() as u64,
            "/Game/Test/ST_Simple.ST_Simple",
            STRINGTABLE_CLASS,
        );

        let string_table = decode_string_table(export_bytes, package, export);

        assert_eq!(
            string_table.object_path.as_str(),
            "/Game/Test/ST_Simple.ST_Simple"
        );
        assert_eq!(string_table.namespace, "ST_Simple");
        assert_eq!(
            string_table.entries,
            vec![
                StringTableEntry {
                    key: "HELLO".to_owned(),
                    source: "Hello from string table".to_owned(),
                },
                StringTableEntry {
                    key: "FAREWELL".to_owned(),
                    source: "Goodbye from string table".to_owned(),
                },
            ]
        );
    }

    #[test]
    fn rejects_string_table_metadata_until_supported() {
        let export_bytes = write_stringtable_export(0, "ST_Simple", &[], 1);
        let package = test_package(vec!["None".into()]);
        let export = test_export(
            export_bytes.len() as u64,
            "/Game/Test/ST_Simple.ST_Simple",
            STRINGTABLE_CLASS,
        );
        let schemas = EmptySchemas;
        let context = AssetDecodeContext {
            source: &export_bytes,
            package: &package,
            schemas: &schemas,
        };

        let error = StringTableDecoder
            .decode(&export, &context)
            .expect_err("metadata unsupported");
        assert_eq!(error.kind(), AssetErrorKind::UnsupportedCapability);
        assert!(error.message().contains("metadata"));
    }

    fn enum_names() -> Vec<String> {
        vec![
            "None".into(),                 // 0
            "E_Color::Red".into(),         // 1
            "E_Color::Green".into(),       // 2
            "E_Color::E_Color_MAX".into(), // 3
            "DisplayNameMap".into(),       // 4
            "MapProperty".into(),          // 5
            "NameProperty".into(),         // 6
            "TextProperty".into(),         // 7
        ]
    }

    #[test]
    fn decodes_user_defined_enum_entries() {
        let export_bytes = write_userdefinedenum_export(0, &[], &[(1, 0), (2, 1), (3, 2)], 2);
        let package = test_package(enum_names());
        let export = test_export(
            export_bytes.len() as u64,
            "/Game/Test/E_Color.E_Color",
            USERDEFINEDENUM_CLASS,
        );

        let decoded = decode_enum(export_bytes, package, export);

        assert_eq!(decoded.object_path.as_str(), "/Game/Test/E_Color.E_Color");
        assert_eq!(decoded.cpp_form, EnumCppForm::EnumClass);
        assert_eq!(
            decoded.entries,
            vec![
                EnumEntry {
                    name: name_ref(1, 0),
                    value: 0,
                    display_name: None,
                },
                EnumEntry {
                    name: name_ref(2, 0),
                    value: 1,
                    display_name: None,
                },
                EnumEntry {
                    name: name_ref(3, 0),
                    value: 2,
                    display_name: None,
                },
            ]
        );
    }

    #[test]
    fn resolves_user_defined_enum_display_names() {
        let mut properties = Vec::new();
        write_display_name_map_property(&mut properties, 4, 5, 6, 7, &[(1, "Red"), (2, "Green")]);
        let export_bytes =
            write_userdefinedenum_export(0, &properties, &[(1, 0), (2, 1), (3, 2)], 2);
        let package = test_package(enum_names());
        let export = test_export(
            export_bytes.len() as u64,
            "/Game/Test/E_Color.E_Color",
            USERDEFINEDENUM_CLASS,
        );

        let decoded = decode_enum(export_bytes, package, export);

        assert_eq!(
            decoded
                .entries
                .iter()
                .map(|entry| entry.display_name.clone())
                .collect::<Vec<_>>(),
            vec![Some("Red".to_owned()), Some("Green".to_owned()), None],
        );
    }

    #[test]
    fn rejects_unsupported_enum_cpp_form() {
        let export_bytes = write_userdefinedenum_export(0, &[], &[(1, 0)], 7);
        let package = test_package(enum_names());
        let export = test_export(
            export_bytes.len() as u64,
            "/Game/Test/E_Color.E_Color",
            USERDEFINEDENUM_CLASS,
        );
        let schemas = EmptySchemas;
        let context = AssetDecodeContext {
            source: &export_bytes,
            package: &package,
            schemas: &schemas,
        };

        let error = EnumDecoder
            .decode(&export, &context)
            .expect_err("unsupported cpp form");
        assert_eq!(error.kind(), AssetErrorKind::MalformedData);
        assert!(error.message().contains("CppForm"));
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
