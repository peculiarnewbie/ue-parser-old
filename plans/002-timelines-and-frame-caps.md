# Plan 002: Bounded CPU/GPU timelines and configurable frame caps

> **Executor instructions**: Follow step by step. Run every verification
> command before proceeding. STOP conditions mean stop and report.
> Update `plans/README.md` when done.
>
> **Drift check (run first)**:
> `git diff --stat 3c97e3c..HEAD -- src/utrace.rs src/bin/uasset.rs tests/utrace_fixture.rs`
> On mismatch with excerpts below, STOP.

## Status

- **Priority**: P0 (timelines) / P1 (frame caps)
- **Effort**: L
- **Risk**: MED
- **Depends on**: plans/001-fix-cpu-scope-totals.md (land or explicitly waive)
- **Category**: direction
- **Planned at**: commit `3c97e3c`, 2026-07-11

## Why this matters

Game perf analysis needs “what ran during hitch frame N” as an interval
timeline, not only global aggregates. Today:

- CPU timeline exists only via `dashboard --frame N` with a hardcoded 500
  interval cap (`src/bin/uasset.rs`).
- GPU has **no** per-frame interval timeline — only capped summary buckets.
- `gpu.frames` and `frame_correlation.frames` silently `truncate(120)` while
  the fixture has ~4294 CPU frame markers and ~3222 GPU boundaries.

This plan makes hitch inspection possible on the existing CPU-frame fixture
without loading unbounded vectors (AGENTS.md).

## Current state

`DashboardOptions` (`src/utrace.rs`):

```2374:2381:src/utrace.rs
/// Options for dashboard decode beyond the default aggregate summary.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DashboardOptions {
    /// When set, retain a bounded CPU timeline for this metadata frame number.
    pub timeline_frame: Option<u32>,
    /// Max intervals retained in `cpu.timeline` (default 500).
    pub timeline_limit: Option<usize>,
}
```

Hard caps:

```3563:3563:src/utrace.rs
    summaries.truncate(120);
```

```3871:3871:src/utrace.rs
    frames.truncate(120);
```

- `CpuTimelineCollector` (~6376–6452): filters by metadata rendered name
  `"Frame N"`, not `Misc.BeginFrame`/`EndFrame` cycle windows.
- CLI: `src/bin/uasset.rs` wires `--frame` and forces `timeline_limit: Some(500)`.
- Fixture timeline contract: `tests/utrace_fixture.rs` ~590–659.
- Coverage note: `memory/utrace-coverage-matrix.md` — full unbounded timelines
  intentionally incomplete.

AGENTS.md: do not grow `utrace.rs` as a dumping ground; extract collectors if
the diff is large. Bound all retained interval vectors.

## Commands you will need

| Purpose | Command | Expected |
|---------|---------|----------|
| Build | `cargo build --features utrace --bin uasset` | exit 0 |
| Unit/fixture | `cargo test --all-features --test utrace_fixture` | non-ignored pass |
| Ignored timeline | set `UTRACE_FIXTURE` + run ignored CPU dashboard / timeline tests | pass |
| Clippy/fmt/test | see plans/README.md baseline | pass |

## Scope

**In scope**:
- `src/utrace.rs` — `DashboardOptions`, frame summary builders, timeline
  collectors, dashboard JSON fields (`truncated`, `total_frame_count`, etc.)
- `src/bin/uasset.rs` — CLI flags for frame limit / timeline limit / optional
  GPU timeline frame
- `tests/utrace_fixture.rs` — contracts for caps + truncation flags
- `memory/utrace-coverage-matrix.md` — update Frames / CPU / GPU rows
- Optional extract: `src/utrace_timeline.rs` if `utrace.rs` hunk is huge

**Out of scope**:
- Streaming NDJSON sidecar formats (follow-up)
- Fixing CPU total inflation (plan 001)
- Memory/LLM (plan 003)
- HTML dashboard redesign beyond exposing new JSON fields if already rendered
- Raising caps to “unlimited”

## Git workflow

- Branch: `advisor/002-timelines-and-frame-caps`
- Commits: imperative, e.g. `Add configurable utrace frame correlation caps`

## Steps

### Step 1: Replace silent 120 caps with options + metadata

Extend `DashboardOptions`:

```rust
pub struct DashboardOptions {
    pub timeline_frame: Option<u32>,
    pub timeline_limit: Option<usize>,
    /// Max frames retained in `gpu.frames` and `frame_correlation.frames`.
    pub max_frames: Option<usize>, // default 120
    /// Optional GPU timeline for one GPU frame number (EventFrameBoundary).
    pub gpu_timeline_frame: Option<u32>,
    pub gpu_timeline_limit: Option<usize>, // default 500
}
```

