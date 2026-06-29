---
name: fixture-ground-truth-contract
description: Where the authored expected values for the E2E DataTable fixtures live
metadata:
  type: reference
---

The ground-truth expected values for the shared Electroswag E2E DataTable fixtures
(`D:\Perforce\Arif_Fixtures\Content\E2EFixture\Data\DT_*.uasset`) live in the sibling
repo at `C:\Users\Ryzen\git\swag\electroswag\e2e\fixtures\unreal\contract.ts`
(`DEFAULT_CONTRACT`). The C++ row structs are in
`D:\Perforce\Arif_Fixtures\Source\E2EFixtures\E2EFixtureTypes.h`.

The parser's own `tests/fixtures/electroswag-v11.json` is the mirror (gitignored;
generate from contract.ts when the upstream contract changes, or extend via
`scripts/generate_electroswag_v12_json.py` for parser-only pins (v10: `DA_MapSet`;
v11: `DT_MapSet`, `DA_Collections`, `DA_Localized`; v12: `UO_Plain` plain UObject).
Its `datatables` section pins each DataTable's object path, ordered row names, columns, and typed cell
values, asserted by `shared_fixture_datatables_match_contract_mirror`.

Contract v8 adds: `emptyTable`, `wideScalarsTable`, `compositeEmptyTable`,
`compositeNestedTable`, populated `DT_Localized` (`Loc_Row1`), and composite row pins
(`CDT_E2EFixture.Row_Alpha.IntValue` = 4243 from parent `DT_Scalars`). `DT_AssetRefs.Texture`
is pinned empty until SWAG/UE save persists soft refs on disk (see
[[softobject-property-misdecode]]).

**How to apply:** When validating decoded DataTable values, assert against contract.ts
(via the electroswag-v8.json mirror), not guesses. For wire-format contracts
(package layout, property tags, primitive encoding), see [[ue-source-reference]].
