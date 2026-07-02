#![no_main]

use libfuzzer_sys::fuzz_target;
use uasset_parser::Reader;
use uasset_parser::property::read_tagged_property_stream;
use uasset_parser::version::{PackageFlags, VersionContext};

fuzz_target!(|data: &[u8]| {
    let names = [
        "None",
        "BoolProperty",
        "ByteProperty",
        "EnumProperty",
        "FloatProperty",
        "DoubleProperty",
        "IntProperty",
        "Int64Property",
        "UInt64Property",
        "NameProperty",
        "StrProperty",
        "TextProperty",
        "StructProperty",
        "Vector",
        "ObjectProperty",
        "SoftObjectProperty",
        "ArrayProperty",
        "SetProperty",
        "MapProperty",
        "Guid",
        "TestName",
        "TestEnum::Value",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect::<Vec<_>>();
    let versions = VersionContext {
        legacy_file_version: -9,
        legacy_ue3: None,
        ue4: VersionContext::LATEST_SUPPORTED_UE4,
        ue5: VersionContext::LATEST_SUPPORTED_UE5,
        licensee: 0,
        package_flags: PackageFlags::from_bits(0),
    };
    let mut reader = Reader::new(data);

    let _ = read_tagged_property_stream(&mut reader, &versions, &names, "Fuzz");
});
