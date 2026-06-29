//! Semantic property decoding seam.

use crate::archive::Reader;
use crate::package::{Package, PackageIndex};
use crate::property::{
    MapEntry, PropertyError, PropertyRecord, PropertyStream, PropertyTagFlags, PropertyTypeName,
    PropertyValue, RawReason, TextValue, VectorValue, read_tagged_property_stream,
};

/// UE `INDEX_NONE` marks a full container replace in map property payloads.
const INDEX_NONE: i32 = -1;
use crate::schema::SchemaProvider;
use crate::version::VersionContext;

pub struct DecodeContext<'a> {
    pub package: &'a Package,
    pub versions: &'a VersionContext,
    pub schemas: &'a dyn SchemaProvider,
}

/// Decodes supported property payloads in place.
///
/// Unsupported property types remain represented as raw payload spans. Malformed
/// supported payloads return an error rather than desynchronizing the stream.
///
/// # Errors
///
/// Returns an error when a supported property payload is not fully consumed,
/// has the wrong byte size, or contains malformed primitive data.
pub fn decode_property_stream_values(
    source: &[u8],
    stream: &mut PropertyStream,
    context: &DecodeContext<'_>,
) -> Result<(), PropertyError> {
    for record in &mut stream.records {
        decode_property_record(source, record, context)?;
    }
    Ok(())
}

fn decode_property_record(
    source: &[u8],
    record: &mut PropertyRecord,
    context: &DecodeContext<'_>,
) -> Result<(), PropertyError> {
    if record.flags.is_skipped() {
        record.value = PropertyValue::Raw {
            reason: RawReason::DecoderRejected("property serialization was skipped".to_owned()),
        };
        return Ok(());
    }

    let Some(type_name) = context.package.resolve_name(record.type_name.name) else {
        record.value = PropertyValue::Raw {
            reason: RawReason::DecoderRejected("unresolved property type name".to_owned()),
        };
        return Ok(());
    };

    let reader = Reader::new(source);
    let mut payload = reader.bounded(record.payload, "Property.Payload")?;
    let property_name = context
        .package
        .resolve_name(record.name)
        .unwrap_or_else(|| "Property".to_owned());
    let path = format!("Property.{property_name}");

    let decoded = if record.flags.is_binary_or_native() {
        match decode_binary_or_native_value(
            &type_name,
            &record.type_name,
            &mut payload,
            context,
            &path,
        ) {
            Ok(Some(value)) => value,
            Ok(None) => {
                record.value = PropertyValue::Raw {
                    reason: RawReason::UnsupportedType,
                };
                return Ok(());
            }
            Err(error) => return Err(error),
        }
    } else {
        match decode_typed_value(
            source,
            &type_name,
            &record.type_name,
            record.flags,
            &mut payload,
            context,
            &path,
        ) {
            Ok(Some(value)) => value,
            Ok(None) => {
                record.value = PropertyValue::Raw {
                    reason: RawReason::UnsupportedType,
                };
                return Ok(());
            }
            Err(error) => return Err(error),
        }
    };

    if payload.remaining() != 0 {
        record.value = PropertyValue::Raw {
            reason: RawReason::DecoderRejected(format!(
                "{} trailing bytes left in decoded {type_name} payload",
                payload.remaining()
            )),
        };
        return Ok(());
    }

    record.value = decoded;
    Ok(())
}

