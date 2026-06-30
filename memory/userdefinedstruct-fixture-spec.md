---
name: userdefinedstruct-fixture-spec
description: v17 UUserDefinedStruct decoder — DONE, validated against the S_E2EFixture ground-truth asset
metadata:
  type: project
---

`UUserDefinedStruct` decoding (v17, the asset after [[userdefinedenum-decoder]])
is **DONE** — `StructDecoder` in `src/asset.rs`, validated byte-exact against the
real `S_E2EFixture.uasset` (in `Content/E2EFixture/Data/`, authored 2026-06-30).
Ground-truth test: `shared_fixture_user_defined_struct_decodes` in
`tests/fixture_project.rs`; synthetic unit tests `decodes_user_defined_struct_fields`
and `rejects_unsupported_struct_field_type` in `src/asset.rs`.

**Validated wire-format facts (these differ from the UE-source research summary —
the real asset caught the errors):**
- `FProperty::RepIndex` is a **`uint16`** (2 bytes), NOT `int32`. This was the
  one width that broke parsing until corrected against real bytes.
- `FBoolProperty` tail is **6 × `u8`** (FieldSize, ByteOffset, ByteMask,
  FieldMask, BoolSize, NativeBool) — `u8`, not `int32`.
- `FField::Serialize` metadata: `bHasMetaData` is an **archive bool = `u32`**
  (4 bytes), then `TMap<FName,FString>` (i32 count + key FName + value FString).
  Field friendly names live under metadata key `DisplayName`; default values
  under `MakeStructureDefaultValue`.
- There **is** a zero-`i32` UObject footer between the struct object's tagged
  stream and the `UStruct` tail (same footer the enum/datatable decoders read).
- Field NAMES on disk are GUID-mangled (`IntValue_2_<hex>`); the friendly names
  also appear in the sibling `UserDefinedStructEditorData` export's
  `VariablesDescriptions` array.

Original decision (2026-06-30): author the fixture FIRST, then build validated —
because synthetic-only tests are self-referential. That paid off: the RepIndex
width error would have passed a synthetic-only suite.

**Why no fixture exists:** the E2E corpus (`D:\Perforce\Arif_Fixtures`, the only
source in our supported UE 5.7.2 uncooked "complete type names" format) has no
UDS — `DT_Structs`/`DA_Structs` use the *native* C++ struct
`/Script/E2EFixtures.E2EFixtureStructsRow`. Engine-shipped UDS assets (e.g.
`Engine\Plugins\Experimental\Landmass\Content\Landscape\BP\Structs\*.uasset`)
are an OLDER property-tag format and the parser rejects them
(`property tags before complete type names are not supported`).

**Authoring spec** — create one UserDefinedStruct (suggested `S_E2EFixture`,
saved under `Content/E2EFixture/Data/`) with fields covering every `FField`
serialize branch, each given a non-default value so the default-instance blob is
exercised:
- `IntValue` Integer, `FloatValue` Float (plain `FProperty`, no extra bytes)
- `StringValue` String, `NameValue` Name, `TextValue` Text (plain)
- `BoolValue` Boolean (exercises `FBoolProperty`: FieldSize/ByteOffset/ByteMask/
  FieldMask/BoolSize/NativeBool — the widths the source research was unsure about)
- `EnumValue` (Enum field → `FEnumProperty`: Enum ref + UnderlyingProp via
  `SerializeSingleField`)
- `StructValue` (e.g. Vector → `FStructProperty`: Struct ref FPackageIndex)
- `ObjectValue` (e.g. Texture2D ref → `FObjectPropertyBase`: PropertyClass ref)
- `SoftObjectValue` (Soft object ref)
- `IntArray` Array of Integer (`FArrayProperty`: Inner via `SerializeSingleField`)
- `NameToIntMap` Map Name→Integer (`FMapProperty`: Key + Value)
- `IntSet` Set of Integer (`FSetProperty`: Element)

Then commit the `.uasset`, and (if pinning values) add it to electroswag
`contract.ts` per [[fixture-ground-truth-contract]]. Note UDS field names are
GUID-mangled on disk (e.g. `IntValue_2_<hex>`); friendly names live in field
MetaData / EditorData.

**Wire format already researched** (ready to implement once the asset lands):
after the tagged-property stream + zero `i32` footer — `SuperStruct` (i32) →
`Children` `TArray<UField*>` (i32 count + indices) → `ChildProperties` via
`SerializeProperties` (i32 count, then per field: `FName` type +
`FField::Serialize`) → script markers `i32 ScriptBytecodeSize` + `i32
ScriptStorageSize` (expect 0/0) → `StructFlags` u32 → default-instance tagged
stream. `FField::Serialize` = `FName` name + `u32` flags + archive-bool (u32)
`bHasMetaData` + optional `TMap<FName,FString>`. `FProperty::Serialize` =
`i32 ArrayDim` + `i32 ElementSize` + `u64 PropertyFlags` + `i32 RepIndex` +
`FName RepNotifyFunc` + `u8 BlueprintReplicationCondition`. All object refs are
`FPackageIndex` (i32). Verify `FBoolProperty` field widths against the real
asset (uint8 vs the int32 the research summary guessed).
