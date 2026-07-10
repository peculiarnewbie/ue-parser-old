# Plan 003: Memory + LLM allocation provider (P0)

> **Executor instructions**: Follow step by step. This plan **requires a new
> fixture capture** — the CPU-frame fixture will never grow Memory/LLM traffic.
> STOP if you cannot obtain or synthesize a capture. Update `plans/README.md`
> when done.
>
> **Drift check (run first)**:
> `git diff --stat 3c97e3c..HEAD -- src/utrace.rs src/lib.rs tests/utrace_fixture.rs memory/utrace-coverage-matrix.md`
> On mismatch, STOP.

## Status

- **Priority**: P0 (upgraded — crucial for normal game load analysis)
- **Effort**: L
- **Risk**: HIGH (format versioning, huge event volume, callstack ids)
- **Depends on**: none (parallel with 001/002). Memory capture available;
  LLM `TagValue` capture still needed.
- **Category**: direction
- **Planned at**: commit `3c97e3c`, 2026-07-11

**Progress (2026-07-11):** `targeted-providers.utrace` supplies a UE v2 Memory
capture with Init, TagSpec, AllocSystem/FreeSystem, and realloc events. The
bounded Memory allocation milestone is implemented and fixture-verified. This
capture declares no LLM events, and allocation-to-tag attribution requires
preserving scoped-event enter/leave style in serial dispatch; both remain
explicit follow-up work in this plan.

## Why this matters

Hitch and streaming analysis without memory is incomplete: LLM tag spikes and
alloc churn explain stalls that CPU scopes alone cannot. `basic-cpu-frame.utrace`
declares `Memory.MemoryScope` but observes **0** events and has **no**
`Memory.Alloc` / `LLM.TagValue`. That is a **fixture gap**, not a reason to
deprioritize the provider.

Today the parser only counts `Memory.MemoryScope` tags:

```4703:4725:src/utrace.rs
fn decode_memory_scope(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
) -> Result<i32, TraceError> {
    read_i32_field(event, data, "Tag", base_offset)
}

fn memory_dashboard(scopes: BTreeMap<i32, u64>) -> MemoryDashboard {
    // tag → count only
}
```

`EVENT_COVERAGE` note: tag counts only; no tag catalog / alloc streams.

## Current state (UE 5.7 sources)

Engine tree: `C:\Users\Ryzen\Perforce\Arif_UE-ManaBreak` (adjust if local path differs).

| Logger | Events | Writer | Insights analyzer |
|--------|--------|--------|-------------------|
| `Memory` | `Init`, `Marker`, `Alloc`/`Free`/`Realloc*`, `HeapSpec`, `TagSpec`, `MemoryScope`, `MemoryScopePtr`, … | `Runtime/Core/Private/ProfilingDebugging/MemoryTrace.cpp` + `TagTrace.cpp` | `Developer/TraceServices/Private/Analyzers/AllocationsAnalysis.cpp` |
| `LLM` | `TagsSpec`, `TrackerSpec`, `TagSetSpec`, `TagValue` | `Runtime/Core/Private/HAL/LowLevelMemTracker.cpp` | `…/Analyzers/MemoryAnalysis.cpp` |

`Memory.Init` fields (SizeShift, Version, MinAlignment, …) are **required**
before interpreting packed `Size` on Alloc events.

`Memory.TagSpec` (`TagTrace.cpp`): `Tag`, `Parent`, `Display` (ANSI string).

Channels: `MemAllocChannel`, `MemTagChannel` — must be enabled at capture time.

Targeted fixture today only requires `Memory.MemoryScope` counts
(`tests/utrace_fixture.rs` ~1244–1268) — **not** alloc lifecycles.

AGENTS.md: new provider → **new module** (`src/utrace_memory.rs`), not more
sprawl in `utrace.rs`. Bound retained samples. Callstack **ids** may be
stored; symbolization is out of scope (no Callstack.* decoder yet).