fn decode_typed_value(
    source: &[u8],
    type_name: &str,
    type_tree: &PropertyTypeName,
    flags: PropertyTagFlags,
    payload: &mut Reader<'_>,
    context: &DecodeContext<'_>,
    path: &str,
) -> Result<Option<PropertyValue>, PropertyError> {
    match type_name {
        "BoolProperty" => Ok(Some(PropertyValue::Bool(flags.bool_value()))),
        "Int8Property" => Ok(Some(PropertyValue::Int(i64::from(
            payload.read_i8(&format!("{path}.Int8"))?,
        )))),
        "Int16Property" => Ok(Some(PropertyValue::Int(i64::from(
            payload.read_i16(&format!("{path}.Int16"))?,
        )))),
        "IntProperty" | "Int32Property" => Ok(Some(PropertyValue::Int(i64::from(
            payload.read_i32(&format!("{path}.Int32"))?,
        )))),
        "Int64Property" => Ok(Some(PropertyValue::Int(
            payload.read_i64(&format!("{path}.Int64"))?,
        ))),
        "ByteProperty" | "UInt8Property" => Ok(Some(PropertyValue::UInt(u64::from(
            payload.read_u8(&format!("{path}.UInt8"))?,
        )))),
        "UInt16Property" => Ok(Some(PropertyValue::UInt(u64::from(
            payload.read_u16(&format!("{path}.UInt16"))?,
        )))),
        "UInt32Property" => Ok(Some(PropertyValue::UInt(u64::from(
            payload.read_u32(&format!("{path}.UInt32"))?,
        )))),
        "UInt64Property" => Ok(Some(PropertyValue::UInt(
            payload.read_u64(&format!("{path}.UInt64"))?,
        ))),
        "FloatProperty" => Ok(Some(PropertyValue::Float(
            payload.read_f32(&format!("{path}.Float"))?,
        ))),
        "DoubleProperty" => Ok(Some(PropertyValue::Double(
            payload.read_f64(&format!("{path}.Double"))?,
        ))),
        "NameProperty" => Ok(Some(PropertyValue::Name(
            payload.read_name_ref(&format!("{path}.Name"))?,
        ))),
        "EnumProperty" => Ok(Some(PropertyValue::Enum(
            payload.read_name_ref(&format!("{path}.Enum"))?,
        ))),
        "StrProperty" => Ok(Some(PropertyValue::String(
            payload.read_fstring(&format!("{path}.String"))?,
        ))),
        "TextProperty" => decode_text_value(payload, path).map(|text| text.map(PropertyValue::Text)),
        "ObjectProperty" | "ClassProperty" | "WeakObjectProperty" | "LazyObjectProperty" => {
            Ok(Some(PropertyValue::ObjectRef(PackageIndex::from_raw(
                payload.read_i32(&format!("{path}.ObjectRef"))?,
            ))))
        }
        "SoftObjectProperty" => Ok(Some(PropertyValue::SoftObjectPath(
            decode_soft_object_path(payload, path, context)?,
        ))),
        "ArrayProperty" => decode_array_value(source, type_tree, payload, context, path).map(Some),
        "SetProperty" => decode_set_value(source, type_tree, payload, context, path).map(Some),
        "MapProperty" => decode_map_value(source, type_tree, payload, context, path).map(Some),
        "StructProperty" => decode_struct_value(source, payload, context, path).map(Some),
        _ => Ok(None),
    }
}

/// Decodes `FSoftObjectPath` / `TSoftObjectPtr` wire format.
///
/// Editor packages with a package-level soft object path table store a 4-byte
/// index into that table. Otherwise the path is serialized inline as `FString`
/// asset path plus optional `FUtf8String` subpath.
fn decode_soft_object_path(
    payload: &mut Reader<'_>,
    path: &str,
    context: &DecodeContext<'_>,
) -> Result<String, PropertyError> {
    if !context.package.soft_object_paths.is_empty() && payload.remaining() == 4 {
        let index = payload.read_i32(&format!("{path}.SoftObjectPathIndex"))?;
        if index < 0 {
            return Ok(String::new());
        }
        let index = usize::try_from(index).map_err(|error| {
            PropertyError::new(
                crate::property::PropertyErrorKind::MalformedData,
                Some(payload.tell()),
                path,
                format!("soft object path index does not fit in usize: {error}"),
            )
        })?;
        return context
            .package
            .soft_object_paths
            .get(index)
            .cloned()
            .ok_or_else(|| {
                PropertyError::new(
                    crate::property::PropertyErrorKind::MalformedData,
                    Some(payload.tell()),
                    path,
                    format!(
                        "soft object path index {index} is out of range (table size {})",
                        context.package.soft_object_paths.len()
                    ),
                )
            });
    }

    payload
        .read_soft_object_path(path)
        .map_err(PropertyError::from)
}

fn decode_binary_or_native_value(
    type_name: &str,
    type_tree: &PropertyTypeName,
    payload: &mut Reader<'_>,
    context: &DecodeContext<'_>,
    path: &str,
) -> Result<Option<PropertyValue>, PropertyError> {
    if type_name == "StructProperty"
        && resolve_struct_type_name(context.package, type_tree).as_deref() == Some("Vector")
    {
        return Ok(Some(PropertyValue::Vector(decode_vector_value(
            payload, path,
        )?)));
    }

    Ok(None)
}

