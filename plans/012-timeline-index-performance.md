# Plan 012: Eliminate duplicate work in `.utix` timeline index build and query

> **Executor instructions**: Follow this plan in order. Phases 1–3 are
> independently shippable; do not start a later phase until the equivalence
> tests for the previous phase pass. Preserve the existing parser limits,
> bounded collectors, and the `.utix` on-disk contract semantics (bump the
> format version if bytes change). If a stop condition occurs, stop and report
> it instead of weakening a resource limit or skipping an equivalence test.

## Status

- **Priority**: P1 performance (first-timeline-query latency + per-query upload)
- **Effort**: M/L
- **Risk**: MEDIUM — touches the hot CPU-scope decode loop and the web
  timeline transport; every change is guarded by output-equivalence tests
- **Depends on**: Plan 002 `.utix` format; Plan 011 phase 1 session core
  (`src/utrace_session.rs`, landed) for phase 2
- **Planned**: 2026-07-13

## Problem (measured)

On a real 259 MB capture, the first timeline query costs ~17.8 s of Rust CLI
time plus the browser-side upload. Measured with the production command
(`utrace timeline index --max-intervals 1000000`):

- CPU intervals decoded: 33,376,033
- Intervals retained in the index: 1,000,000 (cap reached; `truncated: true`)
- Every subsequent timeline query re-uploads the full 259 MB file, buffers it
  in Node memory, SHA-256 hashes it, and writes a temp copy — even when the
  query is a pure `.utix` cache hit.

## Verified causes

Each cause below was verified against the current source.

1. **Duplicate full parse.** `build_cpu_timeline_index` (`src/utrace.rs:3347`)
   is invoked as a separate `utrace timeline index` process
   (`web/vite.parse-plugin.ts:394`) and reruns `read_header` → `read_packets`
   → `read_event_registry` → `read_known_important_events` →
   `read_dashboard_events` on the whole capture, independent of the
   progressive dashboard parse that already decoded the same bytes.
2. **Full dashboard built and discarded.** The index build calls
   `read_dashboard_events` with `DashboardOptions::default()` and drops the
   result (`let _ =` at `src/utrace.rs:3365`). Every provider — GPU queues,
   counters, memory, callstacks, tasks, logging, CSV, platform-file — is fully
   constructed just to feed the CPU timeline sink.
3. **Per-interval allocation continues past the cap.** The sink early-returns
   once full (`src/utrace_timeline.rs:304`), but the decode loop has already
   cloned the spec-name `String` (or `format!("#{spec_id}")`), resolved the
   active frame, computed `duration_seconds`, and built a full
   `CpuTimelineInterval` before calling `record`
   (`src/utrace.rs:8788-8810`, and the metadata-scope arm below it). On the
   measured capture that is ~32.4 M wasted `String` allocations.
4. **Second full-file hash pass in Rust.** `finish()` computes a
   byte-at-a-time FNV-1a over the entire source
   (`src/utrace_timeline.rs:425`), on top of the SHA-256 the Vite middleware
   already computed for the cache key.
5. **Transport waste on every query.** `utraceTimeline` posts the entire
   `File` per query (`web/src/lib/api.ts:300`). The middleware buffers the
   full body (`readBody`), writes the 259 MB temp copy *before* the cache
   lookup (`web/vite.parse-plugin.ts:464`), then hashes the body
   (`web/vite.parse-plugin.ts:382`). On a cache hit the temp copy is never
   used.
6. **Minor, real, not dominant**: the 1 M-record in-memory sort in `finish()`
   and the `sync_all()` fsync of the ~55 MB sidecar. Both are kept (see
   rejected findings).

## Product outcome

- Opening a capture through the progressive dashboard leaves a warm `.utix`
  behind: the first timeline query is a cache hit, not a 17.8 s rebuild.
- Timeline pans/zooms/filters after the first query send a small token
  request, not a 259 MB body.
- The standalone `utrace timeline index` command (still used for cold cache
  rebuilds and CLI users) gets several times faster by decoding only what the
  index needs and by not allocating names past the retention cap.
- Batch dashboard JSON, `.utix` query results, and all existing caps and
  truncation flags remain semantically unchanged.

## Non-goals

