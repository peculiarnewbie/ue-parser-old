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
| **`Summary.SoftObjectPaths` table parse** | Done — binary structured-archive streams are no-ops; table is consecutive `FSoftObjectPath` wire entries at `Summary.SoftObjectPathsOffset`. |
| `WeakObjectProperty` / `ObjectProperty` weak refs | Verified — saved weak fixture resolves through object archive serialization as an `FPackageIndex`; parser decodes it as `ObjectRef`. |
| `LazyObjectProperty` | Verified — persistent lazy refs serialize as a 16-byte `FGuid`; parser decodes them as `Guid`. Self-referencing lazy fixtures also produce a populated UObject export GUID footer. |
| Unit tests | `decodes_populated_soft_object_path_payload`, `decodes_indexed_soft_object_path_payload`, `decodes_soft_object_path_with_subpath`, `decodes_weak_object_payload_as_package_index`, `decodes_lazy_object_payload_as_guid` |

Until the table parser exists, indexed refs on real assets with populated paths
will not resolve. Empty refs (4 zero bytes) still decode correctly via inline path.

Some editor packages store a placeholder `SoftObjectPathsCount` whose bytes overlap
the gatherable-text section; invalid table literals are sanitized to empty during
parse so indexed unset refs still decode as empty.

## Fixture gap: `DT_AssetRefs`

`DT_AssetRefs.Texture` is `TSoftObjectPtr<UTexture2D>`. SWAG/native row set +
save currently persist an **empty** soft-path table entry on disk. Contract v8
pins `Texture` as empty. Target when save works:
`/Engine/EngineResources/DefaultTexture.DefaultTexture`.

See also [[swag-datatable-plugin-roadmap]] for plugin-side improvements.

## Still open

- Structured-archive parser for `Summary.SoftObjectPaths`
