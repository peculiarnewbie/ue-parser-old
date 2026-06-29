---
name: swag-datatable-plugin-roadmap
description: Recommended improvements for SWAG_RemoteControlDataTableLibrary
metadata:
  type: project
---

# SWAG DataTable plugin — improvement roadmap

SWAG’s role: **expose editor-grade DataTable APIs to Blueprint, Python, and
Remote Control**, plus a stable **authoring JSON** dialect for electroswag — not
reimplement Unreal’s DataTable or package serialization.

**Principle:** wrap native, don’t reimplement native.

---

## What SWAG already does well

- `BeginSwagBatch` / `EndSwagBatch` — batched undo (no native equivalent)
- `SetDataTableCell` — partial column patch (no native equivalent)
- `ExportDataTableToAuthoringJSON` — electroswag wire shape (≠ engine export)
- `GetDataTableSchema`, composite introspection, `GetActorsReferencingRow`
- `SaveDataTableAssets` — RC-friendly save by object path

Native engine APIs (`UDataTableFunctionLibrary`, `EditorAssetLibrary`) already
cover whole-table import/export, add/remove row, and save. SWAG should delegate
to them where possible.

---

## P0 — Fix the property write path

**Problem:** `SetDataTableRow` / `SetDataTableCell` use `FJsonObjectConverter`
(`JsonObjectToUStruct` / `JsonValueToUProperty`). That path is weak for
`TSoftObjectPtr`, some enums, and nested types — and can report success without
a faithful in-memory value.

**Improve:**

1. **Single helper:** `SetPropertyFromAuthoringJson(FProperty*, void*, FJsonValue)`
   - Scalars / string / name / enum — JSON converter or `ImportText`
   - **Soft object** — `ImportText_Direct` with plain path string (electroswag shape)
   - **Struct** — recurse nested JSON
   - **Text** — display string (extend later for keyed text if needed)

2. **Use the same helper in `SetDataTableCell` and `SetDataTableRow`** (no duplicated soft-ref patches).

3. **`SetDataTableRow`** — after building scratch row, call `DataTable->AddRow`
   (same as `UDataTableFunctionLibrary::AddDataTableRow`).

Use editor property import (`ImportText`), not a parallel type system.

---

## P0 — Symmetric read APIs

| API | Purpose |
|-----|---------|
| `GetDataTableCellAsAuthoringJson` | RC assert one cell |
| `GetDataTableRowAsAuthoringJson` | Per-row round-trip |
| Optional `bVerifyAfterSet` on setters | Fail if read-back ≠ input |

Implement via `ExportText_Direct` / `UStructToJsonObject` — native, SWAG-shaped output.

---

## P1 — Thin passthroughs for whole-table native ops

Wrap engine APIs with SWAG names + RC param names:

- `FillDataTableFromAuthoringJSON` → `FillDataTableFromJSONString` (replaces entire table)
- `ExportDataTableToEngineJSON` → `ExportDataTableToJSONString` (diff vs reimport)
- `AddDataTableRow` / `RemoveDataTableRow` → `UDataTableFunctionLibrary`

Add `meta=(ScriptMethod="...")` on wrappers so Python discovers them under
`unreal.SWAG_RemoteControlDataTableLibrary` (engine `AddDataTableRow` is
CustomThunk-only and failed from Python in fixture bootstrap).

---

## P1 — Save: stay thin, get honest

- One save implementation (`EditorAssetLibrary` or `UPackage::SavePackage`).
- Use existing `ErrorsByTable` consistently.
- Optional `bVerifySaved`: reload from disk or probe critical cells after save.

Save may not fix soft-ref persistence by itself (see
[[softobject-property-misdecode]]), but “save succeeded” should mean reload
verifies for authoring workflows.

---

## P1 — Schema ↔ authoring JSON contract

Align `GetDataTableSchema` with setter acceptance rules:

- Soft object = **string path** (not `{AssetPath: ...}`)
- Enum = string literal
- Struct = nested object

Extend schema with `authoringExample` per column, or document via
`GetDataTableRowDefaults` as source of truth for empty-row JSON shape.

---

## P2 — Defer / avoid

| Skip | Reason |
|------|--------|
| Package soft-path serialization | Engine save pipeline |
| Replace batched transactions | Real SWAG value |
| Nested path edits (`Stats.HP`) | Use row JSON or struct cell |
| `AddFixtureInts` in shipping plugin | Test-only module |

**Composite (optional):** `GetComposedRowNames`, `SetCompositeParentTables` if
contract.ts needs composite authoring.

---

## Suggested PR order

1. Property writer refactor (`SetPropertyFromAuthoringJson`)
2. Read symmetry (`GetDataTableCellAsAuthoringJson`, `GetDataTableRowAsAuthoringJson`)
3. Native passthroughs + `ScriptMethod` for Python
4. Optional post-save verify on `SaveDataTableAssets`
5. Schema examples aligned with `electroswag/e2e/fixtures/unreal/contract.ts`

---

## Fixture lesson (DT_AssetRefs)

- SWAG JSON + export looked correct in memory.
- Committed `.uasset` had empty soft-ref index (save/collection issue).
- Native `AddRow` + save showed same disk behavior — not only JsonObjectConverter.
- Parser work tracked in [[softobject-property-misdecode]].
