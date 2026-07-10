# Plan 001: Fix inflated global CPU scope totals

> **Executor instructions**: Follow step by step. Run every verification
> command and confirm the expected result before the next step. If anything in
> STOP conditions occurs, stop and report — do not improvise. When done, update
> the status row for this plan in `plans/README.md`.
>
> **Drift check (run first)**:
> `git diff --stat 3c97e3c..HEAD -- src/utrace.rs tests/utrace_fixture.rs`
> If in-scope files changed, compare "Current state" excerpts to live code;
> on mismatch, STOP.

## Status

- **Priority**: P0
- **Effort**: M
- **Risk**: MED
- **Depends on**: none
- **Category**: bug
- **Planned at**: commit `3c97e3c`, 2026-07-11

## Why this matters

On `basic-cpu-frame.utrace` (~9s capture), global `cpu.scopes` ranks
`UpdateAllPrimitiveSceneInfos` and `PrepareViewStateForVisibility` at
~3972s total each. That makes top-scope dashboards unusable for hitch
hunting. Per-frame correlated tops look sane, so the bug is in global
aggregation / cycle reconstruction / inclusive accounting — not missing
data. Until totals are trustworthy, every other CPU perf view is suspect.

## Current state

- `src/utrace.rs` — `decode_cpu_batch` reconstructs cycles and accumulates
  `scope_totals` on leave (`~7015–7085`).
- `src/utrace.rs` — `scope_summaries` emits `cpu.scopes` sorted by
  `total_cycles` (`~2898`, `~5961–5984`).
- `src/utrace_dispatch.rs` — supplies `scope_cycle` for late-connect base.
- `tests/utrace_fixture.rs` — real fixture asserts dashboard shape but does
  **not** assert `total_seconds <= capture_span` for top scopes.

Cycle reconstruction today:

```7028:7047:src/utrace.rs
        let mut cycle = first >> 2;
        // Relative delta against the previous absolute cycle on this thread.
        if cycle < state.thread_state.last_cycle {
            cycle = cycle.saturating_add(state.thread_state.last_cycle);
        }
        // Late-connect / missing absolute base (Insights ProcessBufferV2).
        if cycle < base_cycle {
            cycle = cycle.saturating_add(base_cycle);
        }
        match first & 0b11 {
            0b00 => {
                if let Some(entry) = state.thread_state.stack.pop() {
                    let duration = entry
                        .accumulated_cycles
                        .saturating_add(cycle.saturating_sub(entry.start_cycle));
                    match entry.kind {
                        CpuStackEntryKind::PlainSpec(spec_id) => {
                            let total = state.scope_totals.entry(spec_id).or_insert((0, 0));
                            total.0 += 1;
                            total.1 = total.1.saturating_add(duration);
```

Observed smoking gun on the fixture (cycle_frequency = 10_000_000):

| Scope | count | total_seconds | avg |
|-------|------:|-------------:|----:|
| UpdateAllPrimitiveSceneInfos | 7518 | ~3971.60 | 0.53s |
| PrepareViewStateForVisibility | 1074 | ~3971.24 | 3.70s |
| WaitForTasks | 43877 | ~12.05 | ok-ish |
| FEngineLoop::Tick | 1072 | ~8.96 | matches capture |

Both bad scopes share nearly the **same** absurd total — treat that as a
lead (shared bad duration path / shared base), not coincidence.

UE reference for intended semantics:
`Engine/Source/Runtime/Core/Private/ProfilingDebugging/CpuProfilerTrace.cpp`
and Insights `CpuProfilerTraceAnalysis.cpp` (`ProcessBufferV2`).

Repo conventions: AGENTS.md — no unbounded alloc from file counts; prefer
enums over stringly dispatch; keep hot paths free of eager `format!`.
Verification: `cargo fmt`, `clippy --all-targets --all-features -- -D warnings`,
`cargo test --all-targets --all-features`.

## Commands you will need

| Purpose | Command | Expected on success |
|---------|---------|---------------------|
| Drift | `git diff --stat 3c97e3c..HEAD -- src/utrace.rs tests/utrace_fixture.rs` | empty or understood drift |
| Unit tests | `cargo test --all-features decode_cpu_batch -- --nocapture` | pass (adjust filter to match new test names) |
| Full | `cargo test --all-targets --all-features` | pass |
| Clippy | `cargo clippy --all-targets --all-features -- -D warnings` | pass |
| Fixture | `UTRACE_FIXTURE=D:\Perforce\Arif_Fixtures\Traces\basic-cpu-frame.utrace cargo test --test utrace_fixture --features utrace real_utrace_fixture_exposes_cpu_dashboard_summary -- --ignored` | pass + new total-span assertion |

## Scope

**In scope**:
- `src/utrace.rs` (cycle reconstruction, scope total accounting, anomaly counters)
- `tests/utrace_fixture.rs` (span sanity assertion)
- New unit tests in `src/utrace.rs` `#[cfg(test)]` module (or a focused test module if one already exists nearby)
- `memory/utrace-coverage-matrix.md` — one-line note that global totals are span-checked