Change `gpu_frame_summaries` / `frame_correlation_dashboard` to:

- sort as today
- retain `min(len, max_frames.unwrap_or(120))`
- set `truncated: bool` and `total_frame_count: u64` on the parent structs

**Schema**: bump or extend existing dashboard objects **additively**
(`truncated`, `total_frame_count`). Do not remove fields. If
`schema_version` must bump for consumers, bump it and note in README.

**Verify**: unit test with >120 synthetic frames reports
`total_frame_count > 120` and `truncated == true` when capped.

### Step 2: CLI flags

In `src/bin/uasset.rs` `utrace dashboard`:

- `--max-frames <N>` (default 120)
- `--timeline-limit <N>` (default 500; replace hardcoded Some(500))
- `--gpu-frame <N>` optional (mirrors `--frame` for GPU timeline)

**Verify**: `uasset utrace dashboard --help` lists flags; running with
`--max-frames 10` on the fixture yields ≤10 correlated frames and
`truncated: true`.

### Step 3: CPU timeline windowing improvement

Keep metadata `"Frame N"` as primary (matches current fixture).

Add optional secondary window: if metadata frame is missing but
`Misc.BeginFrame`/`EndFrame` markers exist for the same numeric frame,
document whether you join them — **only if** the fixture’s metadata frame
numbers align with Misc markers. If they do not align, STOP and keep
metadata-only; do not invent a broken join.

Ensure `cpu.timeline` always includes `truncated`, `interval_count`,
`begin_cycle`, `end_cycle` (already present — preserve).

**Verify**: existing ignored timeline fixture test still passes; add case
where limit=2 sets `truncated: true`.

### Step 4: GPU timeline collector (bounded)

Mirror `CpuTimelineCollector` for one `gpu_timeline_frame`:

- Key off `EventFrameBoundary` frame number / `current_gpu_frame()`
- Record work and breadcrumb intervals (start/end GPU timestamps, queue id,
  name/spec id, duration)
- Cap with `gpu_timeline_limit`
- Emit `gpu.timeline: Option<…>` additive field

**Verify**: synthetic GPU begin/end work inside one frame appears in
`gpu.timeline`; over-limit sets truncated.

### Step 5: Fixture contracts

On `basic-cpu-frame.utrace`:

- Default dashboard: `frame_correlation.total_frame_count` (or equivalent)
  ≥ 1000 if that matches observed unique frame numbers; `frames.len() <= 120`
  by default; `truncated == true`.
- `--max-frames 500` returns more rows (still bounded).
- `--frame <known>` and `--gpu-frame <known>` return timelines with
  `interval_count > 0` for a mid-capture frame that has CPU metadata.

**Verify**: ignored tests pass with `UTRACE_FIXTURE` set.

### Step 6: Repo gates

`cargo fmt`, clippy `-D warnings`, `cargo test --all-targets --all-features`.

## Test plan

- Unit: cap truncation flags.
- Unit: GPU timeline pairing + zero-timestamp breadcrumb ignore still holds.
- Fixture: default 120 vs raised max-frames.
- Fixture: CPU `--frame` regression (existing).
- Pattern: `tests/utrace_fixture.rs` timeline section ~590–659.

## Done criteria

- [x] No silent truncate without `truncated` + `total_frame_count` (or named equivalent)
- [x] CLI exposes `--max-frames` and `--timeline-limit`
- [x] `gpu.timeline` available for `--gpu-frame`
- [x] Fixture tests cover truncation and at least one GPU timeline frame
- [x] Clippy/tests clean; `plans/README.md` → DONE

## STOP conditions

- Plan 001 not done and fixture still shows multi-thousand-second scope
  totals — do not ship timeline as “ready for perf” without calling that out;
  prefer finishing 001 first.
- Metadata frame numbers and GPU `EventFrameBoundary` numbers are different
  spaces with no documented join — keep separate `--frame` / `--gpu-frame`;
  do not silently merge.
- Emitting full uncapped interval vectors “because the user might want them”
  — rejected; use bounds.

## Maintenance notes

- HTML renderer (`utrace html`) may still show 80 frames — update or leave
  with a comment that JSON is authoritative.
- Reviewers: watch JSON size on `--max-frames 5000`; keep defaults small.
- Follow-up: NDJSON interval export for multi-frame scrubbing.
