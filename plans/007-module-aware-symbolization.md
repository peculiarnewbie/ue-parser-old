# Plan 007: Add module-aware callstack symbolization as an optional layer

> **Executor instructions**: This is a design/spike plan followed by a Windows
> implementation only if the spike gates pass. Do not invent a cross-platform
> symbol engine. Update `plans/README.md` when done.
>
> **Drift check (run first)**: `git diff --stat 82a0968..HEAD -- src/utrace.rs src/utrace_callstacks.rs src/utrace_symbols.rs src/bin/uasset.rs tests/utrace_fixture.rs Cargo.toml`

## Status

- **Priority**: P0
- **Effort**: L/XL (platform and toolchain dependent)
- **Risk**: HIGH (wrong-build symbols are worse than unresolved addresses)
- **Depends on**: plan 006 (DONE)
- **Category**: direction
- **Planned at**: commit `82a0968`, 2026-07-12
- **Status**: DONE (2026-07-12)
- **Delivered**:
  - `src/utrace_modules.rs` — ModuleInit/Load/Unload, base-shift reconstruct, map → mapped/unmapped/ambiguous, ImageId → GUID+age
  - `src/utrace_symbols.rs` — `SymbolResolver` seam, bounded `SymbolCache`, `map_frame`, CLI enrichment; `PdbSymbolResolver` behind `utrace-symbols`
  - Dashboard `modules` + callstack `mapped_frames` (raw hex `frames` preserved)
  - CLI repeatable `--symbol-path` (errors without `utrace-symbols`)
  - Tiny probe PDB unit tests (build `tests/fixtures/symbols/probe.rs` at test time)
  - Ignored fixture: studio `targeted-providers.utrace` fallback; ≥3 module-mapped frames; optional `UTRACE_SYMBOL_PATH` symbolization

## Spike decision (2026-07-12) — PROCEED

### Fixture facts (`targeted-providers.utrace`)

| Event | Observed | Declared fields | Sample notes |
|-------|----------|-----------------|--------------|
| `Diagnostics.ModuleInit` | 1 | `SymbolFormat: ansi_string`, `ModuleBaseShift: uint8` | `SymbolFormat="pdb"`, `ModuleBaseShift=0` |
| `Diagnostics.ModuleLoad` | 601 | `Name: wide_string`, `Base: uint64`, `Size: uint32`, `ImageId: array` | e.g. `UnrealEditor-Cmd.exe`, Size=524288, ImageId 20 bytes |
| `Diagnostics.ModuleUnload` | 7 | `Base: uint64` | present |
| `Memory.CallstackSpec` | 963344 | (plan 006) | retained catalog available |

ImageId sample hex: `7fce1bf5888c1e43ada14a8298916b8c01000000` = Microsoft in-memory GUID (16) + Age LE u32 (=1).

### Insights parity (`ModuleAnalysis.cpp`)

- Base reconstruction: if `ModuleBaseShift == 0`, use `Base` as `uint64`; else treat `Base` as `uint32` and compute `Base << ModuleBaseShift`.
- Module provider is only created after `ModuleInit` supplies `SymbolFormat`.
- Windows emitter (`WindowsModuleDiagnostics.cpp`) copies CodeView RSDS Guid+Age into ImageId (20 bytes).

### Backend choice

| Option | License | Offline | Identity check | Line info | Verdict |
|--------|---------|---------|----------------|-----------|---------|
| `pdb-addr2line` 0.12 + `pdb` 0.8 | Apache-2.0 / MIT-Apache | yes | GUID+age via `PDBInformation` (convert MS GUID byte order) | yes + inlines | **Selected** |
| `symbolic` (Sentry) | MIT | yes | yes | yes | heavier; defer |
| DIA/dbghelp FFI | MS | yes | yes | yes | avoid unsafe + platform glue for v1 |

**Proven**: local `UnrealEditor-Cmd.pdb` age=1 and GUID `f51bce7f-8c88-431e-ada1-4a8298916b8c` match the fixture ImageId after MS→UUID field conversion. Wrong/mutated PDB can be rejected.

**Gate result**: proceed with module catalog + optional Windows/PDB resolver. Non-`pdb` `SymbolFormat` values stay module+offset only (no fake decoding).

### Architecture

- Library (`utrace`): decode modules, map PC → `{module, relative_address}`; filesystem-free.
- Optional feature `utrace-symbols`: `pdb-addr2line` backend.
- CLI: repeatable `--symbol-path`; never network-search by default.

## Why this matters

Raw stacks identify repeated code addresses but not responsible functions.
Trustworthy symbols require module address ranges and build identities before a
resolver can consult PDB/DWARF data. This plan makes symbolization optional and
honest: unresolved raw addresses always remain valid output, and symbol files
are accepted only when they match the traced module identity.

## Engine contracts to verify

- `Runtime/Core/Private/ProfilingDebugging/ModuleDiagnostics.h` declares
  `Diagnostics.ModuleInit` (symbol format/base shift) and `ModuleLoad`
  (name, base, size, image/build id), with platform-specific emitters.
