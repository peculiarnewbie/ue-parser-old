---
name: softobject-property-misdecode
description: codec.rs decodes Soft/Weak/LazyObjectProperty as a 4-byte FPackageIndex, which is wrong
metadata:
  type: project
---

In `src/codec.rs`, `decode_property_record` groups `SoftObjectProperty`,
`WeakObjectProperty`, and `LazyObjectProperty` with `ObjectProperty`/`ClassProperty`
and decodes all of them as a 4-byte `FPackageIndex` (`read_i32` → `PackageIndex`).

**Why:** Only hard `ObjectProperty`/`ClassProperty` serialize as `FPackageIndex` in
uncooked editor packages. A `SoftObjectProperty` is an `FSoftObjectPath` (asset path
name + subpath string); weak/lazy refs have their own formats. Discovered via the
`DT_AssetRefs` fixture, whose `Texture (SoftObjectProperty)` is empty (4 zero bytes),
so reading it as an index gives a coincidentally-harmless `null`. A populated soft
ref would misparse or be flagged raw.

**How to apply:** Don't trust soft/weak/lazy ref output until `FSoftObjectPath` is
implemented properly. The conservative interim fix is to drop soft/weak/lazy out of
the FPackageIndex match arm so they fall through to `Raw` instead of emitting a
misleading decoded value.