fn decode_array_value(
    source: &[u8],
    type_tree: &PropertyTypeName,
    payload: &mut Reader<'_>,
    context: &DecodeContext<'_>,
    path: &str,
) -> Result<PropertyValue, PropertyError> {
    let (inner_type, inner_name) = resolve_inner_type(context, type_tree, path, "ArrayProperty")?;

    let count = payload.read_count(&format!("{path}.Count"))?;
    let mut values = Vec::with_capacity(count);
    for index in 0..count {
        let element_path = format!("{path}[{index}]");
        values.push(
            decode_typed_value(
                source,
                &inner_name,
                inner_type,
                PropertyTagFlags::default(),
                payload,
                context,
                &element_path,
            )?
            .ok_or_else(|| {
                PropertyError::new(
                    crate::property::PropertyErrorKind::MalformedData,
                    Some(payload.tell()),
                    &element_path,
                    format!("unsupported array element type {inner_name}"),
                )
            })?,
        );
    }
    Ok(PropertyValue::Array(values))
}

fn decode_set_value(
    source: &[u8],
    type_tree: &PropertyTypeName,
    payload: &mut Reader<'_>,
    context: &DecodeContext<'_>,
    path: &str,
) -> Result<PropertyValue, PropertyError> {
    let (element_type, element_name) = resolve_inner_type(context, type_tree, path, "SetProperty")?;

    let remove_count = payload.read_i32(&format!("{path}.ElementsToRemove.Count"))?;
    if remove_count < 0 {
        return Err(PropertyError::new(
            crate::property::PropertyErrorKind::MalformedData,
            Some(payload.tell()),
            path,
            format!("set ElementsToRemove count must be non-negative, got {remove_count}"),
        ));
    }
    for index in 0..remove_count {
        let element_path = format!("{path}.ElementsToRemove[{index}]");
        decode_typed_value(
            source,
            &element_name,
            element_type,
            PropertyTagFlags::default(),
            payload,
            context,
            &element_path,
        )?
        .ok_or_else(|| {
            PropertyError::new(
                crate::property::PropertyErrorKind::MalformedData,
                Some(payload.tell()),
                &element_path,
                format!("unsupported set element type {element_name}"),
            )
        })?;
    }

    let count = payload.read_count(&format!("{path}.Elements.Count"))?;
    let mut values = Vec::with_capacity(count);
    for index in 0..count {
        let element_path = format!("{path}.Elements[{index}]");
        values.push(
            decode_typed_value(
                source,
                &element_name,
                element_type,
                PropertyTagFlags::default(),
                payload,
                context,
                &element_path,
            )?
            .ok_or_else(|| {
                PropertyError::new(
                    crate::property::PropertyErrorKind::MalformedData,
                    Some(payload.tell()),
                    &element_path,
                    format!("unsupported set element type {element_name}"),
                )
            })?,
        );
    }
    Ok(PropertyValue::Set(values))
}

