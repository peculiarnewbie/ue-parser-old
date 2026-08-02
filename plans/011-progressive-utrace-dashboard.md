# Plan 011: Stream progressive UTrace dashboard updates

> **Executor instructions**: Follow this plan in order. Preserve the existing
> parser limits and bounded collectors at every stage. Run the listed checks
> before advancing. If a stop condition occurs, stop and report it instead of
> weakening a resource limit or changing the public final-dashboard contract.

## Status

- **Priority**: P0 UX/performance
- **Effort**: XL (roughly 3–5 focused engineering weeks)
- **Risk**: HIGH — turns a batch-only parse/output boundary into a streaming
  protocol while native and WASM paths coexist
- **Depends on**: Plan 010's shared output contract and backend dispatcher
- **Planned**: 2026-07-12

## Problem

The `.utrace` page currently waits for two independent, full-file operations
before it renders useful analysis:

1. `utraceDashboard(file, { max_frames: 500 })`
2. `utraceInventory(file)`

For the native backend, each operation uploads the whole capture to Vite,
writes a temporary file, and spawns a separate `uasset` process. For WASM,
each operation reads the complete `File` and parses it in the worker. After
both complete, the page also starts a CPU timeline query for the hottest frame.

That is correct batch behavior, but it feels unlike Unreal Insights: the user
cannot start orienting themselves until all work has finished.

## Product outcome

On file selection, the UI shows a capture workspace immediately. Within the
first useful decode window it receives a bootstrap snapshot (header, session,
channels, thread metadata, and progress). It then receives bounded dashboard
snapshots as packets are decoded: frames, CPU/GPU summaries, counters, memory,
I/O, tasks, annotations, and coverage progressively become available. The
capture remains usable while later sections are incomplete. A final snapshot
has exactly the existing `UtraceDashboard` JSON shape and status semantics.

Inventory is folded into the streaming dashboard state: do not retain a second
full parse just to populate the Capture tab. CPU range timelines and selected
GPU-frame timelines remain explicit, on-demand operations.

## Non-goals

- Live connection to a currently recording Unreal Trace Server. This plan
  streams analysis of a locally selected completed file.
- Replacing the bounded `.utix` CPU range index or making it WASM-compatible.
- Making every table grow without bound. Progressive output must retain the
  same caps, truncation indicators, and total counters as batch output.
- Breaking the existing CLI JSON or WASM final-result contracts.
- Putting more transport, collection, and dashboard code into `src/utrace.rs`.

## Architecture

```text
File selected
  -> UI renders loading workspace immediately
  -> selected backend starts one progressive dashboard session
       native: streamed upload -> temp capture -> Rust session -> NDJSON/SSE
       wasm: File stream -> worker -> Rust session -> postMessage snapshots
  -> typed progress events update Solid signals incrementally
  -> terminal `complete` event carries the existing final dashboard envelope
  -> optional timeline/index work starts only after an explicit user action
```

The core must be transport independent. Introduce a decoder/session boundary
that accepts input incrementally and emits typed snapshots at safe packet
boundaries; native HTTP and the WASM worker are adapters around that boundary.
Do not have the Vite plugin parse UTrace bytes or let the UI reconstruct parser
state from raw events.

### Event protocol

Create a discriminated, versioned protocol at the parser/output boundary. It
must not be a stringly-typed object with optional fields. Suggested shape:

```rust
#[non_exhaustive]
enum ProgressiveDashboardEvent {
    Bootstrap { dashboard: DashboardBootstrap },
    Snapshot { sequence: u64, progress: DecodeProgress, patch: DashboardPatch },
    Complete { dashboard: UtraceDashboardOutput },
    Failed { error: DashboardDecodeError },
}
```

`DashboardPatch` is a tagged union of coherent sections, not arbitrary JSON
merge-patch. A section either carries its complete current bounded value or an
explicitly append-only page with an absolute total/truncation state. The UI
must be able to discard a stale/out-of-order update by `sequence` and recover
by replacing its state with `Complete`.

`DecodeProgress` reports bytes consumed, optional total bytes, packets/events
observed, and decode phase. It is informational only; no percentage is claimed
when the total is unavailable. Use `u64` internally for byte/event totals and
validate conversions at the JS boundary.

