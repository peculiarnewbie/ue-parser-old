# Plan 005: TaskTrace waits and thread-group membership

> **Executor instructions**: Follow step by step. `WaitForTasks` CPU scope
> names alone are not wait-edge attribution — implement TaskTrace. Update
> `plans/README.md` when done.
>
> **Drift check (run first)**:
> `git diff --stat 3c97e3c..HEAD -- src/utrace.rs tests/utrace_fixture.rs`
> On mismatch, STOP.

## Status

- **Priority**: P1
- **Effort**: L
- **Risk**: MED
- **Depends on**: plan 002 helpful (frame correlation for wait spans); not hard-blocked
- **Category**: direction
- **Planned at**: commit `3c97e3c`, 2026-07-11
- **Status**: DONE (2026-07-12)
- **Fixture inventory**: CPU-frame has ThreadGroup* but no TaskTrace; waits unit-tested; `UTRACE_TASKS_FIXTURE` ignored gate; WaitForTasks overlap correlation deferred
- **Delivered**: thread-group membership on thread_info/cpu.threads; `utrace_tasks.rs`; dashboard `tasks`; EVENT_COVERAGE

## Why this matters

On the CPU-frame fixture, `WaitForTasks` is a real top cost (~12s inclusive).
Without TaskTrace, you cannot answer “waited on what?”. Thread groups are
counted (`ThreadGroupBegin/End`) but not attached to `$Trace.ThreadInfo`
threads, so thread-pool vs game-thread views stay weak.

## Current state

- No `TaskTrace` / `Tasks` entries in `EVENT_COVERAGE`; absent from `src/`.
- UE writer: `Runtime/Core/Private/Async/TaskTrace.cpp` events:
  `Init`, `Created`, `Launched`, `Scheduled`, `SubsequentAdded`, `Started`,
  `Finished`, `Completed`, `Destroyed`, `WaitingStarted`, `WaitingFinished`.
- Insights: `TasksAnalysis.cpp` (TraceServices).
- Thread groups: partial stack accounting in `src/utrace.rs` (~5050–5116);
  matrix note: membership not attached to threads.
- Fixture expects balanced `BackgroundThreadPool` group begins/ends
  (`tests/utrace_fixture.rs` ~680–699).

AGENTS.md: new family → `src/utrace_tasks.rs`, not dump into `utrace.rs`.

## Commands you will need

| Purpose | Command | Expected |
|---------|---------|----------|
| Inventory | `uasset utrace inventory $TRACE --format json` | TaskTrace.* observed on task fixture |
| Tests | `cargo test --all-features utrace_tasks` | pass |
| Baseline | fmt / clippy / full test | pass |

## Scope

**In scope**:
- `src/utrace_tasks.rs` (+ wire-up)
- Thread-group membership attachment on `thread_info` / `cpu.threads`
- `EVENT_COVERAGE` for TaskTrace events implemented
- Dashboard `tasks` object (additive)
- Fixture env `UTRACE_TASKS_FIXTURE` or extend targeted corpus
- Docs: README + coverage matrix

**Out of scope**:
- Full task graph UI / critical-path solver
- Replacing CPU `WaitForTasks` scope aggregation
- Platform context-switch traces (`PlatformEvent`)

## Git workflow

- Branch: `advisor/005-tasktrace-waits-thread-groups`
- Commit example: `Add TaskTrace wait summaries and thread group membership`

## Steps

### Step 0: Capture / locate TaskTrace fixture

Need a trace with TaskTrace channel enabled during gameplay or editor tick.
Confirm inventory shows `TaskTrace.WaitingStarted` / `WaitingFinished` and
lifecycle events.

If unavailable, implement thread-group membership first (works on
`basic-cpu-frame.utrace`) and leave TaskTrace tests `#[ignore]` with clear
env requirements — but do not mark the plan DONE until TaskTrace decode
lands against a real or synthetic capture.

**Verify**: inventory list saved.

### Step 1: Thread-group membership (CPU-frame fixture)

When processing `$Trace.ThreadGroupBegin` / `End`, associate the **current
thread id** of the event with the active group stack (Insights attaches
groups to threads).

Emit on each `thread_info` / `cpu.threads` entry:

- `groups: ["BackgroundThreadPool", …]` or `active_group`

Keep existing `thread_groups` summary totals for compatibility.

**Verify**: on `basic-cpu-frame.utrace`, Background workers list
`BackgroundThreadPool` (or equivalent); GameThread does not.

### Step 2: TaskTrace lifecycle summaries

Decode core events into:

- task id → name/debug name if present
- counts: created/scheduled/started/completed/destroyed
- wait intervals: pair `WaitingStarted`/`WaitingFinished` →
  `{ thread_id, task_id?, duration_cycles, samples[] }` bounded

Follow field names from `TaskTrace.cpp` / `TasksAnalysis.cpp`.

**Verify**: unit tests with synthetic events; fixture shows
`tasks.wait_count > 0` when waits exist.

### Step 3: Relate waits to CPU scopes (light touch)

Optional but valuable: when a wait interval overlaps a `WaitForTasks` CPU
scope on the same thread (cycle ranges), add `correlated_wait_samples`
bounded list. If overlap logic is ambiguous, skip and document — do not
guess.

**Verify**: either correlated samples > 0 on fixture, or explicit doc that
correlation is deferred.

### Step 4: Coverage + gates

Update `EVENT_COVERAGE`, matrix Tasks row, README env var. Run baseline
verification.

## Test plan

- Unit: thread group stack attach/detach across threads.
- Unit: WaitingStarted/Finished pair + unmatched end counter.
- Fixture: basic-cpu-frame group membership.
- Ignored: tasks fixture wait summaries.
- Pattern: thread group unit test `summarizes_thread_groups` (~9168).

## Done criteria

- [ ] Thread group membership visible on thread dashboard rows for CPU-frame fixture
- [ ] TaskTrace module decodes waits + basic lifecycle counts
- [ ] `EVENT_COVERAGE` includes implemented TaskTrace events
- [ ] Tests + clippy clean; plan status DONE

## STOP conditions

- TaskTrace event field layout differs by protocol and Insights analyzer
  source is unavailable — stop with registry dump of declared fields.
- Waiting* events lack timestamps / task ids needed for pairing — implement
  counts-only and report limitation; do not invent ids.
- Attempt to build a full critical-path solver — out of scope.

## Maintenance notes

- Reviewers: ensure group membership uses the event’s thread id from
  dispatch, not a global “current group”.
- Follow-up: task subsequent edges (`SubsequentAdded`) for dependency views.
- Together with plan 002, waits should eventually show up beside hitch frames.