fn decode_map_value(
    source: &[u8],
    type_tree: &PropertyTypeName,
    payload: &mut Reader<'_>,
    context: &DecodeContext<'_>,
    path: &str,
) -> Result<PropertyValue, PropertyError> {
    let (key_type, key_name) = resolve_map_key_type(context, type_tree, path)?;
    let (value_type, value_name) = resolve_map_value_type(context, type_tree, path)?;

    let keys_to_remove = payload.read_i32(&format!("{path}.KeysToRemove.Count"))?;
    if keys_to_remove > 0 {
        for index in 0..keys_to_remove {
            let key_path = format!("{path}.KeysToRemove[{index}]");
            decode_typed_value(
                source,
                &key_name,
                key_type,
                PropertyTagFlags::default(),
                payload,
                context,
                &key_path,
            )?
            .ok_or_else(|| {
                PropertyError::new(
                    crate::property::PropertyErrorKind::MalformedData,
                    Some(payload.tell()),
                    &key_path,
                    format!("unsupported map key type {key_name}"),
                )
            })?;
        }
    } else if keys_to_remove != 0 && keys_to_remove != INDEX_NONE {
        return Err(PropertyError::new(
            crate::property::PropertyErrorKind::MalformedData,
            Some(payload.tell()),
            path,
            format!("unexpected map KeysToRemove count {keys_to_remove}"),
        ));
    }

    let count = payload.read_count(&format!("{path}.Entries.Count"))?;
    let mut entries = Vec::with_capacity(count);
    for index in 0..count {
        let entry_path = format!("{path}.Entries[{index}]");
        let key_path = format!("{entry_path}.Key");
        let value_path = format!("{entry_path}.Value");
        let key = decode_typed_value(
            source,
            &key_name,
            key_type,
            PropertyTagFlags::default(),
            payload,
            context,
            &key_path,
        )?
        .ok_or_else(|| {
            PropertyError::new(
                crate::property::PropertyErrorKind::MalformedData,
                Some(payload.tell()),
                &key_path,
                format!("unsupported map key type {key_name}"),
            )
        })?;
        let value = decode_typed_value(
            source,
            &value_name,
            value_type,
            PropertyTagFlags::default(),
            payload,
            context,
            &value_path,
        )?
        .ok_or_else(|| {
            PropertyError::new(
                crate::property::PropertyErrorKind::MalformedData,
                Some(payload.tell()),
                &value_path,
                format!("unsupported map value type {value_name}"),
            )
        })?;
        entries.push(MapEntry { key, value });
    }
    Ok(PropertyValue::Map(entries))
}

fn resolve_inner_type<'a>(
    context: &DecodeContext<'_>,
    type_tree: &'a PropertyTypeName,
    path: &str,
    property_kind: &str,
) -> Result<(&'a PropertyTypeName, String), PropertyError> {
    let Some(inner_type) = type_tree.parameters.first() else {
        return Err(PropertyError::new(
            crate::property::PropertyErrorKind::MalformedData,
            Some(0),
            path,
            format!("{property_kind} is missing its inner type parameter"),
        ));
    };
    let Some(inner_name) = context.package.resolve_name(inner_type.name) else {
        return Err(PropertyError::new(
            crate::property::PropertyErrorKind::MalformedData,
            Some(0),
            path,
            format!("{property_kind} has an unresolved inner type name"),
        ));
    };
    Ok((inner_type, inner_name))
}

fn resolve_map_key_type<'a>(
    context: &DecodeContext<'_>,
    type_tree: &'a PropertyTypeName,
    path: &str,
) -> Result<(&'a PropertyTypeName, String), PropertyError> {
    let Some(key_type) = type_tree.parameters.first() else {
        return Err(PropertyError::new(
            crate::property::PropertyErrorKind::MalformedData,
            Some(0),
            path,
            "MapProperty is missing its key type parameter",
        ));
    };
    let Some(key_name) = context.package.resolve_name(key_type.name) else {
        return Err(PropertyError::new(
            crate::property::PropertyErrorKind::MalformedData,
            Some(0),
            path,
            "MapProperty has an unresolved key type name",
        ));
    };
    Ok((key_type, key_name))
}

fn resolve_map_value_type<'a>(
    context: &DecodeContext<'_>,
    type_tree: &'a PropertyTypeName,
    path: &str,
) -> Result<(&'a PropertyTypeName, String), PropertyError> {
    let Some(value_type) = type_tree.parameters.get(1) else {
        return Err(PropertyError::new(
            crate::property::PropertyErrorKind::MalformedData,
            Some(0),
            path,
            "MapProperty is missing its value type parameter",
        ));
    };
    let Some(value_name) = context.package.resolve_name(value_type.name) else {
        return Err(PropertyError::new(
            crate::property::PropertyErrorKind::MalformedData,
            Some(0),
            path,
            "MapProperty has an unresolved value type name",
        ));
    };
    Ok((value_type, value_name))
}

fn decode_struct_value(
    source: &[u8],
    payload: &mut Reader<'_>,
    context: &DecodeContext<'_>,
    path: &str,
) -> Result<PropertyValue, PropertyError> {
    let mut stream = read_tagged_property_stream(
        payload,
        context.versions,
        &context.package.names,
        path,
    )?;
    decode_property_stream_values(source, &mut stream, context)?;
    Ok(PropertyValue::Struct(stream))
}

