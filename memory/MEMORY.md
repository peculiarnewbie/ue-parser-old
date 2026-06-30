# Memory index

- [UE source reference](ue-source-reference.md) — local engine tree at `C:\Users\Ryzen\Perforce\Arif_UE-ManaBreak` for serialization contracts
- [SoftObjectProperty decoding](softobject-property-misdecode.md) — inline vs indexed soft refs; package soft-path table parsing
- [Fixture ground-truth contract](fixture-ground-truth-contract.md) — authored expected values live in electroswag `contract.ts`; parser mirror at `tests/fixtures/electroswag-v15.json`
- [SWAG DataTable plugin roadmap](swag-datatable-plugin-roadmap.md) — recommended SWAG_RemoteControlDataTable improvements
- String tables — `StringTableDecoder` in `src/asset.rs`; `ST_Simple` is an empty real asset because UE Python exposes only read-only string-table APIs
- [UserDefinedEnum decoder](userdefinedenum-decoder.md) — enum tail wire format + display-name resolution; no standalone enum asset in the fixture corpus (unit-test-only); struct decoder is next
- [UserDefinedStruct decoder](userdefinedstruct-fixture-spec.md) — v17 UDS decoder DONE, validated against S_E2EFixture; FField/FProperty wire format (RepIndex u16, bool u8 widths caught by ground truth)