- Raising `DEFAULT_MAX_INDEXED_INTERVALS` or making the index unbounded.
- Making `.utix` build/query WASM-compatible (stays native-only per plan 010).
- Changing `.utix` query semantics, ordering, or the binary-search layout.
- Live/streaming timeline updates while a capture is still decoding
  (plan 011's remit).

## Phase 1 — Make the standalone index build cheap (Rust only)

No protocol or web changes. Target: cut most of the 17.8 s.

### 1.1 Lazy interval construction past the sink's appetite

1. Extend `CpuTimelineSink` with a cheap pre-check so the decode loop never
   builds what the sink will drop. Suggested shape:

   ```rust
   pub(crate) trait CpuTimelineSink {
       /// Cheap scalar bookkeeping for every interval (totals, cycle bounds).
       fn note(&mut self, start_cycle: u64, end_cycle: u64, active_frame: Option<u32>) -> SinkAppetite;
       /// Called only when `note` returned `SinkAppetite::WantsRecord`.
       fn record(&mut self, interval: CpuTimelineInterval, active_frame: Option<u32>);
   }
   ```

   `CpuTimelineIndexBuilder::note` updates `total_interval_count` and
   begin/end cycles and returns `WantsRecord` only while
   `records.len() < max_intervals`. `CpuTimelineCollector` (the frame-scoped
   dashboard collector) keeps its current filtering semantics: derive its
   `note` answer from the same active-frame/cap checks its `record` performs
   today. The exact split is the executor's choice; the invariant is that
   `note` must be allocation-free and that `note` + `record` together produce
   byte-identical sink state to the current single `record`.
2. In the decode hot path (`src/utrace.rs:8788` plain-spec arm and the
   metadata arm at `:8833`), call `note` first; only on `WantsRecord` clone
   the name, resolve `rendered_name`, compute `duration_seconds`, and build
   the `CpuTimelineInterval`. `active_frame` resolution must stay in the
   cheap path only if the frame-scoped collector needs it to answer `note`;
   otherwise move it inside the `WantsRecord` branch too.
3. Tests:
   - Cap-1 builder fed N intervals: `total_interval_count`, `begin_cycle`,
     `end_cycle`, `truncated` identical to before.
   - Fixture (`UTRACE_FIXTURE`) index build: resulting `.utix` byte-identical
     to the pre-change build (same input, same cap).
   - Frame-scoped dashboard timeline (`--frame N --timeline-limit K`) JSON
     unchanged on the fixture.

**Stop if** making `note` cheap requires duplicating frame-attribution logic
in a way that can drift from `record`'s. Keep one source of truth even if it
means `note` takes one more scalar argument.

### 1.2 Decode only what the index needs

1. Add an internal event-filter mode to the dashboard event pass, e.g.
   `DashboardDecodeScope::{Full, CpuTimelineOnly}` threaded into
   `read_dashboard_events`. In `CpuTimelineOnly`, skip — before any provider
   state is touched — every event that is not a data dependency of the CPU
   timeline sink path.
2. Derive the required event set by tracing the sink path's inputs, then
   lock it in with the equivalence test (do not trust the list below without
   the test). Expected dependencies: event registry declarations, `$Trace`
   prologue/thread info, `CpuProfiler` spec/metadata declarations and event
   batches, frame markers, and the metadata-stack events that feed
   `active_frame`/`rendered_name`. Everything else (GPU, counters, memory,
   callstacks, tasks, logging, CSV, platform-file, Slate, bookmarks,
   unmodeled-event accounting) must not allocate in this mode.
3. `build_cpu_timeline_index` switches to `CpuTimelineOnly`. The public
   dashboard paths keep `Full` and must be bit-for-bit unaffected.
4. Equivalence test: on the fixture (and the tiny synthetic corpus), the
   `.utix` produced under `CpuTimelineOnly` is byte-identical to one produced
   under `Full` with the same sink.

**Stop if** the dispatch loop cannot skip an event family without changing
CPU scope decoding (e.g. a shared cursor or shared state dependency). Report
the coupling instead of decoding that family "just in case".

### 1.3 Stop re-hashing the source in Rust

1. Replace the `source: &[u8]` parameter of
   `CpuTimelineIndexBuilder::finish` with a `SourceIdentity { source_bytes: u64,
   fingerprint: u64 }` provided by the caller.
2. For the whole-buffer CLI path, keep the existing FNV-1a value but compute
   it once, streaming, while the input is read (or accept `--source-fingerprint`
   when a caller already knows it). FNV-1a folds per-chunk trivially, which
   phase 2 needs anyway since the progressive session never retains the full
   capture.
3. The `.utix` header bytes must not change for the same input; this is a
   pure who-computes-it refactor.

### 1.4 Measure

Re-run the exact production command on the 259 MB capture before and after
1.1–1.3 and record wall time in this plan's PR description. Expected shape:
interval decode dominated by batch decoding itself, with allocation and
foreign-provider time gone.

## Phase 2 — Build the index during the dashboard parse (one decode, not two)

The progressive session (`src/utrace_session.rs`) already frames packets and
LZ4-decodes incrementally, then runs provider projection once at `finish()`.
Attach the index builder to that single projection.

1. Add an optional timeline-index request to the session/dashboard entry
   points: `dashboard_from_decoded` (and the session `finish` /
   `finish_with_inventory` wrappers) accept an optional
   `TimelineIndexRequest { output: PathBuf, max_intervals: usize, source: SourceIdentity }`
   and feed a `CpuTimelineIndexBuilder` alongside the existing frame-scoped
   collector during the same `read_dashboard_events` pass. Both sinks must be
   able to observe the same interval; if the current
   `Option<&mut dyn CpuTimelineSink>` plumbing cannot carry two sinks, add a
   small fan-out sink rather than a second decode pass.
2. Maintain the source fingerprint incrementally in
   `ProgressiveDashboardSession::push_chunk` (FNV-1a over raw input bytes,
   fold per chunk) so `SourceIdentity` is available at `finish()` without the
   capture buffer.
3. CLI: add `--timeline-index-output <path>` (and `--timeline-index-max-intervals`,
   default 1,000,000) to `dashboard-progress`. On success, emit the resulting
   `CpuTimelineIndexInfo` and index path in the terminal `complete` event
   (extend the progressive DTOs in `src/utrace_progress.rs`; bump the
   documented protocol in `docs/progressive-utrace-protocol.md`). Index-write
   failure must not fail the dashboard: report it as a warning field, never a
   `failed` event.
4. Vite middleware (`handleProgress`): compute the SHA-256 incrementally
   while forwarding upload chunks to the child (streaming `createHash`
   update — no extra buffering). Spawn the CLI with a temp index path inside
   `timelineCacheDir`; after the child succeeds, rename to
   `<sha256>.utix` and run the existing pruner. Forward the index info to the
   browser via the `complete` event so the client learns the capture token
   (phase 3).
5. Make it opt-in from the client (`?timeline_index=1`) so non-browser CLI
   consumers and tests of the bare progressive path see zero change.
6. Tests:
   - Session-built `.utix` equals the standalone `utrace timeline index`
     output byte-for-byte on the fixture and tiny corpus.
   - Dashboard JSON with and without the index request is identical.
   - Chunk-boundary invariance (reuse the adversarial chunk tests) for the
     incremental fingerprint.
   - Progressive CLI test in `tests/cli.rs`: `complete` event carries index
     info; a failing index write (unwritable path) still completes the
     dashboard with the warning surfaced.

**Stop if** feeding the second sink measurably regresses the plain
progressive dashboard decode (>2–3% on the fixture benchmark) even when no
index was requested — the request-off path must be a true no-op.

## Phase 3 — Stop re-uploading the capture per timeline query

1. Add a token query path to `/api/utrace/timeline`: when the request carries
   `?src=<sha256>` and an empty body, skip `readBody`/temp-write/hash, look up
   `<sha256>.utix` directly, `utimes`-touch it, and run the query. If the
   sidecar is missing (pruned/cold), respond `409` with a typed
   `{ error: "index_missing" }` so the client can fall back.
2. Reorder the existing full-body path: hash first, check the cache, and only
   write the temp capture copy when an index build is actually required. On a
   cache hit the 259 MB temp write disappears.
3. Client (`web/src/lib/api.ts`, `web/src/lib/parser-backend.ts`, the
   `Utrace` route): keep the capture's `source_hash` (from phase 2's
   `complete` event, or from the first full-body timeline response — have the
   server return the hash in `X-Ue-Parse-Timing` or the JSON envelope) next to
   the loaded file. `utraceTimeline` prefers the token path and falls back to
   the full-body path exactly once on `index_missing`, then retries the token.