fn decode_text_value(
    payload: &mut Reader<'_>,
    path: &str,
) -> Result<Option<TextValue>, PropertyError> {
    let _flags = payload.read_i32(&format!("{path}.Flags"))?;
    let history_type = payload.read_i8(&format!("{path}.HistoryType"))?;

    if history_type == -1 {
        let _has_culture_invariant =
            read_archive_bool(payload, &format!("{path}.CultureInvariant"))?;
        return Ok(Some(TextValue {
            source: String::new(),
        }));
    }

    if history_type == 0 {
        let _namespace = payload.read_fstring(&format!("{path}.Namespace"))?;
        let _key = payload.read_fstring(&format!("{path}.Key"))?;
        let source = payload.read_fstring(&format!("{path}.SourceString"))?;
        return Ok(Some(TextValue { source }));
    }

    Ok(None)
}

fn decode_vector_value(payload: &mut Reader<'_>, path: &str) -> Result<VectorValue, PropertyError> {
    match payload.remaining() {
        12 => Ok(VectorValue {
            x: payload.read_f32(&format!("{path}.X"))?,
            y: payload.read_f32(&format!("{path}.Y"))?,
            z: payload.read_f32(&format!("{path}.Z"))?,
        }),
        24 => Ok(VectorValue {
            x: payload.read_f64(&format!("{path}.X"))? as f32,
            y: payload.read_f64(&format!("{path}.Y"))? as f32,
            z: payload.read_f64(&format!("{path}.Z"))? as f32,
        }),
        remaining => Err(PropertyError::new(
            crate::property::PropertyErrorKind::MalformedData,
            Some(payload.tell()),
            path,
            format!("unsupported FVector payload size {remaining}"),
        )),
    }
}

fn resolve_struct_type_name(package: &Package, type_tree: &PropertyTypeName) -> Option<String> {
    package.resolve_name(type_tree.parameters.first()?.name)
}

