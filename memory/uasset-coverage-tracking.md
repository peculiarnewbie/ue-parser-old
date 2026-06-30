---
name: uasset-coverage-tracking
description: Where per-class uasset decode coverage and the prioritized backlog are tracked
metadata:
  type: project
---

Per-class uasset decode coverage is tracked in the repo at `docs/asset-coverage.md`
(linked from README "Current status"). It is the canonical scoreboard + prioritized
backlog — update it whenever a class's decode status changes.

IMPORTANT: prioritize against the REAL project `Arif_MBPrototype/Content/Mana`,
NOT `Arif_UE-ManaBreak` (that's the engine tree — stock content, Arif said it's
"not really relevant"). Depth: "metadata + properties", explicitly NOT binary
mesh geometry / image pixels / audio / compressed anim tracks.

`examples/catalog.rs` (`cargo run --release --example catalog -- <root>`) now
measures DECODE coverage per class (decoded/failed/skipped + sample error), not
just package parse. Measured 2026-06-30 on MBPrototype (6625 files): 5834 parse
OK, 791 (~12%) fail at package layer; of parsed primaries 1120 (~18%) decode.

Project is ANIMATION/DATA-heavy (not texture/mesh like the engine). Priority order
revised: **0) raw-tail fallback (unblocks ~most fails), 1) AnimSequence (1957) +
AnimMontage (499)/BlendSpace, 2) Texture2D (1083), 3) StaticMesh (154),
4) SkeletalMesh (110)/Skeleton (25)**.

Good news already-working (~1120): generic UObjectDecoder fully decodes all
tail-less property objects — incl. most custom Mana gameplay configs (M_Line 124,
M_LipsyncAsset 112, M_ShopConfig 20, M_InputMappingContext 30, ...),
MaterialFunction 148, Blueprint 63, WidgetBlueprint 55, InputAction 83, plus
already-supported DataTable 183/185.

Failures split 3 ways: (1) "left N trailing bytes" = binary tail (dominant);
(2) "footer object-guid marker 1 got 0" = consume_uobject_export_footer heuristic
false-positives on 4/20-byte tail remainders (hits AnimMontage/BlendSpace/PoseAsset);
(3) genuine mid-stream property bugs (M_HitStrengthAnimationData soft-object SubPath
in a Map over-reads; texture InterchangeAssetImportData name-index) — see
[[softobject-property-misdecode]]. Raw-tail fallback must fix BOTH 1 and 2 (retain
unrecognized trailing as raw; only consume a footer that validates).

Key constraint: target is **uncooked editor packages**, so the heavy payload is
editor bulk data (`FTextureSource`, `FMeshDescription`, `FSkeletalMeshModel`)
serialized after the tagged-property stream. That tail currently trips
`consume_uobject_export_footer`'s trailing-bytes guard (`asset.rs:1214`), so every
tailed class FAILS today (not properties-only).

Enabling prerequisite for the whole backlog: make `UObjectDecoder` **retain the
unparsed tail as a raw Span instead of erroring** (mirror [[softobject-property-misdecode]]'s
RawReason approach). Cheapest highest-leverage step — flips tailed classes from
fail to properties-only and gives each decoder a known byte range to crack.

## DONE 2026-07-01 — raw-tail fallback + StaticMesh

Raw-tail fallback landed: `UObjectDecoder` retains trailing bytes as
`DecodedUObject.tail: Span`; `consume_uobject_export_footer_lenient` only consumes
a canonical zero/guid footer, else returns the rest as tail. CLI surfaces
`tail_bytes`; SCHEMA_VERSION bumped 3→4 (cli.rs + fixture_project.rs asserts
updated). Tests: `generic_uobject_retains_binary_tail_instead_of_failing`,
`generic_uobject_consumes_zero_footer_without_tail`.

Impact (MBPrototype): primaries decoded 1120 → **7695** of 7758 (~99%), failed
5866 → 63.

StaticMesh: DONE at the "metadata + properties" bar — no bespoke tail parser
needed. All useful metadata is in the property stream (StaticMaterials = slot
names+refs, SourceModels = LOD count via array len, ExtendedBounds, lightmap
settings); ~94-byte tail retained; heavy mesh data is in a separate bulk segment.

## DONE 2026-07-01 — import-data JSON prefix fix (StaticMesh complete)

