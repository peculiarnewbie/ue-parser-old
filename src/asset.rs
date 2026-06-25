//! Asset-level decoders.

use std::fmt;

use crate::archive::{ArchiveError, ArchiveErrorKind, NameRef};
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

#[derive(Clone, Debug, PartialEq)]
pub enum DecodedAsset {
    DataTable(DecodedDataTable),
}

#[derive(Clone, Debug, PartialEq)]
pub struct DecodedDataTable {
    pub object_path: ObjectPath,
    pub row_struct: Option<ObjectPath>,
    pub properties: PropertyStream,
    pub rows: Vec<DataTableRow>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DataTableRow {
    pub name: NameRef,
    pub properties: PropertyStream,
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
        class_path.as_str() == "/Script/Engine.DataTable"
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
        if !self.supports(class_path) {
            return Err(AssetError::new(
                AssetErrorKind::UnsupportedFormat,
                format!("unsupported asset class {class_path}"),
            ));
        }

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
        let row_struct = row_struct_path(context.package, &properties);

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
            object_path: export.object_path.clone(),
            row_struct,
            properties,
            rows,
        }))
    }
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