## Commands you will need

| Purpose | Command | Expected |
|---------|---------|----------|
| Harvest | `bash scripts/harvest-ue-trace-events.sh <UE_ROOT> > /tmp/ue_events.txt` (or Git Bash on Windows) | file contains `Memory.Alloc` and `LLM.TagValue` |
| Coverage | `uasset utrace coverage $UTRACE_MEMORY_FIXTURE --universe ue_events.txt --format json` | Memory/LLM declared; after decode, not all raw |
| Tests | `cargo test --all-features utrace_memory` | pass |
| Ignored | `UTRACE_MEMORY_FIXTURE=… cargo test --test utrace_fixture --features utrace memory_ -- --ignored` | pass |

## Scope

**In scope**:
- New `src/utrace_memory.rs` (+ wire in `src/lib.rs` / `utrace` mod tree)
- `EVENT_COVERAGE` entries for Memory + LLM events you implement
- Dashboard `memory` object expansion (additive schema)
- `tests/utrace_fixture.rs` — `UTRACE_MEMORY_FIXTURE` resolver + ignored test
- `README.md` fixture docs
- `memory/utrace-coverage-matrix.md` Memory row
- Synthetic unit tests with crafted packets / hex fixtures under
  `tests/fixtures/tiny/` if feasible

**Out of scope**:
- Callstack symbolization (`Callstack.*`)
- Full Insights Allocations UI parity (heap graph, free-list, etc.)
- Storing every alloc interval unbounded — use summaries + bounded samples
- Changing CPU-frame fixture expectations to require Memory allocs

## Git workflow

- Branch: `advisor/003-memory-llm-allocation-provider`
- Commits: e.g. `Add Memory/LLM utrace dashboard provider`

## Steps

### Step 0: Capture recipe (BLOCKING — human or automation)

Produce `UTRACE_MEMORY_FIXTURE` (suggested path
`D:\Perforce\Arif_Fixtures\Traces\memory-llm-load.utrace`):

1. Editor or packaged game with Trace enabled.
2. Enable channels at least: `MemAlloc`, `MemTag` (exact UI names may be
   “Memory allocations” / LLM tag channel — match `MemAllocChannel` /
   `MemTagChannel` in engine).
3. Also enable enough CPU/frame context to correlate (`Cpu`, `Gpu`, counters
   optional).
4. Reproduce a **load** or streaming hitch (map load, asset burst), 10–60s.
5. Confirm with inventory before coding:

```text
uasset utrace inventory $UTRACE_MEMORY_FIXTURE --format json
```

Must show non-zero `Memory.Init`, `Memory.Alloc` (or AllocSystem),
`Memory.Free`, `Memory.TagSpec`, and ideally `LLM.TagsSpec` + `LLM.TagValue`.

If LLM channel cannot be enabled in this build, document and implement
Memory alloc path first; keep LLM types ready but mark test `#[ignore]`
with reason.

**Verify**: inventory counts printed and saved into the PR description /
plan status note.

### Step 1: Module skeleton + Init/TagSpec

Create `src/utrace_memory.rs`:

- Decode `Memory.Init` → store `version`, `size_shift`, `min_alignment`, …
- Decode `Memory.TagSpec` → `BTreeMap<i32, {parent, display}>`
- Resolve `Memory.MemoryScope` tag ids to display names in dashboard
- Register in `EVENT_COVERAGE` as Partial/Decoded with accurate notes

Wire dispatch from `read_dashboard_events` important/normal streams the same
way other providers are wired (see Counters / LoadTime patterns).

**Verify**: unit test with synthetic Init+TagSpec+MemoryScope yields named
scope in `memory.scopes`.

### Step 2: Alloc / Free / Realloc summaries (bounded)

Implement Insights-inspired **summaries**, not a full live heap unless cheap:

Minimum viable dashboard fields (names can vary but must be stable JSON):

