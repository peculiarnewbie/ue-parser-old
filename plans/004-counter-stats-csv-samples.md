# Plan 004: Counter, Stats.EventBatch2, and CSV sample streams

> **Executor instructions**: Follow step by step. Prefer the existing targeted
> fixture for Counters; extend capture if Stats/CSV samples are missing.
> Update `plans/README.md` when done.
>
> **Drift check (run first)**:
> `git diff --stat 3c97e3c..HEAD -- src/utrace.rs tests/utrace_fixture.rs`
> On mismatch, STOP.

## Status

- **Priority**: P1
- **Effort**: M
- **Risk**: MED
- **Depends on**: fixture with samples (`UTRACE_TARGETED_FIXTURE` and/or richer capture)
- **Category**: direction
- **Planned at**: commit `3c97e3c`, 2026-07-11
- **Status**: DONE (2026-07-12)
- **Fixture inventory (studio traces)**:
  - `targeted-providers.utrace`: `Counters.SetValueInt` observed; Stats/CSV catalogs only — **no** `EventBatch2` / BeginStat / CustomStat samples
  - `basic-cpu-frame.utrace`: catalogs only; Counter samples = 0
  - Stats/CSV sample paths unit-tested synthetically; live `sample_events > 0` needs StatsChannel + CSV profiler capture
- **Delivered**: `utrace_stats_batch.rs`, `utrace_csv.rs`, real sample_events, EVENT_COVERAGE Partial rows

## Why this matters

Catalogs without samples cannot plot frame-time, draw counts, or custom CSV
curves. On `basic-cpu-frame.utrace`: 209 counter specs, 1132 stat specs, 34 CSV
defs, but **0** value samples. Counters decoding already exists when samples
are present; Stats and CSV sample events are still catalog-only.

## Current state

Counters — implemented (bounded samples):

- `Counters.Spec` / `SetValueInt` / `SetValueFloat` in `EVENT_COVERAGE`
- `counter_dashboard` keeps min/max/latest + ≤40 sample points

Stats — catalog only; samples hardcoded zero:

```4156:4158:src/utrace.rs
        sample_events: 0,
        groups,
        stats,
```

UE has `Stats.EventBatch2` (`uint8[] Data`) with opcodes Increment/Decrement/
AddInteger/SetInteger/AddFloat/SetFloat
(`Runtime/Core/Private/Stats/StatsTrace.cpp`). Insights:
`StatsTraceAnalysis.cpp` routes `EventBatch` + `EventBatch2`.

CSV — registration only; `sample_events: 0` hardcoded (~4298). UE events in
`CsvProfilerTrace.cpp`: `BeginStat`, `EndStat`, `BeginExclusiveStat`,
`EndExclusiveStat`, `CustomStatInt`, `CustomStatFloat`, `Event`,
`BeginCapture`, `EndCapture`, `Metadata`.

Targeted fixture already requires `Counters.SetValueInt` and
`dashboard.counters.samples > 0`
(`tests/utrace_fixture.rs` ~1244–1259).

## Commands you will need

| Purpose | Command | Expected |
|---------|---------|----------|
| Targeted | `UTRACE_TARGETED_FIXTURE=… cargo test --test utrace_fixture --features utrace targeted_utrace_fixtures_exercise_provider_lifecycles -- --ignored` | pass |
| Unit | `cargo test --all-features stats_event_batch csv_` | pass |
| Baseline | fmt / clippy `-D warnings` / `cargo test --all-targets --all-features` | pass |

## Scope

**In scope**:
- `src/utrace.rs` and/or new `src/utrace_stats_batch.rs` / `utrace_csv.rs`
- `EVENT_COVERAGE` for `Stats.EventBatch2` and CSV sample events you decode
- Dashboard: real `stats.sample_events`, `csv.sample_events`, bounded series
- Tests: unit + extend targeted fixture assertions when events present
- `memory/utrace-coverage-matrix.md` Counters/Stats/CSV rows
- README note if a new `UTRACE_CSV_FIXTURE` is required

**Out of scope**:
- Full Insights counter provider UI
- Replacing Counters with Stats or vice versa
- Unbounded per-stat time series in default dashboard JSON

## Git workflow

- Branch: `advisor/004-counter-stats-csv-samples`
- Commit example: `Decode Stats.EventBatch2 and CSV profiler samples`

## Steps

### Step 1: Confirm fixture inventory

On targeted + CPU fixtures, list observed:

- `Counters.SetValueInt` / `SetValueFloat`
- `Stats.EventBatch2`
- `CsvProfiler.BeginStat` / `CustomStatInt` / …

If Stats/CSV sample events are absent from all local fixtures, capture a short
trace with CSV profiling + stats channel enabled (document recipe in PR).
Do **not** mark Stats/CSV done based only on Counter samples.

**Verify**: written inventory snippet in the PR/plan status.

### Step 2: Stats.EventBatch2 decoder

Implement varint/opcode batch decode mirroring `FStatsTraceInternal::EOpType`
and Insights `FStatsAnalyzer` batch handling.

Dashboard updates:

- Increment `sample_events` actually observed
- Per-stat min/max/latest (like counters) for a bounded set of hot stats
- Cap sample points (≤40 per stat or global top stats by |delta|)

Add `EVENT_COVERAGE` Partial note: “batch samples summarized; no full series”.

**Verify**: unit test with hand-crafted Data blob for SetInteger/AddFloat;
`sample_events > 0` on a fixture that contains EventBatch2.

### Step 3: CSV sample events

Decode at least:

- `BeginStat` / `EndStat` (pair into durations by stat id + thread if fields allow)
- `CustomStatInt` / `CustomStatFloat` (value samples)

Wire `csv.sample_events` to a real counter. Emit bounded
`csv.stat_samples` / top durations.

**Verify**: unit tests for begin/end pair; unmatched ends counted.

### Step 4: Counters — fixture CI clarity

Counters code path is done; ensure:

- Targeted ignored test remains the gate for samples
- Optional: document that CPU-frame fixture may have 0 samples (not a regression)
- If sample_points are too coarse for hitch curves, raise per-counter cap via
  `DashboardOptions` (default stays 40)

**Verify**: targeted test still passes.

### Step 5: Gates + docs

Update coverage matrix. Run fmt/clippy/full tests.

## Test plan

- Unit: Stats batch opcodes (set/add int/float, inc/dec).
- Unit: CSV begin/end + custom stat.
- Ignored: targeted fixture counters > 0; if new CSV/Stats fixture, assert
  `sample_events > 0`.
- Pattern: existing `summarizes_counter_specs_and_values` unit test in
  `src/utrace.rs`.

## Done criteria

- [ ] `stats.sample_events` is not hardcoded `0` when EventBatch2 is present
- [ ] At least Begin/End or CustomStat CSV samples decoded when present
- [ ] `EVENT_COVERAGE` updated for new events
- [ ] Unit tests cover batch opcodes
- [ ] Clippy/tests clean; plan status DONE

## STOP conditions

- `EventBatch2` encoding in this engine build differs from StatsTrace.cpp
  opcodes and Insights analyzer disagrees — stop with hex dump of one batch.
- CSV exclusive stat nesting rules unclear — implement non-exclusive
  Begin/End first; defer exclusive.
- Pressure to emit full per-frame CSV tables unbounded — keep caps.

## Maintenance notes

- Stats batches can be huge; keep aggregation streaming (one pass).
- Reviewers: compare a few STAT_ values against Insights on the same utrace.
- Follow-up: align Stats samples into `frame_correlation` as optional curves.
