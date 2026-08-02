# Plan 013: Flatten live frame chart update cost during progressive streaming

> **Executor instructions**: Follow this plan in order. Phases 1–3 are
> independently shippable and UI-only; phase 4 is optional backend hardening.
> The live chart's product behavior — a full-capture view that keeps
> accumulating frames beyond the server's sliding window — must be preserved
> exactly; a fix that shrinks the live view to the trailing server window is a
> regression, not a simplification. If a stop condition occurs, stop and
> report it.

## Status

- **Priority**: P1 UX/performance (visible degradation while a capture streams)
- **Effort**: S/M
- **Risk**: LOW/MEDIUM — mostly confined to `Utrace.tsx` and `Charts.tsx`;
  the merge rewrite must not change what the chart shows
- **Depends on**: landed progressive streaming UI (plan 011 phases already in
  tree); independent of plan 012
- **Planned**: 2026-07-13

## Problem (observed)

While a large capture streams through the progressive dashboard, the live
"Frame timing streaming" chart updates quickly at first and gets visibly
slower as decode progresses. On the WASM backend the updates can also arrive
faster than the browser paints.

## Verified causes

Each cause below was verified against the current source.

1. **Every snapshot carries the full retained window.** `frame_patch` clones
   the server's entire retained frame deque per snapshot
   (`src/utrace_session.rs:155-167`). The progressive request asks for
   `max_frames: 500` (`web/src/routes/Utrace.tsx:515`), and the window
   *slides*: once full, `pop_front` drops the oldest frame per new frame
   (`src/utrace_session.rs:559-562`). So each snapshot is the newest ≤500
   frames, not a stable prefix.
2. **The client accumulates, and its per-update cost grows with the
   accumulation — this is the observed slowdown.** `updateLiveFrames`
   (`web/src/routes/Utrace.tsx:144-169`) rebuilds a `Map` from every
   already-accumulated frame, inserts the incoming ≤500 (mostly identical
   replacements in the overlap), converts back to an array, sorts the entire
   array, and slices to `MAX_LIVE_CHART_FRAMES = 10_000`
   (`web/src/routes/Utrace.tsx:77-79`). Because the client keeps frames that
   slid out of the server window, the accumulated array grows toward
   min(total frames, 10,000) **during normal loading today** — the 500-frame
   request bounds only the per-snapshot payload, not the merge cost. Early
   snapshots merge into a small array (fast); late snapshots rebuild and
   re-sort thousands of entries (slow).
3. **Each publication re-renders everything, twice.** The new array flows
   through the `points` memo (`buildFramePoints` over the whole accumulated
   array, `web/src/components/Charts.tsx:243-245`) into a fresh
   `peculiar-charts` `<Chart>` data array: scales, axes, and the main
   `<Line>` recompute, plus a second `<Line>` inside `<Brush>`
   (`web/src/components/Charts.tsx:405-419`). During streaming the chart is
   mounted in `fullRange` mode with `brush={null}` and no-op handlers
   (`web/src/routes/Utrace.tsx:769-782`), so the brush mini-chart is
   duplicated line work with no interaction value at that stage.
4. **WASM emits snapshots with no time throttle.** Native gates frame
   snapshots on "frame count/last-frame changed AND ≥100 ms elapsed"
   (`src/bin/uasset.rs:1060-1066`). WASM gates only on change: one frame
   snapshot per pushed chunk whenever a new frame closed (`src/wasm.rs:228`),
   with the worker client pushing 1 MiB chunks
   (`web/src/lib/wasm-worker-client.ts:130-134`) and posting events without
   backpressure. A fast decode can emit far more than one update per paint.

Not causes: both backends already skip snapshots when the frame count and
last frame number are unchanged (`src/bin/uasset.rs:1060`,
`src/wasm.rs:228`), so a client-side "skip if unchanged" guard is cheap
insurance at best, not the fix.

## Product outcome

- Live chart update cost is flat over the life of a stream: merging a
  snapshot costs O(new frames), rendering costs O(fixed point budget) —
  neither scales with how much has already accumulated.
- The live chart still shows the full capture range, accumulating past the
  500-frame server window up to the existing 10,000-frame cap.
- WASM and native streaming feel equivalently smooth; updates are coalesced
  to at most one publication per paint.

## Non-goals

