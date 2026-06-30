# UAsset CLI

Fast, read-only command-line inspector for classic Unreal Engine asset packages.
The Rust library is an implementation detail; the versioned CLI output is the
integration contract for other programs.

The initial compatibility target is UE 5.7.2, uncooked editor packages using
versioned tagged properties.

## Current status

Phases 1 through 6 are implemented:

- Downward-only module skeleton
- Bounded little-endian binary reader
- Checked cursor operations and child readers
- Configurable count and allocation limits
- Offset- and field-aware errors
- `FGuid`, `FIoHash`, classic `FName`, `FString`, and generic `TArray` reads
- Reusable source `Span`
- Version-gated classic `FPackageFileSummary` parsing
- Custom versions for enum, GUID, and optimized legacy layouts
- Explicit rejection of cooked, unversioned-property, future-version, and
  swapped-endian packages
- Validated header/table locations
- Name map parsing with legacy hash handling
- Import/export map parsing for supported classic UE4/UE5 layouts
- `FPackageIndex` object-path resolution across imports and exports
- Bounded export readers validated against `SerialOffset + SerialSize`
- Fixture-backed `UDataTable` export discovery by `/Script/Engine.DataTable`
- Modern UE5 tagged-property envelope parsing with complete type names
- Root UObject serialization-control extension handling
- Raw payload spans retained for unsupported property values
- Fixture-backed `UDataTable` property stream parsing through `NAME_None`
- Minimum property value decoding for bool, integers, floats, names, strings,
  and object/class references
- Fixture-backed `RowStruct` object-reference decoding and path resolution
- `UDataTable` asset adapter with row-count and row-name decoding
- Row tagged-property streams are parsed and decoded where the supported
  property subset applies
- Per-row property names, types, and decoded values are surfaced in both the
  text and JSON `inspect` output; unsupported property types are reported as
  raw with their type and payload byte length
- `UUserDefinedEnum` decoding: the `UEnum` name/value pairs and `CppForm`
  from the export tail, with per-entry display names resolved from the
  `DisplayNameMap` carried in the tagged-property stream
- `uasset inspect` command with text and schema-versioned JSON output
  (current `schema_version` is 3)
- File and stdin input
- Stable stdout/stderr and exit-code behavior

## CLI

```text
uasset inspect Asset.uasset
uasset inspect Asset.uasset --format json
uasset inspect - --format json
```

Successful output is written only to stdout. Errors and diagnostics are written
only to stderr.

Exit codes:

- `0`: success
- `2`: malformed package data
- `3`: unsupported format, version, or capability
- `4`: input/output failure
- `5`: internal output failure
- `64`: invalid command-line usage

Run checks with:

```text
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
```

## Unreal Engine source

Serialization contracts (package summary layout, version gates, property tags,
primitive wire formats) can be verified against the local UE 5.7 tree:

```text
C:\Users\Ryzen\Perforce\Arif_UE-ManaBreak
```

Start with `Engine/Source/Runtime/CoreUObject/Public/UObject/PackageFileSummary.h`,
`Engine/Source/Runtime/Core/Public/UObject/ObjectVersion.h`, and
`Engine/Source/Runtime/CoreUObject/Private/UObject/PropertyTag.cpp`. A fuller
file-to-module map lives in `memory/ue-source-reference.md`.

## Shared Unreal fixture project

The parser reuses Electroswag's UE 5.7 fixture project rather than maintaining
a second Unreal project. The parser-owned mirror of the current fixture contract is:

```text
tests/fixtures/electroswag-v15.json
```

Its upstream source of truth is Electroswag's
`e2e/fixtures/unreal/contract.ts`; keep the two in sync when the contract
changes. The mirror's `datatables` section pins each DataTable's object path,
ordered row names, expected columns, and typed cell values, which the fixture
tests assert against decoded output (e.g. `DT_Scalars2` row `Row_Beta` decodes
to `IntValue = 222`, `StringValue = "FromScalars2"`, the composite override
that `contract.ts` pins).

Fixture tests validate both the Rust parser and the spawned CLI against every
contract asset. Resolution order:

1. `UASSET_FIXTURE_DIR`
2. Studio default `D:\Perforce\Arif_Fixtures`

When no fixture exists, these tests skip so normal builds remain portable. To
make absence a test failure in fixture-backed CI:

```text
UASSET_REQUIRE_FIXTURE=1 cargo test --test fixture_project
```