4. Timing fields: extend `ParsePhaseTiming` so token-path queries report
   `upload_bytes: 0` honestly and cache hits report `write_temp_ms: 0`.
5. Tests: middleware-level (or a small integration harness) covering token
   hit, token miss → fallback → rebuild → token hit, and pruner eviction
   between queries. UI: panning/searching the timeline issues no full-file
   uploads after the first query (assert via the timing header in the
   existing timing surface).

## Phase 4 — Deferred / rejected findings

- **Incremental record writing instead of collect-and-sort**: rejected.
  Records must be globally sorted by `start_cycle` for the binary search and
  the `prefix_end_cycle` accelerator is a sequential pass over sorted order,
  while intervals arrive in per-thread end-order. An external sort buys
  nothing at a 1 M cap where the in-memory sort is ~100–200 ms. Revisit only
  if the cap is ever raised past ~10 M.
- **Dropping `sync_all()`**: rejected. The sidecar is a cache keyed by
  content hash; a torn write surviving a crash would be served forever. The
  fsync + atomic rename stays.
- **Skipping interval decode entirely past the cap**: impossible as stated —
  `total_interval_count` and the capture cycle bounds in the header require
  observing every interval. Phase 1.1's `note` path is the correct floor.
- **SHA-256 in Rust to unify the two hashes**: unnecessary once phase 1.3
  makes the FNV fingerprint free (single streamed pass) and phase 3 keys the
  transport on the middleware's SHA-256. Two hashes, each computed once,
  is fine.