**Out of scope**:
- Timeline / frame-cap work (plan 002)
- Memory/LLM (plan 003)
- Changing JSON schema field names for `cpu.scopes` (may **add** diagnostic fields)
- Rewriting `utrace_dispatch.rs` unless drift proves `scope_cycle` is the sole cause — if so, STOP and report before large dispatch changes

## Git workflow

- Branch: `advisor/001-fix-cpu-scope-totals`
- Commit style from recent log: imperative sentence, e.g. `Fix inflated CPU scope cycle totals`
- Do NOT push/PR unless asked

## Steps

### Step 1: Reproduce and classify the inflation

Run dashboard JSON against the fixture and compute capture span from
`Misc.BeginFrame`/`EndFrame` cycles (or `frames` markers) using
`prologue.cycle_frequency`.

Record for the two bad scopes and for `FEngineLoop::Tick`:
`count`, `total_cycles`, `total_seconds`, `total_seconds / capture_span`.

Hypotheses to falsify (test one at a time with synthetic batches):

1. **Late-connect double-add**: `batch_base_cycle` wrong → absolute cycle too
   large → `duration = cycle - start` huge.
2. **Relative-delta misfire**: `cycle < last_cycle` branch applied when the
   value was already absolute.
3. **Inclusive cross-thread sum is “correct” but useless**: totals are
   inclusive and summed across threads; still should not exceed
   `threads * capture_span` by orders of magnitude for a single scope —
   if they do, it is still a bug.
4. **Metadata vs plain double-count into `scope_totals`**: plain leave path
   should be the only writer to `scope_totals`; confirm metadata leaves do
   not also write the same spec id.

**Verify**: a short script or `uasset utrace dashboard … --format json`
shows the two scopes still > 10× capture span before the fix.

### Step 2: Add failing characterization tests

Add unit tests that encode the bug:

1. Synthetic EventBatchV3 (reuse patterns around existing tests near
   `src/utrace.rs` ~8797–9093: coroutine, late-connect, restored metadata).
2. A fixture-gated assertion:
   `for scope in cpu.scopes.iter().take(20) { assert!(scope.total_seconds <= capture_span * thread_count_fudge) }`
   with a documented fudge (start with `thread_count` from `thread_info.len()`
   or a fixed generous multiplier like `64.0` — pick one and document why).

**Verify**: `cargo test --all-features <new_test_name>` **fails** before the fix.

### Step 3: Fix cycle/duration accounting

Implement the minimal fix that makes Step 2 pass and matches Insights
`ProcessBufferV2` behavior. Prefer:

- Correct base-cycle selection (`raw_event.scope_cycle` vs prologue start).
- Reject / clamp impossible durations (e.g. duration > remaining capture
  estimate) only as a **safety net with a counted anomaly**, not as the
  primary fix — silent clamping hides bugs.
- Add dashboard diagnostics on `cpu.batches` (or similar):
  `implausible_duration_count`, `implausible_duration_cycles` if you clamp.

Re-read UE analyzer before changing the relative-delta order.

**Verify**: characterization tests pass; fixture top scopes for
`FEngineLoop::Tick` remain ~capture length; the two previously-bad scopes
drop below the span fudge.

### Step 4: Fixture contract + docs

Update `real_utrace_fixture_exposes_cpu_dashboard_summary` with the span
sanity check. Note the fix in `memory/utrace-coverage-matrix.md` CPU profiler
row (one sentence).

**Verify**: ignored fixture test passes with `UTRACE_FIXTURE` set.

### Step 5: Repo gates

**Verify**:

```text
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

All exit 0.

## Test plan

- Unit: late-connect batch where wrong base previously inflated duration.
- Unit: normal nested enter/leave still produces inclusive parent ≥ child.
- Unit: unmatched leave still increments `unmatched_ends`, does not invent
  huge durations.
- Fixture: top-N scope `total_seconds` bounded by capture span × fudge.
- Pattern after existing CPU batch tests in `src/utrace.rs` cfg(test).

## Done criteria

- [ ] No top-20 `cpu.scopes` entry on `basic-cpu-frame.utrace` exceeds
      `capture_span_seconds * documented_fudge`
- [ ] New unit tests exist and pass
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0
- [ ] `cargo test --all-targets --all-features` exits 0
- [ ] Only in-scope files changed
- [ ] `plans/README.md` row → DONE

## STOP conditions

- Root cause is in `utrace_dispatch` serial/`scope_cycle` assignment and needs
  a broad dispatch rewrite — stop after a minimal repro writeup.
- Fix would require changing the public meaning of `cpu.scopes` from inclusive
  to exclusive without a schema bump plan — stop and propose schema change.
- Fixture path missing and `UTRACE_REQUIRE_FIXTURE` cannot be satisfied —
  still land unit-test fix; mark fixture assertion `#[ignore]` only if
  operator agrees (prefer keeping ignored-but-present).

## Maintenance notes

- Plan 002 timelines will amplify whatever duration math you leave behind —
  keep anomaly counters.
- Reviewers: compare before/after top-20 scopes on the same fixture; Tick /
  Scene / WaitForTasks should stay in the same ballpark.
- Deferred: exclusive-time columns, per-thread normalized % of frame.
