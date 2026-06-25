//! Semantic property decoding seam.

use crate::archive::Reader;
use crate::package::{Package, PackageIndex};
use crate::property::{PropertyError, PropertyRecord, PropertyStream, PropertyValue, RawReason};
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
    let decoded = match type_name.as_str() {
        "BoolProperty" => PropertyValue::Bool(record.flags.bool_value()),
        "Int8Property" => PropertyValue::Int(i64::from(payload.read_i8("Property.Int8")?)),
        "Int16Property" => PropertyValue::Int(i64::from(payload.read_i16("Property.Int16")?)),
        "IntProperty" | "Int32Property" => {
            PropertyValue::Int(i64::from(payload.read_i32("Property.Int32")?))
        }
        "Int64Property" => PropertyValue::Int(payload.read_i64("Property.Int64")?),
        "ByteProperty" | "UInt8Property" => {
            PropertyValue::UInt(u64::from(payload.read_u8("Property.UInt8")?))
        }
        "UInt16Property" => PropertyValue::UInt(u64::from(payload.read_u16("Property.UInt16")?)),
        "UInt32Property" => PropertyValue::UInt(u64::from(payload.read_u32("Property.UInt32")?)),
        "UInt64Property" => PropertyValue::UInt(payload.read_u64("Property.UInt64")?),
        "FloatProperty" => PropertyValue::Float(payload.read_f32("Property.Float")?),
        "DoubleProperty" => PropertyValue::Double(payload.read_f64("Property.Double")?),
        "NameProperty" => PropertyValue::Name(payload.read_name_ref("Property.Name")?),
        "StrProperty" => PropertyValue::String(payload.read_fstring("Property.String")?),
        "ObjectProperty" | "ClassProperty" | "SoftObjectProperty" | "WeakObjectProperty"
        | "LazyObjectProperty" => PropertyValue::ObjectRef(PackageIndex::from_raw(
            payload.read_i32("Property.ObjectRef")?,
        )),
        _ => {
            record.value = PropertyValue::Raw {
                reason: RawReason::UnsupportedType,
            };
            return Ok(());
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
