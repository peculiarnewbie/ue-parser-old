//! Synthetic wire-format builders shared by unit tests.

use crate::archive::{NameRef, Reader};
use crate::version::VersionContext;

pub fn push_i32(bytes: &mut Vec<u8>, value: i32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

pub fn push_f32(bytes: &mut Vec<u8>, value: f32) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

pub fn push_f64(bytes: &mut Vec<u8>, value: f64) {
    bytes.extend_from_slice(&value.to_le_bytes());
}

pub fn push_fstring(bytes: &mut Vec<u8>, value: &str) {
    let ansi = format!("{value}\0");
    push_i32(bytes, i32::try_from(ansi.len()).expect("fits in i32"));
    bytes.extend_from_slice(ansi.as_bytes());
}

pub fn name_ref(index: i32, number: i32) -> NameRef {
    let mut bytes = Vec::new();
    push_i32(&mut bytes, index);
    push_i32(&mut bytes, number);
    Reader::new(&bytes)
        .read_name_ref("test.NameRef")
        .expect("name ref")
}

pub fn ue5_versions() -> VersionContext {
    VersionContext {
        legacy_file_version: -9,
        legacy_ue3: None,
        ue4: 522,
        ue5: 1018,
        licensee: 0,
        package_flags: crate::version::PackageFlags::from_bits(0),
    }
}

pub struct TypeParam {
    pub type_index: i32,
    pub parameters: Vec<TypeParam>,
}

pub fn write_type_name(bytes: &mut Vec<u8>, param: &TypeParam) {
    push_i32(bytes, param.type_index);
    push_i32(bytes, 0);
    push_i32(
        bytes,
        i32::try_from(param.parameters.len()).expect("fits in i32"),
    );
    for inner in &param.parameters {
        write_type_name(bytes, inner);
    }
}

pub fn write_property_tag(
    bytes: &mut Vec<u8>,
    name_index: i32,
    type_param: &TypeParam,
    flags: u8,
    payload: &[u8],
) {
    push_i32(bytes, name_index);
    push_i32(bytes, 0);
    write_type_name(bytes, type_param);
    push_i32(bytes, i32::try_from(payload.len()).expect("fits in i32"));
    bytes.push(flags);
    bytes.extend_from_slice(payload);
}

pub fn write_property_terminator(bytes: &mut Vec<u8>, none_name_index: i32) {
    push_i32(bytes, none_name_index);
    push_i32(bytes, 0);
}

pub fn write_int_property_tag(bytes: &mut Vec<u8>, name_index: i32, type_index: i32, value: i32) {
    write_property_tag(
        bytes,
        name_index,
        &TypeParam {
            type_index,
            parameters: Vec::new(),
        },
        0,
        &value.to_le_bytes(),
    );
}

pub fn write_object_array_property_tag(
    bytes: &mut Vec<u8>,
    name_index: i32,
    array_type_index: i32,
    object_type_index: i32,
    indices: &[i32],
) {
    let mut payload = Vec::new();
    push_i32(
        &mut payload,
        i32::try_from(indices.len()).expect("fits in i32"),
    );
    for index in indices {
        push_i32(&mut payload, *index);
    }
    write_property_tag(
        bytes,
        name_index,
        &TypeParam {
            type_index: array_type_index,
            parameters: vec![TypeParam {
                type_index: object_type_index,
                parameters: Vec::new(),
            }],
        },
        0,
        &payload,
    );
}

pub fn write_object_property_tag(
    bytes: &mut Vec<u8>,
    name_index: i32,
    type_index: i32,
    index: i32,
) {
    write_property_tag(
        bytes,
        name_index,
        &TypeParam {
            type_index,
            parameters: Vec::new(),
        },
        0,
        &index.to_le_bytes(),
    );
}

/// Builds a synthetic `UDataTable` export serial blob: UObject root properties,
/// zero data marker, row count, then each row's name and tagged-property stream.
pub fn write_datatable_export(
    none_name_index: i32,
    root_properties: &[u8],
    rows: &[(i32, &[u8])],
) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.push(0); // class serialization-control extensions
    bytes.extend_from_slice(root_properties);
    write_property_terminator(&mut bytes, none_name_index);
    push_i32(&mut bytes, 0); // data marker
    push_i32(&mut bytes, i32::try_from(rows.len()).expect("fits in i32"));
    for (name_index, row_properties) in rows {
        push_i32(&mut bytes, *name_index);
        push_i32(&mut bytes, 0);
        bytes.extend_from_slice(row_properties);
        write_property_terminator(&mut bytes, none_name_index);
    }
    bytes
}

/// Builds a synthetic UObject export serial blob: extensions byte, tagged
/// properties, and terminator only (no DataTable row payload).
pub fn write_uobject_export(none_name_index: i32, properties: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.push(0); // class serialization-control extensions
    bytes.extend_from_slice(properties);
    write_property_terminator(&mut bytes, none_name_index);
    bytes
}
