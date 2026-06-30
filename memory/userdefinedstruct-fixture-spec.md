---
name: userdefinedstruct-fixture-spec
description: v17 UUserDefinedStruct decoder is blocked on authoring a ground-truth fixture; this is the field spec
metadata:
  type: project
---

`UUserDefinedStruct` decoding (v17, the asset after [[userdefinedenum-decoder]])
is **blocked on a ground-truth fixture**. Decision (2026-06-30): author the
fixture FIRST, then build the parser validated byte-exact against real bytes —
because the format is large (a full `FField`/`FProperty` definition parser) and
synthetic-only tests are self-referential (they'd encode my reading of the UE
source and pass even with a wrong field width).

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
