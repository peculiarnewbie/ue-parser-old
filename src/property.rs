//! Generic tagged-property model and envelope parsing.

use std::fmt;

use crate::archive::{ArchiveError, ArchiveErrorKind, Guid, NameRef, Reader, Span};
use crate::package::PackageIndex;
use crate::version::VersionContext;

const UE5_PROPERTY_TAG_EXTENSION_AND_OVERRIDABLE_SERIALIZATION: i32 = 1011;
const UE5_PROPERTY_TAG_COMPLETE_TYPE_NAME: i32 = 1012;

const TAG_FLAG_HAS_ARRAY_INDEX: u8 = 0x01;
const TAG_FLAG_HAS_PROPERTY_GUID: u8 = 0x02;
const TAG_FLAG_HAS_PROPERTY_EXTENSIONS: u8 = 0x04;
const TAG_FLAG_HAS_BINARY_OR_NATIVE_SERIALIZE: u8 = 0x08;
const TAG_FLAG_BOOL_TRUE: u8 = 0x10;
const TAG_FLAG_SKIPPED_SERIALIZE: u8 = 0x20;

const PROPERTY_EXTENSION_RESERVE_FOR_FUTURE_USE: u8 = 0x01;
const PROPERTY_EXTENSION_OVERRIDABLE_INFORMATION: u8 = 0x02;