Root cause of the `*ImportData` derail: `UAssetImportData::Serialize`
(`EditorFramework/AssetImportData.cpp`) writes a JSON `FString` BEFORE
`Super::Serialize` (the property stream) when `!IsFilterEditorOnly()`. So every
import-data sub-object's serial data is `[JSON FString][property stream][tail]`.
Fix: `decode_uobject_properties` consumes a leading FString for classes whose leaf
name ends in `ImportData` when editor data is present (FILTER_EDITOR_ONLY unset).
Helper `is_asset_import_data_class`. Test:
`asset_import_data_skips_leading_json_before_property_stream`.

Single root cause — unblocked meshes AND textures (InterchangeAssetImportData) AND
anims (Fbx*ImportData). Result: StaticMesh whole-file inspect 41% → **98%
(117/120)**; textures 20/20; the 3 SM stragglers are legacy pre-UE5.4 property
format (UnsupportedVersion — genuinely unsupported, not a bug).

## DONE 2026-07-01 — Skeleton (FReferenceSkeleton tail parse)

First class needing real tail parsing (bone hierarchy is NOT in properties).
`SkeletonDecoder` in `src/asset.rs`: decode property stream → consume object-guid
footer (inline, via `consume_inline_object_guid_footer`: i32 marker 0=none /
1=+FGuid) → `FReferenceSkeleton` = i32 bone count, then per bone (FName Name, i32
ParentIndex, editor-only FString ExportName, gated on !FILTER_EDITOR_ONLY). Pose
array + rest of tail left unparsed. New `DecodedAsset::Skeleton(DecodedSkeleton)` +
`SkeletonBone{name,parent_index}`; SKELETON_CLASS const. CLI: `kind:"Skeleton"`,
`bones` array; SCHEMA_VERSION 5→**6**. Verified order: USkeleton::Serialize does
Super::Serialize (props+footer) then `Ar << ReferenceSkeleton`. 25/25 project
skeletons OK, single root (root→pelvis→spine_01, 364 bones). Tests:
`skeleton_decoder_parses_reference_skeleton_bones`. NOTE: adding a DecodedAsset
variant requires updating exhaustive matches in tests/fixture_project.rs (6 sites)
and examples/dump_raw.rs — watch for this on the next variant.

SkeletalMesh (the mesh) still generic UObject+tail; its FReferenceSkeleton is
nested in FSkeletalMeshModel deeper in the tail — separate follow-up.

## DONE 2026-07-01 — Texture2D (enum ByteProperty fix)

Texture2D done at metadata bar, no bespoke decoder — all in property stream.
Root fix: enum-backed `ByteProperty` serializes as an 8-byte `FName`, not a u8;
`decode_typed_value` now reads it as `Enum(NameRef)` when payload != 1 byte, else
u8 (`codec.rs`). General win — fixes Format/CompressionSettings/LODGroup and enum
bytes across ALL assets. Dimensions/mips from the `Source` (FTextureSource) struct
which already decodes; struct/array output already recurses → surfaces in JSON.
Verified on real texture: 512x512, 1 mip, Source.Format=TSF_BGRA8,
CompressionSettings=TC_Masks, LODGroup=TEXTUREGROUP_Character. Tests:
`decodes_enum_backed_byte_property_as_name`, `decodes_plain_byte_property_as_uint`.
All 14 fixture tests still pass (no pinned enum cell regressed).

## DONE 2026-07-01 — per-export resilient inspect

`InspectOutput::from_package` is now infallible: it collects per-export decode
failures into `decode_errors: Vec<DecodeErrorOutput>` (object_path, class_path,
kind, message) instead of returning Err. When non-empty, status="partial" and the
CLI exits **6** (new EXIT_PARTIAL); fully-ok files stay status="ok" exit 0.
SCHEMA_VERSION 4→**5**. Removed now-dead `exit_code_for_asset_error` and
`ErrorOutput::asset`; added shared `asset_error_kind_name`. Text output appends
`decode_error:` lines. Tests: `partial_decode_reports_errors_and_exit_six`
(cli.rs, gated on UASSET_PARTIAL_SAMPLE); cli/fixture schema asserts → 5.

Verified: legacy SM_Plane_01 (pre-UE5.4 property format) → status partial, 6
decode_errors, exit 6; good SM → exit 0.

STILL OPEN: ~63 remaining primary failures are other bucket-3 per-class property
misparses (now surfaced as decode_errors, not aborts). Next priorities per
[[uasset-coverage-tracking]] backlog: AnimSequence/Montage (largest), Texture2D
curated metadata, SkeletalMesh.

See [[ue-source-reference]] for engine file pointers.