Snapshot emission is rate-limited by both packet count and elapsed time (for
example, no more than 10 events/sec). Never format or serialize snapshots on
every decoded event. A final snapshot is always emitted, even if the final
rate-limited interval was not reached.

### Parser ownership and safety

Split streaming work into small modules, for example:

- `src/utrace_session.rs` — bounded incremental input/session lifecycle and
  progress cadence.
- `src/utrace_progress.rs` — typed progressive event/output DTOs shared by
  native and WASM adapters.
- Existing provider modules continue to own their collector state and expose a
  bounded `snapshot()`/`dashboard()` projection as appropriate.
- `src/output.rs` remains the shared final JSON contract; avoid a parallel
  dashboard DTO hierarchy.

The session must retain all existing `Reader` limits, depth/cycle defenses, and
provider caps. File-provided counts must still be checked before allocation.
Streaming is not authorization for unbounded queues: each outbound transport
uses a bounded queue with explicit coalescing (keep the newest snapshot) and a
terminal event that cannot be dropped.

Do not solve incremental input by repeatedly reparsing byte prefixes. That is
quadratic on large captures and can expose partial-packet behavior unlike the
current parser. Extract the existing packet decoder/state machine so a session
can preserve its cursor, declarations, serializers, channels, and provider
state across chunks. A partial packet must stay buffered under an explicit
maximum packet-size limit; EOF must use the existing truncated-input error
semantics.

## Implementation phases

### 1. Establish a batch-equivalent session core

1. Map the current UTrace top-level loop and extract its mutable state into a
   private session type. Keep `inspect`, `inventory`, and `dashboard_with_options`
   as compatibility wrappers that feed a complete byte slice then finish.
2. Add `push_chunk(&[u8])` and `finish()` APIs. The input buffer must have a
   checked maximum; consumed bytes are discarded/compacted without copying the
   full capture repeatedly.
3. Make snapshot construction pure with respect to decode state: a snapshot
   must not consume provider data or change final output.
4. Add synthetic tests that feed identical traces with adversarial chunk
   boundaries (one byte, packet boundary, declaration split, compressed packet
   split) and assert the final output equals current batch output exactly.

**Stop if** extracting the loop changes final dashboard output, partial-success
status, or parser errors for whole-buffer input. Restore batch equivalence
before adding a transport.

### 2. Add progressive contract and bounded snapshots

1. Define serializable `ProgressiveDashboardEvent`, `DashboardBootstrap`,
   `DashboardPatch`, and `DecodeProgress` in the shared output layer.
2. Emit bootstrap as soon as enough metadata is valid; do not wait for a full
   inventory or a complete frame correlation.
3. Emit coalesced snapshots after bounded decode work. Begin with overview,
   threads, frames, CPU, and GPU; add providers one at a time so each has an
   explicit partial/complete state.
4. Finalize by producing the unchanged dashboard envelope and an inventory
   projection from the same session state.
5. Add schema/version documentation and TypeScript discriminated-union types.

**Stop if** a snapshot requires cloning uncapped raw events, grows with trace
size outside existing caps, or forces an eager `format!` into a hot decode path.

### 3. Native progressive endpoint

1. Add `POST /api/utrace/progress` in `web/vite.parse-plugin.ts`; preserve the
   existing batch endpoints for regression fallback during this plan.
2. Stream request bytes to one temporary file with a size cap rather than
   collecting the entire body in memory. Start the parser only after its input
   contract can safely consume the available chunks; if concurrent file tailing
   is not portable, first deliver progressive *analysis* after upload and keep
   transport streaming as a later substep.
3. Spawn a progressive native CLI mode that writes newline-delimited JSON
   events to stdout. Keep diagnostics on stderr. NDJSON is preferred over SSE
   because it works for both CLI pipes and `fetch()` response streams without
   event framing translation.
4. Parse stdout line-by-line in the Vite middleware, validate bounded message
   size/sequence, and forward it as `application/x-ndjson`. The middleware must
   terminate child processes and remove temporary files on client disconnect.
5. Record upload, decoder-first-bootstrap, first-snapshot, and completion
   timings in the existing timing model.