- Changing the progressive protocol, patch shapes, or the server's retained
  sliding-window semantics (plan 011's remit).
- Replacing `peculiar-charts` or virtualizing the post-load charts.
- Reducing `MAX_LIVE_CHART_FRAMES` — the fix is making the cap affordable,
  not shrinking it.

## Phase 1 — Sorted append-merge (kill the O(n log n) accumulation)

1. Replace the Map-rebuild-and-sort in `updateLiveFrames` with a merge that
   exploits sortedness: the incoming patch is the server deque in
   frame-number order, and the accumulated array is already sorted. Binary
   search the accumulated array for the first incoming frame number, replace
   the overlapping suffix, and append the rest — O(overlap + appended), no
   `Map`, no full sort.
2. Preserve object identity for frames whose values are unchanged in the
   overlap (reuse the existing element instead of the freshly mapped one) so
   downstream memos and the chart see stable references for stable data.
3. Keep the `MAX_LIVE_CHART_FRAMES` cap by dropping from the front after the
   merge, as today.
4. Defensive fallback: if an incoming batch is ever not sorted ascending
   (assert in dev), fall back to the current full merge for that batch
   rather than rendering wrong order. Frame timings are recorded once at
   `EndFrame`, so overlap entries are expected to be value-identical;
   replacement (not skip) stays the rule so a changed server value still
   wins.
5. Unit tests (the merge is a pure function — extract it to
   `web/src/lib/` so it is testable without the component): disjoint append,
   full overlap, partial overlap at the slide boundary, cap eviction,
   identity preservation for unchanged frames, unsorted-input fallback.

**Stop if** the live chart's rendered frame range after a full stream differs
from the current implementation's on the same event sequence. Behavior
equivalence is the gate; only the cost may change.

## Phase 2 — Coalesce publication to paint rate

1. Buffer the latest merged array and publish `setLiveFrameTiming` at most
   once per animation frame (or a ~100 ms timer while the tab is hidden,
   since rAF stalls in background tabs). Intermediate snapshots between
   paints are superseded, not queued.
2. Always flush the final state: `complete` and `failed` events force an
   immediate publication so the last frames are never dropped, and the
   coalescer is cancelled in the existing `onCleanup`/abort paths.
3. This neutralizes the WASM cadence at the UI layer for every backend, and
   keeps doing so if a future backend emits even faster.

## Phase 3 — Bound the render cost

1. Downsample the *rendered* series to a fixed budget (~300–800 points)
   inside the streaming chart path, keeping the full accumulated array in
   state. Use min/max-per-bucket (or LTTB) so frame spikes — the thing the
   user is watching for — survive; plain striding hides them.
2. During streaming (`fullRange` mode), drop the `<Brush>` mini-chart: it
   renders a duplicate `<Line>` while its handlers are no-ops
   (`web/src/routes/Utrace.tsx:775-778`). Render the main lines only; the
   brush returns on the post-load interactive chart.
3. Optional follow-up, not a gate: the post-load `FrameCostBrushChart` with a
   10,000-frame retained limit has the same per-render cost. Downsampling it
   is riskier because brush ranges are point-index based — indexes must be
   mapped back to frame numbers before filtering tabs. Defer unless already
   touching the brush plumbing; do not silently change index semantics.

**Stop if** downsampling requires changing `BrushRange` semantics on the
interactive chart to ship the streaming fix — split it out instead.

## Phase 4 — Optional backend hardening (WASM emission gate)

Mirror native's 100 ms gate in the WASM session wrapper (`src/wasm.rs:228`),
using a JS-clock time source (`js_sys`) or a chunk-count gate (e.g. at most
one frame snapshot per N chunks) if adding a time dependency is unwelcome in
the wasm crate. This is defense in depth after phase 2 — do it only if the
worker's event volume itself (serialization + postMessage) shows up in
profiles, and keep the "always emit the final state" invariant.

## Test and verification matrix

| Contract | Test |
|---|---|
| Merge correctness | Pure-function unit tests: append, overlap, slide boundary, cap, identity, unsorted fallback. |
| Behavior equivalence | Replaying a recorded NDJSON event sequence through the old and new `updateLiveFrames` yields identical final arrays. |
| Coalescing | Bursts of N snapshots between paints produce one publication; `complete`/`failed` always flush; abort cancels the pending flush. |
| Downsampling | Bucketed series preserves each bucket's min and max; spike frames remain visible in rendered output. |
| Regression | `cd web && npm run build` plus existing web tests stay green. |

There is no committed `.utrace` fixture for deterministic UI replay, so add
the replay test at the event level: generate a progressive NDJSON recording
once from the CLI's `dashboard-progress` on the synthetic tiny trace (or
check in a recorded event log, which is small JSON) and drive
`updateLiveFrames` from it.

Manually verify on the 259 MB capture, both backends:

1. Chart update latency at the end of the stream matches the start (no
   progressive degradation) — eyeball plus Performance-panel long-task count
   before/after.
2. WASM streaming no longer floods updates between paints.
3. The live chart still ends showing the full capture range (not just the
   last 500 frames), capped at 10,000.

## Acceptance criteria

- Per-snapshot UI work no longer scales with the accumulated frame count:
  merge is O(delta), publication is ≤1 per paint, render is O(point budget).
- The live full-capture accumulation behavior and the 10,000-frame cap are
  unchanged.
- The streaming chart renders one line set (no duplicate brush line) until
  the interactive post-load chart replaces it.
- Final streamed state is always rendered, including on failure and abort.