const CLASS_EXTENSION_RESERVE_FOR_FUTURE_USE: u8 = 0x01;
const CLASS_EXTENSION_OVERRIDABLE_SERIALIZATION_INFORMATION: u8 = 0x02;
const MAX_PROPERTY_TYPE_DEPTH: usize = 64;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PropertyTypeName {
    pub name: NameRef,
    pub parameters: Vec<PropertyTypeName>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PropertyTagFlags(pub u8);

impl PropertyTagFlags {
    #[must_use]
    pub const fn contains(self, flag: u8) -> bool {
        self.0 & flag == flag
    }

    #[must_use]
    pub const fn bool_value(self) -> bool {
        self.contains(TAG_FLAG_BOOL_TRUE)
    }

    #[must_use]
    pub const fn is_binary_or_native(self) -> bool {
        self.contains(TAG_FLAG_HAS_BINARY_OR_NATIVE_SERIALIZE)
    }

    #[must_use]
    pub const fn is_skipped(self) -> bool {
        self.contains(TAG_FLAG_SKIPPED_SERIALIZE)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RawReason {
    UnsupportedType,
    DecoderRejected(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextValue {
    pub source: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct VectorValue {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntPointValue {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MapEntry {
    pub key: PropertyValue,
    pub value: PropertyValue,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PropertyValue {
    Bool(bool),
    Float(f32),
    Double(f64),
    Int(i64),
    UInt(u64),
    Name(NameRef),
    Enum(NameRef),
    String(String),
    Text(TextValue),
    Vector(VectorValue),
    IntPoint(IntPointValue),
    ObjectRef(PackageIndex),
    Guid(Guid),
    SoftObjectPath(String),
    Array(Vec<PropertyValue>),
    Set(Vec<PropertyValue>),
    Map(Vec<MapEntry>),
    Struct(PropertyStream),
    Raw { reason: RawReason },
}

#[derive(Clone, Debug, PartialEq)]
pub struct PropertyRecord {
    pub name: NameRef,
    pub type_name: PropertyTypeName,
    pub array_index: i32,
    pub flags: PropertyTagFlags,
    pub property_guid: Option<Guid>,
    pub extensions: Option<PropertyTagExtensions>,
    pub payload: Span,
    pub value: PropertyValue,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PropertyTagExtensions {
    pub raw_flags: u8,
    pub override_operation: Option<u8>,
    pub experimental_overridable_logic: Option<bool>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PropertyStream {
    pub class_extensions: Option<ClassSerializationControlExtensions>,
    pub records: Vec<PropertyRecord>,
    pub terminator: Span,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClassSerializationControlExtensions {
    pub raw_flags: u8,
    pub overridable_operation: Option<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PropertyErrorKind {
    MalformedData,
    ResourceLimit,
    UnsupportedVersion,
    UnsupportedCapability,
}

#[derive(Debug)]
pub struct PropertyError {
    kind: PropertyErrorKind,
    offset: Option<u64>,
    path: String,
    detail: String,
    source: Option<Box<ArchiveError>>,
}

impl PropertyError {
    pub(crate) fn new(
        kind: PropertyErrorKind,
        offset: Option<u64>,
        path: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            offset,
            path: path.into(),
            detail: detail.into(),
            source: None,
        }
    }

    #[must_use]
    pub const fn kind(&self) -> PropertyErrorKind {
        self.kind
    }

    #[must_use]
    pub const fn offset(&self) -> Option<u64> {
        self.offset
    }

    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for PropertyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.offset {
            Some(offset) => write!(
                formatter,
                "{:?} at byte {offset} while reading {}: {}",
                self.kind, self.path, self.detail
            ),
            None => write!(
                formatter,
                "{:?} while reading {}: {}",
                self.kind, self.path, self.detail
            ),
        }
    }
}

impl std::error::Error for PropertyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|source| source.as_ref() as &(dyn std::error::Error + 'static))
    }
}

impl From<ArchiveError> for PropertyError {
    fn from(source: ArchiveError) -> Self {
        let kind = match source.kind() {
            ArchiveErrorKind::OutOfBounds
            | ArchiveErrorKind::InvalidSeek
            | ArchiveErrorKind::InvalidCount
            | ArchiveErrorKind::MissingNullTerminator
            | ArchiveErrorKind::InvalidString
            | ArchiveErrorKind::InvalidNameReference
            | ArchiveErrorKind::IntegerOverflow => PropertyErrorKind::MalformedData,
            ArchiveErrorKind::AllocationLimit => PropertyErrorKind::ResourceLimit,
        };
        Self {
            kind,
            offset: Some(source.offset()),
            path: source.path().to_owned(),
            detail: source.detail().to_owned(),
            source: Some(Box::new(source)),
        }
    }
}

/// Parses a root UObject versioned tagged-property stream.
///
/// UE5 root object streams include class serialization-control extensions
/// immediately before the tagged-property array. Struct/row streams do not.
///
/// # Errors
///
/// Returns the same errors as [`read_tagged_property_stream`], plus unsupported
/// class serialization-control extension groups.
pub fn read_uobject_tagged_property_stream(
    reader: &mut Reader<'_>,
    versions: &VersionContext,
    names: &[String],
    path: &str,
) -> Result<PropertyStream, PropertyError> {
    let class_extensions = versions
        .is_at_least_ue5(UE5_PROPERTY_TAG_EXTENSION_AND_OVERRIDABLE_SERIALIZATION)
        .then(|| {
            read_class_serialization_control_extensions(
                reader,
                &format!("{path}.SerializationControlExtensions"),
            )
        })
        .transpose()?;
    let mut stream = read_tagged_property_stream(reader, versions, names, path)?;
    stream.class_extensions = class_extensions;
    Ok(stream)
}

/// Parses a versioned tagged-property stream until `NAME_None`.
///
/// The reader must already be bounded to the containing export or row stream.
/// On success, the reader cursor is positioned immediately after the terminator.
///
/// # Errors
///
/// Returns an error for unsupported pre-complete-type-name tags, malformed name
/// references, malformed type trees, invalid payload sizes, unsupported future
/// extension groups, or any bounded-reader failure.
pub fn read_tagged_property_stream(
    reader: &mut Reader<'_>,
    versions: &VersionContext,
    names: &[String],
    path: &str,
) -> Result<PropertyStream, PropertyError> {
    if !versions.is_at_least_ue5(UE5_PROPERTY_TAG_COMPLETE_TYPE_NAME) {
        return Err(PropertyError::new(
            PropertyErrorKind::UnsupportedVersion,
            Some(reader.tell()),
            path,
            "property tags before complete type names are not supported",
        ));
    }

    let mut records = Vec::new();
    loop {
        let tag_start = reader.tell();
        let name = reader.read_name_ref(&format_args!("{path}.Tag.Name"))?;
        validate_name_ref(names, name, &format!("{path}.Tag.Name"))?;
        if resolve_name_ref(names, name)? == "None" {
            return Ok(PropertyStream {
                class_extensions: None,
                records,
                terminator: Span::new(tag_start, reader.tell() - tag_start)?,
            });
        }

        let type_name = read_property_type_name(reader, names, &format!("{path}.Tag.Type"))?;
        let size_offset = reader.tell();
        let size = reader.read_i32(&format_args!("{path}.Tag.Size"))?;
        if size < 0 {
            return Err(PropertyError::new(
                PropertyErrorKind::MalformedData,
                Some(size_offset),
                format!("{path}.Tag.Size"),
                format!("property payload size must be non-negative, got {size}"),
            ));
        }
        let payload_size = u64::try_from(size).expect("size was checked as non-negative");
        let flags = PropertyTagFlags(reader.read_u8(&format_args!("{path}.Tag.Flags"))?);
        let array_index = if flags.contains(TAG_FLAG_HAS_ARRAY_INDEX) {
            reader.read_i32(&format_args!("{path}.Tag.ArrayIndex"))?
        } else {
            0
        };
        let property_guid = flags
            .contains(TAG_FLAG_HAS_PROPERTY_GUID)
            .then(|| reader.read_guid(&format_args!("{path}.Tag.PropertyGuid")))
            .transpose()?;
        let extensions = if flags.contains(TAG_FLAG_HAS_PROPERTY_EXTENSIONS) {
            Some(read_property_extensions(
                reader,
                versions,
                &format!("{path}.Tag.PropertyExtensions"),
            )?)
        } else {
            None
        };

        let payload = Span::new(reader.tell(), payload_size)?;
        reader.skip(payload_size, &format_args!("{path}.Tag.Payload"))?;
        records.push(PropertyRecord {
            name,
            type_name,
            array_index,
            flags,
            property_guid,
            extensions,
            payload,
            value: PropertyValue::Raw {
                reason: RawReason::UnsupportedType,
            },
        });
    }
}

fn read_class_serialization_control_extensions(
    reader: &mut Reader<'_>,
    path: &str,
) -> Result<ClassSerializationControlExtensions, PropertyError> {
    let extension_offset = reader.tell();
    let raw_flags = reader.read_u8(path)?;
    if raw_flags & CLASS_EXTENSION_RESERVE_FOR_FUTURE_USE != 0 {
        return Err(PropertyError::new(
            PropertyErrorKind::UnsupportedCapability,
            Some(extension_offset),
            path,
            "future class serialization-control extension groups are not supported",
        ));
    }
    let overridable_operation = (raw_flags & CLASS_EXTENSION_OVERRIDABLE_SERIALIZATION_INFORMATION
        != 0)
        .then(|| reader.read_u8(&format!("{path}.OverridableOperation")))
        .transpose()?;
    Ok(ClassSerializationControlExtensions {
        raw_flags,
        overridable_operation,
    })
}

fn read_property_type_name(
    reader: &mut Reader<'_>,
    names: &[String],
    path: &str,
) -> Result<PropertyTypeName, PropertyError> {
    read_property_type_name_at_depth(reader, names, path, 0)
}

fn read_property_type_name_at_depth(
    reader: &mut Reader<'_>,
    names: &[String],
    path: &str,
    depth: usize,
) -> Result<PropertyTypeName, PropertyError> {
    if depth >= MAX_PROPERTY_TYPE_DEPTH {
        return Err(PropertyError::new(
            PropertyErrorKind::MalformedData,
            Some(reader.tell()),
            path,
            format!("property type-name nesting exceeds depth limit {MAX_PROPERTY_TYPE_DEPTH}"),
        ));
    }
    let name = reader.read_name_ref(&format!("{path}.Name"))?;
    validate_name_ref(names, name, &format!("{path}.Name"))?;
    let inner_count_offset = reader.tell();
    let inner_count = reader.read_i32(&format!("{path}.InnerCount"))?;
    if inner_count < 0 {
        return Err(PropertyError::new(
            PropertyErrorKind::MalformedData,
            Some(inner_count_offset),
            format!("{path}.InnerCount"),
            format!("type-name inner count must be non-negative, got {inner_count}"),
        ));
    }
    let inner_count = usize::try_from(inner_count).expect("non-negative i32 fits in usize");
    let capacity = reader.checked_vec_capacity::<PropertyTypeName>(
        inner_count,
        12,
        &format!("{path}.InnerCount"),
    )?;
    let mut parameters = Vec::with_capacity(capacity);
    for index in 0..inner_count {
        parameters.push(read_property_type_name_at_depth(
            reader,
            names,
            &format!("{path}.Parameters[{index}]"),
            depth + 1,
        )?);
    }
    Ok(PropertyTypeName { name, parameters })
}

fn read_property_extensions(
    reader: &mut Reader<'_>,
    versions: &VersionContext,
    path: &str,
) -> Result<PropertyTagExtensions, PropertyError> {
    if !versions.is_at_least_ue5(UE5_PROPERTY_TAG_EXTENSION_AND_OVERRIDABLE_SERIALIZATION) {
        return Err(PropertyError::new(
            PropertyErrorKind::MalformedData,
            Some(reader.tell()),
            path,
            "property tag has extension flag before extension serialization is supported",
        ));
    }
    let extension_offset = reader.tell();
    let raw_flags = reader.read_u8(path)?;
    if raw_flags & PROPERTY_EXTENSION_RESERVE_FOR_FUTURE_USE != 0 {
        return Err(PropertyError::new(
            PropertyErrorKind::UnsupportedCapability,
            Some(extension_offset),
            path,
            "future property-tag extension groups are not supported",
        ));
    }
    let (override_operation, experimental_overridable_logic) =
        if raw_flags & PROPERTY_EXTENSION_OVERRIDABLE_INFORMATION != 0 {
            (
                Some(reader.read_u8(&format!("{path}.OverriddenPropertyOperation"))?),
                Some(read_archive_bool(
                    reader,
                    &format!("{path}.ExperimentalOverridableLogic"),
                )?),
            )
        } else {
            (None, None)
        };
    Ok(PropertyTagExtensions {
        raw_flags,
        override_operation,
        experimental_overridable_logic,
    })
}

fn read_archive_bool(reader: &mut Reader<'_>, path: &str) -> Result<bool, PropertyError> {
    let offset = reader.tell();
    match reader.read_u32(path)? {
        0 => Ok(false),
        1 => Ok(true),
        value => Err(PropertyError::new(
            PropertyErrorKind::MalformedData,
            Some(offset),
            path,
            format!("serialized bool must be 0 or 1, got {value}"),
        )),
    }
}

fn validate_name_ref(names: &[String], name: NameRef, path: &str) -> Result<(), PropertyError> {
    if usize::try_from(name.index().get())
        .ok()
        .is_some_and(|index| index < names.len())
    {
        Ok(())
    } else {
        Err(PropertyError::new(
            PropertyErrorKind::MalformedData,
            None,
            path,
            format!("name index {} is outside name map", name.index().get()),
        ))
    }
}

fn resolve_name_ref(names: &[String], name: NameRef) -> Result<String, PropertyError> {
    validate_name_ref(names, name, "NameRef")?;
    let base = &names[usize::try_from(name.index().get()).expect("u32 fits in usize")];
    if name.number() == 0 {
        Ok(base.clone())
    } else {
        Ok(format!("{}_{}", base, name.number() - 1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::{ArchiveErrorKind, ArchiveLimits, Reader};
    use crate::test_support::{
        TypeParam, name_ref, push_i32, ue5_versions, write_property_tag, write_property_terminator,
        write_type_name,
    };

    fn names() -> Vec<String> {
        vec![
            "None".into(),
            "IntProperty".into(),
            "EnumProperty".into(),
            "EnumValue".into(),
            "MyEnum::Alpha".into(),
            "ArrayProperty".into(),
            "NameProperty".into(),
            "Shop".into(),
            "Forge".into(),
        ]
    }

    #[test]
    fn reads_tagged_property_stream_with_complete_type_names() {
        let mut bytes = Vec::new();
        let payload = 42_i32.to_le_bytes();
        write_property_tag(
            &mut bytes,
            3,
            &TypeParam {
                type_index: 1,
                parameters: Vec::new(),
            },
            0,
            &payload,
        );
        write_property_terminator(&mut bytes, 0);

        let mut reader = Reader::new(&bytes);
        let stream = read_tagged_property_stream(&mut reader, &ue5_versions(), &names(), "Test")
            .expect("parse stream");

        assert_eq!(stream.records.len(), 1);
        assert_eq!(stream.records[0].payload.len(), 4);
        assert_eq!(stream.records[0].type_name.name, name_ref(1, 0));
        assert_eq!(reader.tell(), bytes.len() as u64);
    }

    #[test]
    fn reads_optional_tag_fields_and_property_extensions() {
        let mut bytes = Vec::new();
        push_i32(&mut bytes, 3); // property name
        push_i32(&mut bytes, 0);
        write_type_name(
            &mut bytes,
            &TypeParam {
                type_index: 1,
                parameters: Vec::new(),
            },
        );
        push_i32(&mut bytes, 4); // payload size
        bytes.push(
            TAG_FLAG_HAS_ARRAY_INDEX
                | TAG_FLAG_HAS_PROPERTY_GUID
                | TAG_FLAG_HAS_PROPERTY_EXTENSIONS,
        );
        push_i32(&mut bytes, 7); // array index
        for value in [1_u32, 2, 3, 4] {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.push(PROPERTY_EXTENSION_OVERRIDABLE_INFORMATION);
        bytes.push(9); // override operation
        bytes.extend_from_slice(&1_u32.to_le_bytes());
        bytes.extend_from_slice(&42_i32.to_le_bytes());
        write_property_terminator(&mut bytes, 0);

        let mut reader = Reader::new(&bytes);
        let stream = read_tagged_property_stream(&mut reader, &ue5_versions(), &names(), "Test")
            .expect("parse optional fields");
        let record = &stream.records[0];

        assert_eq!(record.array_index, 7);
        assert_eq!(
            record.property_guid,
            Some(crate::archive::Guid {
                a: 1,
                b: 2,
                c: 3,
                d: 4,
            })
        );
        assert_eq!(
            record.extensions,
            Some(PropertyTagExtensions {
                raw_flags: PROPERTY_EXTENSION_OVERRIDABLE_INFORMATION,
                override_operation: Some(9),
                experimental_overridable_logic: Some(true),
            })
        );
        assert_eq!(record.payload.len(), 4);
        assert_eq!(reader.tell(), bytes.len() as u64);
    }

    #[test]
    fn reads_root_class_serialization_control_extensions() {
        let mut bytes = vec![CLASS_EXTENSION_OVERRIDABLE_SERIALIZATION_INFORMATION, 5];
        write_property_terminator(&mut bytes, 0);
        let mut reader = Reader::new(&bytes);

        let stream =
            read_uobject_tagged_property_stream(&mut reader, &ue5_versions(), &names(), "Object")
                .expect("parse root extensions");

        assert_eq!(
            stream.class_extensions,
            Some(ClassSerializationControlExtensions {
                raw_flags: CLASS_EXTENSION_OVERRIDABLE_SERIALIZATION_INFORMATION,
                overridable_operation: Some(5),
            })
        );
        assert!(stream.records.is_empty());
        assert_eq!(reader.tell(), bytes.len() as u64);
    }

    #[test]
    fn rejects_reserved_property_and_class_extension_groups() {
        let mut class_bytes = vec![CLASS_EXTENSION_RESERVE_FOR_FUTURE_USE];
        write_property_terminator(&mut class_bytes, 0);
        let class_error = read_uobject_tagged_property_stream(
            &mut Reader::new(&class_bytes),
            &ue5_versions(),
            &names(),
            "Object",
        )
        .unwrap_err();
        assert_eq!(class_error.kind(), PropertyErrorKind::UnsupportedCapability);

        let mut property_bytes = Vec::new();
        push_i32(&mut property_bytes, 3);
        push_i32(&mut property_bytes, 0);
        write_type_name(
            &mut property_bytes,
            &TypeParam {
                type_index: 1,
                parameters: Vec::new(),
            },
        );
        push_i32(&mut property_bytes, 0);
        property_bytes.push(TAG_FLAG_HAS_PROPERTY_EXTENSIONS);
        property_bytes.push(PROPERTY_EXTENSION_RESERVE_FOR_FUTURE_USE);
        let property_error = read_tagged_property_stream(
            &mut Reader::new(&property_bytes),
            &ue5_versions(),
            &names(),
            "Test",
        )
        .unwrap_err();
        assert_eq!(
            property_error.kind(),
            PropertyErrorKind::UnsupportedCapability
        );
    }

    #[test]
    fn reads_array_property_inner_type_parameters() {
        let mut bytes = Vec::new();
        write_type_name(
            &mut bytes,
            &TypeParam {
                type_index: 5,
                parameters: vec![TypeParam {
                    type_index: 6,
                    parameters: Vec::new(),
                }],
            },
        );
        let mut reader = Reader::new(&bytes);
        let type_name =
            read_property_type_name_for_test(&mut reader, &names()).expect("read type name");
        assert_eq!(type_name.name, name_ref(5, 0));
        assert_eq!(type_name.parameters.len(), 1);
        assert_eq!(type_name.parameters[0].name, name_ref(6, 0));
    }

    #[test]
    fn reads_multiple_records_through_the_terminator() {
        let mut bytes = Vec::new();
        for (name_index, value) in [(3, 10_i32), (4, 20_i32)] {
            write_property_tag(
                &mut bytes,
                name_index,
                &TypeParam {
                    type_index: 1,
                    parameters: Vec::new(),
                },
                0,
                &value.to_le_bytes(),
            );
        }
        write_property_terminator(&mut bytes, 0);
        let mut reader = Reader::new(&bytes);

        let stream = read_tagged_property_stream(&mut reader, &ue5_versions(), &names(), "Test")
            .expect("parse records");

        assert_eq!(stream.records.len(), 2);
        assert_eq!(stream.records[0].name, name_ref(3, 0));
        assert_eq!(stream.records[1].name, name_ref(4, 0));
        assert_eq!(stream.terminator.len(), 8);
        assert_eq!(reader.tell(), bytes.len() as u64);
    }

    #[test]
    fn rejects_pre_complete_type_name_versions() {
        let versions = VersionContext {
            ue5: UE5_PROPERTY_TAG_COMPLETE_TYPE_NAME - 1,
            ..ue5_versions()
        };
        let error = read_tagged_property_stream(&mut Reader::new(&[]), &versions, &names(), "Test")
            .unwrap_err();

        assert_eq!(error.kind(), PropertyErrorKind::UnsupportedVersion);
        assert_eq!(error.offset(), Some(0));
    }

    #[test]
    fn rejects_negative_property_type_inner_count() {
        let mut bytes = Vec::new();
        push_i32(&mut bytes, 1);
        push_i32(&mut bytes, 0);
        push_i32(&mut bytes, -1);
        let error =
            read_property_type_name_for_test(&mut Reader::new(&bytes), &names()).unwrap_err();

        assert_eq!(error.kind(), PropertyErrorKind::MalformedData);
        assert!(error.path().ends_with("InnerCount"));
    }

    #[test]
    fn rejects_overly_deep_property_type_names() {
        fn write_nested_type(bytes: &mut Vec<u8>, depth: usize) {
            push_i32(bytes, 5);
            push_i32(bytes, 0);
            if depth == 0 {
                push_i32(bytes, 0);
            } else {
                push_i32(bytes, 1);
                write_nested_type(bytes, depth - 1);
            }
        }

        let mut bytes = Vec::new();
        write_nested_type(&mut bytes, MAX_PROPERTY_TYPE_DEPTH);
        let mut reader = Reader::new(&bytes);

        let error = read_property_type_name_for_test(&mut reader, &names())
            .expect_err("depth limit should reject nested type names");

        assert_eq!(error.kind(), PropertyErrorKind::MalformedData);
        assert!(error.detail().contains("depth limit"));
    }

    #[test]
    fn rejects_negative_property_payload_size() {
        let mut bytes = Vec::new();
        push_i32(&mut bytes, 3);
        push_i32(&mut bytes, 0);
        write_type_name(
            &mut bytes,
            &TypeParam {
                type_index: 1,
                parameters: Vec::new(),
            },
        );
        push_i32(&mut bytes, -1);

        let mut reader = Reader::new(&bytes);
        let error = read_tagged_property_stream(&mut reader, &ue5_versions(), &names(), "Test")
            .expect_err("negative size");
        assert_eq!(error.kind(), PropertyErrorKind::MalformedData);
        assert!(error.path().contains("Tag.Size"));
    }

    #[test]
    fn maps_archive_allocation_limit_as_resource_limit() {
        let limits = ArchiveLimits {
            max_array_elements: 10,
            max_allocation_bytes: 7,
            ..ArchiveLimits::default()
        };
        let bytes = 2_i32.to_le_bytes();
        let archive_error = Reader::with_limits(&bytes, limits)
            .read_tarray::<u32>("Values", 4, |reader, _| reader.read_u32("Value"))
            .unwrap_err();
        assert_eq!(archive_error.kind(), ArchiveErrorKind::AllocationLimit);

        let error = PropertyError::from(archive_error);

        assert_eq!(error.kind(), PropertyErrorKind::ResourceLimit);
        assert_eq!(error.path(), "Values.Count");
    }

    fn read_property_type_name_for_test(
        reader: &mut Reader<'_>,
        names: &[String],
    ) -> Result<PropertyTypeName, PropertyError> {
        read_property_type_name(reader, names, "Test.Type")
    }
}