- **String dictionary double-storage** (`Vec<String>` + `BTreeMap` keys, plus
  the extra clone in `record`): real but small next to the record vector.
  Optional cleanup if the executor is already touching `intern`; not a gate.

## Test and verification matrix

| Contract | Test |
|---|---|
| Index equivalence | `.utix` byte-identical across: pre/post phase 1, `Full` vs `CpuTimelineOnly`, standalone vs session-built. |
| Truncation accounting | Cap-1 and cap-N builders report identical totals/bounds/truncated before and after lazy construction. |
| Dashboard invariance | Batch and progressive dashboard JSON unchanged with the index request off; identical with it on. |
| Chunked fingerprint | Adversarial chunk boundaries produce the same `SourceIdentity` as whole-buffer. |
| CLI surface | `tests/cli.rs` covers new flags, `complete`-event index info, and index-failure-as-warning. |
| Transport | Token hit, miss→fallback→retry, pruner eviction; cache hit performs no temp write. |
| Query behavior | `query_cpu_timeline_index` untouched: existing timeline query tests keep passing unmodified. |

Run:

```text
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo check --lib --target wasm32-unknown-unknown --no-default-features --features wasm
cd web && npm run build
UTRACE_REQUIRE_FIXTURE=1 cargo test --test utrace_fixture --features utrace -- --ignored
```

Manually verify on the 259 MB capture and record in the PR:

1. Standalone `utrace timeline index` wall time before/after phase 1
   (baseline: ~17.8 s).
2. Progressive load with `?timeline_index=1`: added wall time over a plain
   progressive load, and that the first timeline query is a cache hit.
3. A timeline pan after the first query: request size and `Server-Timing`
   show no upload and no temp write.

## Acceptance criteria

- First timeline query after a progressive capture load performs no trace
  reparse (cache hit on the session-built `.utix`).
- A cold standalone index build no longer constructs non-CPU providers and no
  longer allocates interval names past the retention cap.
- Timeline queries after the first send no capture body; the fallback path
  still recovers from cache eviction without user action.
- `.utix` bytes, query results, dashboard JSON, caps, and truncation flags
  are unchanged for identical inputs, proven by equivalence tests.
- No new unbounded allocation; the index request adds bounded, opt-in memory
  (~the existing 1 M-record builder) to the session finish only when asked.
