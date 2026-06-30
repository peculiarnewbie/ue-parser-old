---
name: softobject-property-misdecode
description: SoftObjectProperty decoding — inline FString vs package soft-object-path index table
metadata:
  type: project
---

## Wire format

`SoftObjectProperty` / `TSoftObjectPtr` payloads are **not** always an inline `FString`.

When the package summary includes a **soft object path table** (UE5+), each
property stores a **4-byte `i32` index** into that table. Table entries serialize
via `FSoftObjectPath::SerializePathWithoutFixup` (asset path `FString` + subpath
`FString`).

When the table is absent or the payload is not exactly 4 bytes, decode as inline
`FString` (+ optional subpath). Empty unset ref = 4 zero bytes (index 0 or empty
inline string).

## Parser status

| Piece | Status |
|-------|--------|
| Inline `FString` (+ subpath) decode | Done — `decode_soft_object_path` in `src/codec.rs` |
| Index resolve when table populated | Done — uses `Package::soft_object_paths` when payload is 4 bytes |
| **`Summary.SoftObjectPaths` table parse** | Done (fixed 2026-06-30) — each entry is `FTopLevelAssetPath` = PackageName `FName` + AssetName `FName` (each an i32 name-map index + i32 number) followed by the subpath `FString`. Formatted `PackageName.AssetName[:SubPath]`; an unset/`None` package = empty. The earlier reader treated the entry as an inline `FString` and only "worked" because every prior fixture had empty entries; `S_E2EFixture` (3 real entries) exposed it. Parsed in `read_soft_object_path_list` (`src/package.rs`) with name resolution. |
| `WeakObjectProperty` / `ObjectProperty` weak refs | Verified — saved weak fixture resolves through object archive serialization as an `FPackageIndex`; parser decodes it as `ObjectRef`. |
| `LazyObjectProperty` | Verified — persistent lazy refs serialize as a 16-byte `FGuid`; parser decodes them as `Guid`. Self-referencing lazy fixtures also produce a populated UObject export GUID footer. |
| Unit tests | `decodes_populated_soft_object_path_payload`, `decodes_indexed_soft_object_path_payload`, `decodes_soft_object_path_with_subpath`, `decodes_weak_object_payload_as_package_index`, `decodes_lazy_object_payload_as_guid` |

Until the table parser exists, indexed refs on real assets with populated paths
will not resolve. Empty refs (4 zero bytes) still decode correctly via inline path.

Some editor packages store a placeholder `SoftObjectPathsCount` whose bytes overlap
the gatherable-text section; invalid table literals are sanitized to empty during
parse so indexed unset refs still decode as empty.

## `DT_AssetRefs` — resolved (was a parser bug, not a save bug)

`DT_AssetRefs.Texture` is `TSoftObjectPtr<UTexture2D>`. It was assumed to persist
**empty** on disk; in fact it always held
`/Engine/EngineResources/DefaultTexture.DefaultTexture` — the soft-path table
mis-parse (above) just couldn't read it. After the 2026-06-30 fix the cell
resolves correctly. The parser mirror `tests/fixtures/electroswag-v15.json` and
`parses_soft_object_path_list_from_fixture_when_available` were updated to the
real path. **Upstream divergence:** electroswag `contract.ts` still pins
`Texture` as empty (contract v8) — it should be corrected to the DefaultTexture
path; see [[fixture-ground-truth-contract]].

See also [[swag-datatable-plugin-roadmap]] for plugin-side improvements.

## Still open

- (none for the header table parse)
