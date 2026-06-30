---
name: userdefinedenum-decoder
description: UUserDefinedEnum decoder status, wire format, and the fixture-corpus gap
metadata:
  type: project
---

`EnumDecoder` in `src/asset.rs` decodes `/Script/Engine.UserDefinedEnum` exports.

**Wire format (UE 5.7):** after the tagged-property stream and the zero `i32`
UObject footer, `UEnum::Serialize` writes `int32 Num`, then `Num` ×
(`FName` qualified `Enum::Entry` + `int64` value), then `uint8 CppForm`
(0 Regular / 1 Namespaced / 2 EnumClass). `UUserDefinedEnum::Serialize` adds
nothing on disk — `DisplayNameMap` (`TMap<FName, FText>`) rides in the
tagged-property stream, so `DecodedEnum.entries[*].display_name` is resolved
from that map (the auto-appended `_MAX` sentinel has no display name → `None`).

**Fixture gap:** there is NO standalone `UUserDefinedEnum` asset in
`D:\Perforce\Arif_Fixtures`. `DT_Enums`/`DA_Enums` reference a native C++ enum
(`/Script/E2EFixtures.EE2EFixtureEnum`), decoded as the qualified `FName` cell
value, not a Blueprint enum asset. So enum decoding is validated by parser unit
tests against synthetic bytes only — the same situation as populated string
tables (see [[fixture-ground-truth-contract]]). To pin a real enum on disk,
author an `E_*.uasset` UserDefinedEnum in the fixture project (needs the editor).

CLI `schema_version` bumped 2 → 3 for the new `Enum` asset output (`kind: "Enum"`,
`cpp_form`, `enum_entries`). Next asset planned: `UUserDefinedStruct` (v17).

**Why:** the chosen roadmap was "enum then struct" for closing the data-driven
asset family; enums make `EnumProperty`/`ByteProperty` cells resolvable.
**How to apply:** when adding the struct decoder, mirror this decoder's shape and
the `display_name_map` helper pattern; add the new `DecodedAsset` variant arms to
`src/bin/uasset.rs`, `examples/dump_raw.rs`, and `tests/fixture_project.rs`.
