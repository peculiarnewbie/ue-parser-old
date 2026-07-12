# UTrace P0/P1 implementation plans

Generated 2026-07-11 against commit `3c97e3c`.

Scope: close every **P0** and **P1** gap from the
`basic-cpu-frame.utrace` perf-gap ranking, with **Memory/LLM upgraded to P0**
even though that family is absent from the CPU-frame fixture.

## Priority ranking (perf-analysis cruciality)

| Pri | Area | Kind | Plan |
|-----|------|------|------|
| P0 | Trustworthy global CPU scope totals | parser | [001](001-fix-cpu-scope-totals.md) |
| P0 | Memory alloc streams + LLM tag values | parser + fixture | [003](003-memory-llm-allocation-provider.md) |
| P0 | Bounded CPU + GPU timelines (Insights-usable) | parser | [002](002-timelines-and-frame-caps.md) |
| P1 | Uncapped / configurable frame correlation | parser | [002](002-timelines-and-frame-caps.md) |
| P1 | Counter / Stats / CSV sample streams | parser + fixture | [004](004-counter-stats-csv-samples.md) |
| P1 | Wait / TaskTrace + thread-group membership | parser + fixture | [005](005-tasktrace-waits-thread-groups.md) |
| P0 | Raw callstack catalog + consumer joins | parser + fixture | [006](006-callstack-catalog-and-joins.md) |
| P0 | Module-aware symbolization | parser + optional tooling | [007](007-module-aware-symbolization.md) |

## Execution order & status

| Plan | Title | Priority | Effort | Depends on | Status |
|------|-------|----------|--------|------------|--------|
| 001 | Fix inflated CPU scope totals | P0 | M | — | DONE |
| 002 | Bounded timelines + configurable frame caps | P0/P1 | L | 001 | DONE |
| 003 | Memory/LLM allocation provider | P0 | L | — (parallel with 001/002 after capture exists) | DONE |
| 004 | Counter / Stats.EventBatch2 / CSV samples | P1 | M | targeted fixture with samples | DONE |
| 005 | TaskTrace waits + thread-group membership | P1 | L | 002 helpful for correlating waits to frames | DONE |
| 006 | Bounded callstack catalog + ID joins | P0 | L | coordinate with 003 | DONE |
| 007 | Module-aware optional symbolization | P0 | L/XL | 006 | DONE |

Status values: `TODO` | `IN PROGRESS` | `DONE` | `BLOCKED` | `REJECTED`

Plan 003 is live-fixture-verified for Memory allocation traffic and
synthetic-wire-verified for LLM catalog/value decoding. A capture with
`MemTagChannel` enabled remains a fixture-expansion follow-up, not an open
provider implementation task.

## Dependency notes

- **001 before 002**: timeline/correlation work must not amplify wrong durations.
  Fix totals (or at least quarantine known-bad scopes) first.
- **003 is parallelizable** with 001/002 once a Memory/LLM capture exists. Do
  not block Memory work on CPU timeline work — game-load analysis needs it
  regardless of frame-timeline polish.
- **004** already has decoder stubs for Counters values; main gap is fixture
  samples + Stats `EventBatch2` + CSV `BeginStat`/`EndStat`/`CustomStat*`.
  Targeted fixture (`UTRACE_TARGETED_FIXTURE`) already requires
  `Counters.SetValueInt`.
- **005** needs a TaskTrace-enabled capture; `WaitForTasks` CPU scopes alone
  are not enough for wait-edge attribution.
- **006 before 007**: raw program counters and stable callstack-id joins are
  useful without symbols and form the deterministic parser boundary.
- **006 and 003 overlap** only at bounded memory allocation samples. Reconcile
  that output shape before either executor edits it concurrently.
- **007 starts with a spike** because module/build-id parity and a trustworthy
  offline Windows symbol backend must be proven before committing to an API.

## Fixture strategy (shared)

| Env | Role |
|-----|------|
| `UTRACE_FIXTURE` | `basic-cpu-frame.utrace` — CPU/GPU volume, totals, timelines, correlation |
| `UTRACE_TARGETED_FIXTURE` / `_DIR` | LoadTime + Counters samples + MemoryScope + MetadataStack restore |
| `UTRACE_MEMORY_FIXTURE` (**new**, plan 003) | MemAllocChannel + MemTagChannel capture with Alloc/Free + LLM TagValue |
| `UTRACE_IOSTORE_FIXTURE` | out of P0/P1 scope (keep ignored test as-is) |
| `UTRACE_TASKS_FIXTURE` | TaskTrace wait traffic (`WaitingStarted` / `WaitingFinished`) |
| `UTRACE_CALLSTACK_FIXTURE` | CallstackSpec + module diagnostics + at least one callstack-bearing consumer |
| `UTRACE_SYMBOL_PATH` | Optional local PDB root for ignored callstack fixture symbolization (`utrace-symbols`) |

Capture recipes live inside plans 003 and 004. The CPU-frame fixture will
**never** exercise Memory/LLM; that is expected, not a parser failure.

## Module split rule (AGENTS.md)

Do **not** dump new providers into `src/utrace.rs`. Prefer new modules:

- `src/utrace_memory.rs` — Memory + LLM (plan 003)
- `src/utrace_stats_batch.rs` / `src/utrace_csv.rs` — Stats.EventBatch2 + CSV samples (plan 004)
- `src/utrace_tasks.rs` — TaskTrace (plan 005)
- `src/utrace_callstacks.rs` — raw callstack catalog and bounded joins (plan 006)
- `src/utrace_symbols.rs` — optional resolver boundary/backend (plan 007)
- Timeline collectors may stay near existing `CpuTimelineCollector` initially,
  but extract if `utrace.rs` growth exceeds ~reviewable hunks.

## Verification baseline (every plan)

```text
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

Fixture-gated:

```text
UTRACE_REQUIRE_FIXTURE=1 cargo test --test utrace_fixture --features utrace -- --ignored
```

## Findings considered and rejected (for this tranche)

- **Full unbounded in-memory timelines for entire traces**: rejected as default
  output. Plan 002 uses configurable bounds + truncation flags (AGENTS.md
  allocation discipline). Streaming/sidecar formats are a follow-up.
- **Bookmarks / regions / Slate deep analysis**: still deferred. Raw callstack
  decoding and symbolization are promoted to P0 in plans 006/007 because they
  make allocation and annotation evidence actionable.
- **IoStore / LoadTime request deep graphs**: useful but not in the P0/P1 list
  for this tranche; targeted fixture already covers basic LoadTime lifecycle.
- **GPU fence pairing as first-class intervals**: deferred; counts stay for now
  unless TaskTrace work naturally needs them.
- **Replacing Unreal Insights**: explicitly out of scope (roadmap).