fn read_archive_bool(reader: &mut Reader<'_>, path: &str) -> Result<bool, PropertyError> {
    let offset = reader.tell();
    match reader.read_u32(path)? {
        0 => Ok(false),
        1 => Ok(true),
        value => Err(PropertyError::new(
            crate::property::PropertyErrorKind::MalformedData,
            Some(offset),
            path,
            format!("serialized bool must be 0 or 1, got {value}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive::{Reader, Span};
    use crate::property::{
        PropertyRecord, PropertyStream, PropertyTagFlags, PropertyTypeName, PropertyValue, RawReason,
        TextValue, VectorValue, read_tagged_property_stream,
    };
    use crate::schema::{ClassSchema, SchemaProvider, StructSchema};
    use crate::package::test_package;
    use crate::test_support::{
        TypeParam, push_f32, push_f64, push_fstring, push_i32, ue5_versions,
        write_property_tag, write_property_terminator,
    };

    struct EmptySchemas;

    impl SchemaProvider for EmptySchemas {
        fn find_struct(&self, _path: &crate::package::ObjectPath) -> Option<&StructSchema> {
            None
        }

        fn find_class(&self, _path: &crate::package::ObjectPath) -> Option<&ClassSchema> {
            None
        }
    }

    fn decode_record(
        names: Vec<String>,
        type_index: i32,
        type_params: Vec<PropertyTypeName>,
        flags: PropertyTagFlags,
        payload: &[u8],
    ) -> PropertyValue {
        let source = payload.to_vec();
        let record = PropertyRecord {
            name: crate::test_support::name_ref(0, 0),
            type_name: PropertyTypeName {
                name: crate::test_support::name_ref(type_index, 0),
                parameters: type_params,
            },
            array_index: 0,
            flags,
            property_guid: None,
            extensions: None,
            payload: Span::new(0, source.len() as u64).expect("payload span"),
            value: PropertyValue::Raw {
                reason: RawReason::UnsupportedType,
            },
        };
        let package = test_package(names);
        let schemas = EmptySchemas;
        let context = DecodeContext {
            package: &package,
            versions: &package.summary.versions,
            schemas: &schemas,
        };
        let mut record = record;
        decode_property_record(&source, &mut record, &context).expect("decode record");
        record.value
    }

    fn decode_stream(payload: &[u8]) -> PropertyStream {
        let bytes = payload.to_vec();
        let names = vec![
            "None".into(),
            "NestedInt".into(),
            "IntProperty".into(),
            "NestedVector".into(),
            "StructProperty".into(),
            "Vector".into(),
        ];
        let mut reader = Reader::new(&bytes);
        let mut stream =
            read_tagged_property_stream(&mut reader, &ue5_versions(), &names, "Test.Struct")
                .expect("parse struct stream");
        let package = test_package(names);
        let schemas = EmptySchemas;
        let context = DecodeContext {
            package: &package,
            versions: &package.summary.versions,
            schemas: &schemas,
        };
        decode_property_stream_values(&bytes, &mut stream, &context).expect("decode struct");
        stream
    }

    #[test]
    fn decodes_enum_payload_as_name_literal() {
        let names = vec![
            "None".into(),
            "EnumProperty".into(),
            "MyEnum::Alpha".into(),
        ];
        let mut payload = Vec::new();
        push_i32(&mut payload, 2);
        push_i32(&mut payload, 0);

        let package = test_package(names.clone());
        let value = decode_record(names, 1, Vec::new(), PropertyTagFlags(0), &payload);
        let PropertyValue::Enum(name) = value else {
            panic!("expected enum, got {value:?}");
        };
        assert_eq!(package.resolve_name(name), Some("MyEnum::Alpha".to_owned()));
    }

    #[test]
    fn decodes_weak_object_payload_as_package_index() {
        let names = vec!["WeakObjectProperty".into()];
        let payload = (-2_i32).to_le_bytes();

        let value = decode_record(names, 0, Vec::new(), PropertyTagFlags(0), &payload);

        assert_eq!(
            value,
            PropertyValue::ObjectRef(PackageIndex::from_raw(-2))
        );
    }

    #[test]
    fn decodes_lazy_object_payload_as_package_index() {
        let names = vec!["LazyObjectProperty".into()];
        let payload = (-3_i32).to_le_bytes();

        let value = decode_record(names, 0, Vec::new(), PropertyTagFlags(0), &payload);

        assert_eq!(
            value,
            PropertyValue::ObjectRef(PackageIndex::from_raw(-3))
        );
    }

    #[test]
    fn decodes_empty_text_payload() {
        let names = vec!["TextProperty".into()];
        let payload = [0, 0, 0, 0, 0xFF, 0, 0, 0, 0];
        let value = decode_record(names, 0, Vec::new(), PropertyTagFlags(0), &payload);
        assert_eq!(
            value,
            PropertyValue::Text(TextValue {
                source: String::new(),
            })
        );
    }

    #[test]
    fn decodes_keyed_text_payload() {
        let names = vec!["TextProperty".into()];
        let mut payload = Vec::new();
        push_i32(&mut payload, 0); // flags
        payload.push(0); // Base history
        push_fstring(&mut payload, ""); // namespace
        push_fstring(&mut payload, "deadbeef");
        push_fstring(&mut payload, "Hello");

        let value = decode_record(names, 0, Vec::new(), PropertyTagFlags(0), &payload);
        assert_eq!(
            value,
            PropertyValue::Text(TextValue {
                source: "Hello".to_owned(),
            })
        );
    }

    #[test]
    fn decodes_name_array_payload() {
        let names = vec![
            "ArrayProperty".into(),
            "NameProperty".into(),
            "Alpha".into(),
            "Beta".into(),
        ];
        let mut payload = Vec::new();
        push_i32(&mut payload, 2);
        push_i32(&mut payload, 2);
        push_i32(&mut payload, 0);
        push_i32(&mut payload, 3);
        push_i32(&mut payload, 0);

        let value = decode_record(
            names,
            0,
            vec![PropertyTypeName {
                name: crate::test_support::name_ref(1, 0),
                parameters: Vec::new(),
            }],
            PropertyTagFlags(0),
            &payload,
        );
        let PropertyValue::Array(values) = value else {
            panic!("expected array, got {value:?}");
        };
        assert_eq!(values.len(), 2);
    }

    #[test]
    fn decodes_nested_struct_and_binary_vector_payloads() {
        let mut struct_bytes = Vec::new();
        let int_payload = 7_i32.to_le_bytes();
        write_property_tag(
            &mut struct_bytes,
            1,
            &TypeParam {
                type_index: 2,
                parameters: Vec::new(),
            },
            0,
            &int_payload,
        );
        let mut vector_payload = Vec::new();
        push_f64(&mut vector_payload, 1.0);
        push_f64(&mut vector_payload, 2.0);
        push_f64(&mut vector_payload, 3.0);
        write_property_tag(
            &mut struct_bytes,
            3,
            &TypeParam {
                type_index: 4,
                parameters: vec![TypeParam {
                    type_index: 5,
                    parameters: Vec::new(),
                }],
            },
            0x08, // binary/native
            &vector_payload,
        );
        write_property_terminator(&mut struct_bytes, 0);

        let stream = decode_stream(&struct_bytes);
        assert_eq!(stream.records.len(), 2);
        assert_eq!(stream.records[0].value, PropertyValue::Int(7));
        assert_eq!(
            stream.records[1].value,
            PropertyValue::Vector(VectorValue {
                x: 1.0,
                y: 2.0,
                z: 3.0,
            })
        );
    }

    #[test]
    fn reports_raw_when_enum_payload_has_trailing_bytes() {
        let names = vec!["EnumProperty".into(), "MyEnum::Alpha".into()];
        let mut payload = Vec::new();
        push_i32(&mut payload, 1);
        push_i32(&mut payload, 0);
        payload.push(0xFF);

        let value = decode_record(names, 0, Vec::new(), PropertyTagFlags(0), &payload);
        assert!(matches!(
            value,
            PropertyValue::Raw {
                reason: RawReason::DecoderRejected(_),
            }
        ));
    }

    #[test]
    fn decodes_populated_soft_object_path_payload() {
        let names = vec!["SoftObjectProperty".into()];
        let mut payload = Vec::new();
        push_fstring(
            &mut payload,
            "/Engine/EngineResources/DefaultTexture.DefaultTexture",
        );
        push_fstring(&mut payload, "");

        let value = decode_record(names, 0, Vec::new(), PropertyTagFlags(0), &payload);
        assert_eq!(
            value,
            PropertyValue::SoftObjectPath(
                "/Engine/EngineResources/DefaultTexture.DefaultTexture".into()
            )
        );
    }

    #[test]
    fn decodes_soft_object_path_with_subpath() {
        let names = vec!["SoftObjectProperty".into()];
        let mut payload = Vec::new();
        push_fstring(&mut payload, "/Game/MyPackage.MyAsset");
        push_fstring(&mut payload, "SubObject");

        let value = decode_record(names, 0, Vec::new(), PropertyTagFlags(0), &payload);
        assert_eq!(
            value,
            PropertyValue::SoftObjectPath("/Game/MyPackage.MyAsset:SubObject".into())
        );
    }

    #[test]
    fn decodes_indexed_soft_object_path_payload() {
        let names = vec!["SoftObjectProperty".into()];
        let mut package = test_package(names);
        package.soft_object_paths = vec![
            String::new(),
            "/Engine/EngineResources/DefaultTexture.DefaultTexture".into(),
        ];
        let payload = 1_i32.to_le_bytes();
        let source = payload.to_vec();
        let mut record = PropertyRecord {
            name: crate::test_support::name_ref(0, 0),
            type_name: PropertyTypeName {
                name: crate::test_support::name_ref(0, 0),
                parameters: Vec::new(),
            },
            array_index: 0,
            flags: PropertyTagFlags(0),
            property_guid: None,
            extensions: None,
            payload: Span::new(0, source.len() as u64).expect("payload span"),
            value: PropertyValue::Raw {
                reason: RawReason::UnsupportedType,
            },
        };
        let schemas = EmptySchemas;
        let context = DecodeContext {
            package: &package,
            versions: &package.summary.versions,
            schemas: &schemas,
        };
        decode_property_record(&source, &mut record, &context).expect("decode");
        assert_eq!(
            record.value,
            PropertyValue::SoftObjectPath(
                "/Engine/EngineResources/DefaultTexture.DefaultTexture".into()
            )
        );
    }

    #[test]
    fn decodes_name_set_payload() {
        let names = vec![
            "SetProperty".into(),
            "NameProperty".into(),
            "Alpha".into(),
            "Beta".into(),
        ];
        let mut payload = Vec::new();
        push_i32(&mut payload, 0); // ElementsToRemove
        push_i32(&mut payload, 2); // Elements
        push_i32(&mut payload, 2);
        push_i32(&mut payload, 0);
        push_i32(&mut payload, 3);
        push_i32(&mut payload, 0);

        let value = decode_record(
            names,
            0,
            vec![PropertyTypeName {
                name: crate::test_support::name_ref(1, 0),
                parameters: Vec::new(),
            }],
            PropertyTagFlags(0),
            &payload,
        );
        let PropertyValue::Set(values) = value else {
            panic!("expected set, got {value:?}");
        };
        assert_eq!(values.len(), 2);
    }

    #[test]
    fn decodes_int_to_string_map_payload() {
        let names = vec![
            "MapProperty".into(),
            "IntProperty".into(),
            "StrProperty".into(),
        ];
        let mut payload = Vec::new();
        push_i32(&mut payload, 0); // KeysToRemove
        push_i32(&mut payload, 2); // Entries
        push_i32(&mut payload, 1);
        push_fstring(&mut payload, "one");
        push_i32(&mut payload, 2);
        push_fstring(&mut payload, "two");

        let value = decode_record(
            names,
            0,
            vec![
                PropertyTypeName {
                    name: crate::test_support::name_ref(1, 0),
                    parameters: Vec::new(),
                },
                PropertyTypeName {
                    name: crate::test_support::name_ref(2, 0),
                    parameters: Vec::new(),
                },
            ],
            PropertyTagFlags(0),
            &payload,
        );
        let PropertyValue::Map(entries) = value else {
            panic!("expected map, got {value:?}");
        };
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].key, PropertyValue::Int(1));
        assert_eq!(entries[0].value, PropertyValue::String("one".into()));
        assert_eq!(entries[1].key, PropertyValue::Int(2));
        assert_eq!(entries[1].value, PropertyValue::String("two".into()));
    }

    #[test]
    fn decodes_map_with_full_replace_marker() {
        let names = vec![
            "MapProperty".into(),
            "IntProperty".into(),
            "IntProperty".into(),
        ];
        let mut payload = Vec::new();
        push_i32(&mut payload, INDEX_NONE); // KeysToRemove = full replace
        push_i32(&mut payload, 1); // Entries
        push_i32(&mut payload, 42);
        push_i32(&mut payload, 7);

        let value = decode_record(
            names,
            0,
            vec![
                PropertyTypeName {
                    name: crate::test_support::name_ref(1, 0),
                    parameters: Vec::new(),
                },
                PropertyTypeName {
                    name: crate::test_support::name_ref(2, 0),
                    parameters: Vec::new(),
                },
            ],
            PropertyTagFlags(0),
            &payload,
        );
        let PropertyValue::Map(entries) = value else {
            panic!("expected map, got {value:?}");
        };
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].key, PropertyValue::Int(42));
        assert_eq!(entries[0].value, PropertyValue::Int(7));
    }

    #[test]
    fn reports_raw_for_unsupported_property_type() {
        let names = vec!["DelegateProperty".into()];
        let value = decode_record(names, 0, Vec::new(), PropertyTagFlags(0), &[0x01]);
        assert!(matches!(
            value,
            PropertyValue::Raw {
                reason: RawReason::UnsupportedType,
            }
        ));
    }

    #[test]
    fn decodes_fvector_from_float_layout() {
        let names = vec!["StructProperty".into(), "Vector".into()];
        let mut payload = Vec::new();
        push_f32(&mut payload, 4.0);
        push_f32(&mut payload, 5.0);
        push_f32(&mut payload, 6.0);

        let value = decode_record(
            names,
            0,
            vec![PropertyTypeName {
                name: crate::test_support::name_ref(1, 0),
                parameters: Vec::new(),
            }],
            PropertyTagFlags(0x08),
            &payload,
        );
        assert_eq!(
            value,
            PropertyValue::Vector(VectorValue {
                x: 4.0,
                y: 5.0,
                z: 6.0,
            })
        );
    }
}