**Stop if** the Node middleware needs to buffer the entire progressive response
or the CLI protocol intermixes log text with JSON. Fix framing first.

### 4. WASM worker progressive adapter

1. Extend the WASM facade with a session handle or a chunk-message API; it must
   share the Rust session implementation rather than duplicate parsing logic in
   TypeScript.
2. Read `File.stream()` in the worker client and transfer bounded chunks to the
   worker. The UI thread only forwards events; it never parses bytes or JSON
   snapshots.
3. Post typed progress events from the worker. Use explicit backpressure: do
   not queue unlimited `ArrayBuffer`s or snapshots while the worker is busy.
4. Keep one final JSON output path and compare its semantic result to the
   native final dashboard under Plan 010's parity rules.

### 5. Replace the UTrace route loading model

1. On selection, immediately set a `LoadingCapture` model containing file
   name, byte size, backend, and zeroed progress. Render the capture bar and
   tabs with skeleton/partial states before any parser response.
2. Subscribe to the progressive stream and reduce events by `sequence` into
   Solid signals. A later section must never erase a previously valid section.
3. Remove `Promise.all([utraceDashboard, utraceInventory])` as the gate to
   rendering. Populate Capture from the stream's shared inventory projection.
4. Remove the automatic hottest-frame `loadFrameTimeline` await from `onFile`.
   Make selected-frame/range timeline actions explicitly user initiated; idle
   prefetch is optional only after completion and must be cancelable.
5. Show honest state labels: `reading`, `indexing`, `analyzing`, `partial`,
   `complete`, and `failed`. Keep a cancel button that aborts fetch and
   terminates/cancels the worker session.
6. Keep a temporary feature-flagged batch fallback until fixture, browser, and
   native streaming coverage pass. Do not silently fall back after an error;
   tell the user which mode failed.

### 6. Retire duplicate batch work deliberately

After streaming final-output parity is stable, have the normal route use the
single progressive dashboard session. Retain standalone CLI `inventory` and
`dashboard` commands as batch tools. Remove browser-side duplicate inventory
work only after confirming Capture receives equivalent data from the session.

## Test and verification matrix

Add tests before switching the default UI path:

| Contract | Test |
|---|---|
| Batch compatibility | Whole-buffer session final result equals `dashboard_with_options` JSON on tiny corpus and real fixture. |
| Chunk invariance | Same trace fed with one-byte, random bounded, and packet-aligned chunks produces identical final output. |
| Truncation | EOF in each packet/header position returns the existing error/status, never hangs or emits `Complete`. |
| Resource bounds | Oversized partial packet, snapshot backlog, and output line all fail/coalesce under named limits. |
| Ordering | Reducer ignores stale sequence values and `Complete` wins over partial patches. |
| Native protocol | NDJSON events are individually valid, bounded, and final output matches the batch CLI. |
| WASM parity | Streaming WASM final output semantically equals native for the existing tiny corpus. |
| UI behavior | Capture shell appears immediately; dashboard sections render on snapshots; inventory does not gate frames; automatic timeline load does not run. |
| Cancellation | Abort cleans server child/temp resources and worker pending state; a subsequent file can be opened. |

Run:

```text
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo check --lib --target wasm32-unknown-unknown --no-default-features --features wasm
cd web && npm run build
UTRACE_REQUIRE_FIXTURE=1 cargo test --test utrace_fixture --features utrace -- --ignored
```

Manually test a representative large capture in both native and WASM modes.
Record time to capture shell, bootstrap, first frame list, and final dashboard;
compare these to the current batch route. Treat a faster final completion that
still delays the first useful frame list as a failure of this plan's goal.

## Acceptance criteria

- Selecting a trace immediately replaces the empty drop-only screen with a
  capture workspace and visible progress.
- A capture's initial dashboard is usable before all providers/inventory finish.
- One normal browser load performs one full dashboard decode, not separate
  dashboard and inventory decodes.
- Final native and WASM dashboard JSON remains semantically equivalent to the
  existing batch contract for supported operations.
- Chunked input, cancellation, malformed data, caps, and partial-success
  behavior are covered by automated tests.
- No added unbounded allocation, recursion, event retention, transport queue,
  or eager hot-loop formatting is introduced.