- `memory.init` — version / size_shift
- `memory.tags` — tag catalog sample
- `memory.allocs` — `{ count, bytes_allocated, bytes_freed, net_bytes, by_root_heap[], by_tag_top[], samples: [] }`
- Decode packed size using `SizeShift` from Init (see `MemoryTrace.cpp`
  Alloc logging)
- Cap `samples` (e.g. 40) and top-N by tag/heap
- Count `unresolved_free` (free address never seen) without retaining all
  addresses forever — use a bounded outstanding map with overflow counter
  (AGENTS.md). If overflow is hit, set `outstanding_overflow: true` and
  STOP expanding the map.

Support at least: `Alloc`, `Free`, `ReallocAlloc`, `ReallocFree`.
System/Video variants: count separately or fold with a `kind` field.

**Verify**: synthetic alloc+free net_bytes → 0; fixture shows
`allocs.count > 0` and `bytes_allocated > 0`.

### Step 3: LLM catalog + TagValue samples

Decode:

- `LLM.TagsSpec`, `TrackerSpec`, `TagSetSpec`
- `LLM.TagValue` — arrays of tag ids + values at a cycle

Dashboard:

- `memory.llm.tags`, `memory.llm.latest_values` (bounded), `sample_events`

Follow `MemoryAnalysis.cpp` field names.

**Verify**: on fixture with LLM, `sample_events > 0` and at least one named
tag value; without LLM, fields empty and status still ok.

### Step 4: Fixture test + env var

Add `UTRACE_MEMORY_FIXTURE` resolver beside existing helpers in
`tests/utrace_fixture.rs`.

Ignored test `memory_llm_utrace_fixture_exposes_alloc_and_tag_summaries`:

- `Memory.Init` observed
- `dashboard.memory.allocs.count > 0`
- tag names resolved for any MemoryScope present
- if `LLM.TagValue` observed → `llm.sample_events > 0`

Document in README next to other UTRACE_* vars.

**Verify**: ignored test passes with env set; skips/fails loudly when unset
per existing fixture helper patterns.

### Step 5: Coverage matrix + gates

Update `memory/utrace-coverage-matrix.md` Memory row to describe alloc + LLM
partial provider. Run fmt/clippy/test baseline.

## Test plan

- Unit: Init size_shift packing/unpacking against known Size values.
- Unit: TagSpec parent/display.
- Unit: outstanding map overflow counter trips without OOM.
- Tiny hex fixture optional if packet crafting is already used elsewhere
  (`tests/tiny_corpus.rs` pattern).
- Ignored real memory fixture test.

## Done criteria

- [x] `src/utrace_memory.rs` exists and is wired
- [x] `EVENT_COVERAGE` lists implemented Memory events (not silent raw)
- [x] Dashboard exposes alloc summaries + tag names
- [x] `UTRACE_MEMORY_FIXTURE` ignored test exists and passes on a real capture
- [x] CPU-frame fixture still passes (Memory sections may remain empty)
- [ ] Clippy/tests clean; README + matrix updated; plan status DONE (pending LLM capture/decoder)

## STOP conditions

- Cannot capture any Alloc events after enabling channels — stop; do not fake
  alloc decode from MemoryScope alone.
- `Memory.Init` Version outside Insights supported range (see
  `AllocationsAnalysis.cpp` Min/MaxSupportedVersion) — stop and report
  version; do not guess Size packing.
- Full address-map heap tracking exceeds memory on a 1–2 min game capture —
  keep summaries + overflow counters; do not disable bounds.
- Pressure to implement Callstack symbolization in this plan — defer.

## Maintenance notes

- Reviewers: confirm SizeShift handling against a known alloc size from
  Insights on the same file.
- Game-load CI should eventually require `UTRACE_MEMORY_FIXTURE` like
  targeted providers.
- Follow-up: join alloc spikes to frame_correlation via Marker cycles.
