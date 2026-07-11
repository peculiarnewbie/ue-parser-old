# Plan 006: Decode bounded callstack catalogs and join callstack IDs

> **Executor instructions**: Follow this plan step by step. Run every verification
> command before continuing. Update this plan's row in `plans/README.md` when done.
>
> **Drift check (run first)**: `git diff --stat 82a0968..HEAD -- src/lib.rs src/utrace.rs src/utrace_callstacks.rs tests/utrace_fixture.rs memory/utrace-coverage-matrix.md`
> If the dashboard dispatch or memory/bookmark callstack fields have materially
> changed, stop and reconcile this plan before implementing it.

## Status

- **Priority**: P0
- **Effort**: L (multi-day, but much smaller than symbolization)
- **Risk**: MED (large catalogs and aux-array wire decoding)
- **Depends on**: none; plan 003 is DONE — join via existing `MemoryAllocationSample.callstack_id`
- **Category**: direction
- **Planned at**: commit `82a0968`, 2026-07-12
- **Status**: DONE
- **Drift check**: `utrace.rs` / coverage matrix grew (memory LLM + dashboard), but
  bookmark `callstack_count` and allocation `callstack_id` fields are unchanged;
  `CallstackSpec` still absent. Safe to proceed.

## Why this matters

Memory allocations and bookmarks already carry `CallstackId`, but the dashboard
cannot resolve those ids. UE 5.7 emits a compact catalog, not symbols:
`Memory.CallstackSpec { CallstackId: uint32, Frames: uint64[] }`. Decoding that
catalog makes raw program-counter stacks available immediately and establishes
a stable input boundary for later symbolization.

## Current state and engine contract

- `src/utrace.rs:43` owns `TraceDashboard`; add a `callstacks` provider output.
- `src/utrace.rs:948` exposes allocation `callstack_id`; `src/utrace.rs:5620`
  reads bookmark ids but currently retains only a count.
- `src/utrace_memory.rs` owns bounded allocation aggregation. Do not move it
  back into `utrace.rs`.
- UE source:
  `Runtime/Core/Private/ProfilingDebugging/CallstackTracePrivate.h` declares
  `Memory.CallstackSpec` with `uint32 CallstackId` and `uint64[] Frames`.
- Insights reference:
  `Developer/TraceServices/Private/Analyzers/CallstacksAnalysis.cpp` caps frame
  count at 255, treats id 0 as absent, and also recognizes the legacy `Id: u64`
  hash form. Current-format numeric ids are the required first milestone.
- AGENTS.md requires checked file-driven allocation counts and new providers in
  small modules. Implement this in `src/utrace_callstacks.rs`.

## Scope

**In scope**:

- `src/utrace_callstacks.rs` (new)
- `src/lib.rs`
- `src/utrace.rs` provider dispatch and serialized types
- `tests/utrace_fixture.rs`
- `memory/utrace-coverage-matrix.md`
- `plans/README.md`

**Out of scope**:

- PDB/DWARF/PSYM parsing, source lines, demangling, or external symbol servers
- module lifetime decoding (`Diagnostics.Module*`)
- unbounded retention of every stack or duplicating frames on every allocation
- changing unrelated memory accounting or bookmark text formatting

## Steps

### Step 1: Add the bounded provider and decoder

Create `src/utrace_callstacks.rs` with typed `CallstackId(u32)`, provider state,
and output summaries. Decode the `Frames` aux array using the declared element
shape (`u64`, little-endian) and a fixed-size bounded child reader. Reject or
truncate malformed/excessive frame counts explicitly; match Insights' semantic
maximum of 255 frames per stack. Apply independent caps to retained stack count
and total retained frames, with `observed`, `retained`, `dropped`, `truncated`,
duplicate-id, and malformed counters. ID 0 means no recorded stack.

Do not allocate directly from the payload count without a `Reader` limit check.
Do not store a copy of a stack at each consumer; the catalog owns frames and
consumers retain only ids.

**Verify**: `cargo test --all-features utrace_callstacks` exits 0. Unit tests
must cover empty, 1/255/256 frames, absurd array size, duplicate id, id 0,
catalog cap, total-frame cap, and truncated payload.

### Step 2: Dispatch `Memory.CallstackSpec` and publish coverage

Wire the provider through serial event dispatch, add the event to
`EVENT_COVERAGE`, and add `TraceDashboard.callstacks`. Output should include
catalog completeness counters and a bounded list of `{ id, frames }`; format
addresses as JSON-safe hexadecimal strings as well as (or instead of) raw JSON
numbers so 64-bit addresses survive JavaScript consumers exactly.

Support current `CallstackId` first. If a fixture proves the legacy `Id` form
is present, implement a separate typed legacy-hash map following Insights;
never narrow the `u64` hash to `u32`.

**Verify**: synthetic dashboard test observes `Memory.CallstackSpec` as partial,
resolves one id to its exact ordered addresses, and reports no raw event.

### Step 3: Join catalog ids to existing consumers without duplication

Expose resolution status on bounded memory allocation samples and retain
bounded bookmark event samples containing their `callstack_id`. A consumer
should distinguish `none` (id 0), `resolved`, `missing`, and `catalog_truncated`.
Keep the canonical frames only in `dashboard.callstacks`; consumer samples
reference the id. Add aggregate unresolved-reference counters.

If plan 003 is changing the memory sample shape concurrently, stop and
reconcile ownership instead of editing overlapping structures independently.

**Verify**: unit tests demonstrate a memory allocation and bookmark resolving
to the same catalog entry without duplicate frame arrays in their JSON objects.

### Step 4: Add a real fixture contract

Add `UTRACE_CALLSTACK_FIXTURE` (or reuse `UTRACE_MEMORY_FIXTURE` when it actually
declares `Memory.CallstackSpec`) and an ignored test requiring at least one
nonzero stack with at least one frame plus at least one resolved consumer id.
Document the capture channels (`Memory` allocation + Callstack) and explicit
skip/require behavior. Update the coverage matrix from `not parsed` to `partial`:
raw addresses and joins are decoded; module/symbol resolution remains absent.

**Verify**:

```text
UTRACE_CALLSTACK_FIXTURE=<trace> cargo test --test utrace_fixture --all-features callstack -- --ignored
```

exits 0 and asserts real decoded frames, not only event declaration.

## Final verification

```text
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

All commands exit 0. `git status --short` lists only in-scope files.

## STOP conditions

- The real trace declares a different `CallstackSpec` schema than UE 5.7 source.
- Aux-array element boundaries cannot be proven from registry metadata.
- Supporting joins requires unbounded allocation or bookmark retention.
- Plan 003 has overlapping in-progress memory output changes.
- A fixture has only callstack declarations but no observed callstack payloads.

## Maintenance notes

Preserve raw addresses even after symbols are added: missing symbol files are a
normal state. Reviewers should focus on array bounds, 64-bit JSON fidelity,
catalog truncation semantics, and avoiding frame-vector duplication.