- `Developer/TraceServices/Private/Analyzers/ModuleAnalysis.cpp` is the parity
  reference for address-to-module mapping and build-id handling.
- UE platforms advertise formats such as `pdb`, `dwarf`, and `psym`; this plan's
  first implementation target is the studio's Windows/PDB workflow only.

## Scope

**In scope**: module catalog decoding, address-to-module mapping, a resolver
trait, one verified Windows resolver backend, CLI symbol-path options, bounded
symbol cache, resolution diagnostics, tests, and coverage docs.

**Out of scope**: downloading from arbitrary public symbol servers by default,
embedding UE code, implementing PDB/DWARF parsers from scratch, macOS/iOS/Linux
backends, and deleting raw addresses after resolution.

## Steps

### Step 1: Spike real capture and resolver choices

Inventory a Windows callstack fixture and record exact registry fields for
`Diagnostics.ModuleInit`, `ModuleLoad`, and unload events if present. Compare
one address/module/function against Unreal Insights. Evaluate mature Rust crates
and external tools available in the build environment for PDB lookup; record
license, maintenance, line-info support, demangling, and build-id validation.

Write the decision and test fixture facts into this plan before implementation.
Proceed only if one backend can be tested offline and can reject mismatched
symbols. Otherwise stop with a smaller follow-up proposal for module+offset
output (`Game.exe+0x1234`), which is still useful.

### Step 2: Decode the module catalog

Add a small module provider (separate file if it would expand
`utrace_callstacks.rs` materially). Use checked base+size arithmetic and preserve
module load order/lifetime data when events provide timestamps. Map each raw
frame to `{ module, relative_address }`; explicitly report unmapped and
ambiguous ranges. Apply `ModuleBaseShift` exactly as the engine analyzer does.

**Verify**: unit tests cover boundaries, overlapping ranges, overflow,
load/unload/reload at the same base, base shift, and missing `ModuleInit`.

### Step 3: Define an optional resolver seam

Create a resolver interface taking verified module identity plus relative
address and returning a tagged result: resolved symbol/source, module-only,
symbols-missing, identity-mismatch, or resolver-error. Add a bounded cache keyed
by module identity and relative address. Resolver failure must never make trace
parsing fail or erase the raw address.

Add explicit CLI inputs such as repeatable `--symbol-path`; do not silently
search the network. Keep the library parser deterministic and filesystem-free;
perform symbol discovery at the CLI/application seam.

### Step 4: Implement and validate the Windows backend

Implement only the backend selected in step 1. Validate PDB identity before
using names, normalize/demangle consistently, and emit optional file/line data.
Add dashboard resolution totals and bounded representative resolved stacks;
never eagerly duplicate resolved strings across every allocation.

**Verify**: a tiny checked test binary/PDB resolves known functions, a mutated
or wrong PDB is rejected, missing PDB produces module+offset output, and the
real fixture matches the same frames in Insights.

### Step 5: Integrate consumers and document limitations

Allow memory and bookmark views to look up resolved stacks by id. Document the
supported platform/toolchain, symbol search order, privacy implications of
source paths, cache bounds, and the fact that optimized/inlined frames may not
match source expectations exactly.

#### Limitations (shipped)

- **Platform**: Windows/PDB only (`SymbolFormat::Pdb`). `dwarf` / `psym` /
  `other` stay module+offset; no fake decoding.
- **Search order** (`--symbol-path`, repeatable): for each root, try
  `{stem}.pdb` then `{module basename with .pdb}`. No recursive walk, no
  network / symbol-server fetch.
- **Identity**: PDB GUID+age must match traced ImageId (MS→UUID conversion).
  Mismatch → `identity_mismatch`; missing file → `symbols_missing` /
  `module_offset` remains valid.
- **Privacy**: resolved `file` paths come from the PDB and may contain local
  studio absolute paths; treat dashboard JSON as sensitive when symbols are on.
- **Cache**: bounded per-identity relative-address cache; does not grow with
  every allocation sample (only retained catalog stacks are enriched).
- **Inlines / optimized code**: `pdb-addr2line` may report inline frames; names
  need not match naive source expectations. Raw PCs always remain in `frames`.
- **Library boundary**: `utrace` parsing stays filesystem-free; only the CLI
  (or app) opens PDBs when `--symbol-path` is set.

## Final verification

```text
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
UTRACE_CALLSTACK_FIXTURE=<trace> cargo test --test utrace_fixture --all-features callstack -- --ignored
```

All commands exit 0. The real-fixture comparison records at least three matching
module/function frames against Insights.

## STOP conditions

- Module build identity cannot be validated for the chosen backend.
- The resolver dependency has incompatible licensing or is unmaintained.
- Symbolization would require network access during normal parsing.
- Address mapping differs from Insights and the base-shift/lifetime cause is
  not understood.
- Supporting Windows requires pretending other advertised formats are decoded.

## Maintenance notes

Treat module+offset as a durable output, not an error fallback. Add other
platform backends independently behind the resolver interface. Review every
change for wrong-build symbol acceptance and unbounded cache/string growth.

