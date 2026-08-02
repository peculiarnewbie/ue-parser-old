# Plan 014: Hot-path parser performance — allocation, map, and dispatch gaps

> **Executor instructions**: Phases are ordered by expected impact and are
> independently shippable. Run the full verification suite after each phase
> before starting the next. All output (dashboard JSON, `.utix` bytes, CLI
> exit codes) must be bit-for-bit identical to pre-change for the same input.
> Do not touch `.utix` on-disk format; do not raise any caps or limits. If a
> stop condition occurs, stop and report it.

## Status

- **Priority**: P1 performance (decode throughput, streaming latency)
- **Effort**: M/L
- **Risk**: LOW/MEDIUM — no output semantics change; risk is regressions in
  edge-case coverage or subtle ordering dependencies on the dispatch path
- **Depends on**: Plan 012 phase 1.1 landed (note()/record() split already in
  tree); independent of plans 011/013
- **Planned**: 2026-07-13

## Background — how this was found

Deep comparison of our parse pipeline against Unreal Engine 5.7's
`TraceAnalysis` (`Engine.cpp`, `TidPacketTransport`, `StreamReader`) and
`TraceServices` (`CpuProfilerTraceAnalysis`, `TMonotonicTimeline`,
`PagedArray`, `FSlabAllocator`, `FStringStore`) source at
`C:\Program Files\Epic Games\UE_5.7\Engine\Source\Developer\`. The comparison
was run against a 259 MB capture that decodes 33,376,033 CPU intervals.

## Verified causes (each confirmed in current source)

### C1 — streaming path rebuilds the registry map per event
`src/utrace_session.rs:565-569`. Inside the per-event `loop` of
`decode_normal_frame_events`, a fresh `BTreeMap<u16, &EventTypeInfo>` is
collected from `bootstrap_registry` every iteration:
```rust
let registry = self.bootstrap_registry.iter()
    .map(|(uid, event)| (*uid, event))
    .collect::<BTreeMap<_, _>>();
```
This allocates and inserts O(registry size) entries for every event parsed
during progressive streaming. The fix is hoisting the collection outside the
loop — it does not change between iterations.

### C2 — hot batch-decode path uses BTreeMap for all per-interval bookkeeping
~40 `BTreeMap` instances in `read_dashboard_events` (`utrace.rs:3637-3680`).
Per-interval (33M calls each on a large capture):
- `scope_totals.entry(spec_id)` — `utrace.rs:9027`
- `thread_scope_totals.entry(spec_id)` — `utrace.rs:9031`
- `frame_scope_totals.entry(frame_number).or_default(); frame_totals.entry(spec_id)` — `utrace.rs:9287` (nested, 2 BTreeMap traversals)
- `frame_cycle_bounds.entry(frame_number)` — `utrace.rs:9291`

`spec_id` (`u32`) and `frame_number` (`u32`) are small dense integers assigned
sequentially. `thread_id` (`u16`) is small and sparse but bounded by thread
count. `BTreeMap` for these is cache-hostile pointer chasing at every insert;
`FxHashMap` or `Vec`-by-id cuts this. Epic uses array indexing by UID for all
of these (O(1), no allocation, cache-friendly).

No fast-hash crate exists in `Cargo.toml` today.

### C3 — every normal event is heap-copied, twice
`src/utrace_dispatch.rs:139,169,181`. `parse_thread_normal_events` copies each
event's payload bytes into an owned `Vec<u8>`:
```rust
let mut data = stream[parsed.data_start..parsed.data_end].to_vec();   // copy 1
// aux blobs:
let mut aux_bytes = stream[aux.offset..aux.total_end].to_vec();       // copy 2
data.extend_from_slice(&aux_bytes);
```
Then all events from all threads are merged into a second
`Vec<DispatchedNormalEvent>` (`:338`), so both copies live in memory
simultaneously before analysis starts. Epic's `FEventDataInfo` points directly
into the decompressed thread buffer; aux blobs are pointers too, defragmented
only if the analyzer actually reads a fragmented field (`Engine.cpp:799,
4932`). Since our thread streams are `Vec<u8>` that outlive the dispatch phase,
`DispatchedNormalEvent` can carry `(thread_id, data_range: Range<usize>,
aux_ranges: SmallVec<Range<usize>>)` referencing those buffers by offset.

Same issue in the streaming path: `utrace_session.rs:576,586`.

### C4 — `active_frame_number` re-parses a string per interval
`src/utrace.rs:6547`. Called at `:9056`, `:9104`, and `:9284` — up to 3× per
interval:
```rust
self.active.iter().rev().find_map(|entry| {
    metadata.get(&entry.metadata_id)            // BTreeMap lookup
        .and_then(|r| r.rendered_name.as_deref())
        .and_then(parse_rendered_frame_number)  // string parse every call
})
```
The frame number only changes when the metadata stack changes. The resolved
`Option<u32>` should be cached in the thread state and invalidated on
push/pop of the metadata stack, making per-interval calls a single field read.

### C5 — LZ4 output buffer allocated fresh per packet
`src/utrace.rs:3044`, `src/utrace_session.rs:362`:
```rust
let mut decoded = vec![0u8; usize::from(decoded_size)];
```
One `Vec` allocated per compressed packet. Epic decompresses directly into the
destination buffer: `Thread->Buffer.Append(DecodedSize)` reserves space in the
growable per-thread stream, then `Decode` writes into it — one allocation amortized
across many packets (`TidPacketTransport.cpp:90`). We can do the same:
`Vec::extend_with` (or `resize` + `lz4_flex::decompress_into`) into the
existing per-thread stream buffer eliminates the intermediate alloc.

### C6 — no inlining on hot primitives, no release-profile tuning
No `#[inline]` anywhere in the crate. `VarintReader::read_u64`
(`utrace.rs:9787`) — the innermost primitive of the 33M-interval batch decode
— does a bounds-checked byte loop per varint without any inlining hint. No
`[profile.release]` section in `Cargo.toml` (`lto`, `codegen-units`).

### C7 — per-event string-compare dispatch chain (no UID jump table)
`utrace.rs:3984-4138`. Every dispatched normal event is routed by a 20+ arm
`if`/`else if` chain of `(logger.as_str(), event.as_str())` comparisons.
The UID→`EventTypeInfo` map is already built at registration time; adding a
`UID → EventKind` enum lookup at registration turns hot-path dispatch into a
`match` on a `u8` / `u16` with no string comparison per event. Epic resolves
routes once per type declaration, memoizes `TypeToRoute[uid]` with a
biased-by-one sentinel for "no subscribers", and per-event routing is a single
array index (`Engine.cpp:1521, 1628`).

### C8 — `dispatched_events` walked twice
`utrace.rs:3952`, `:3969`. After merge-sorting, the full `Vec<DispatchedNormalEvent>`
is scanned once for `CpuProfiler.Metadata` events, then again for everything
else. One ordered pass handles both.

### C9 — timeline string dictionary double-storage
`utrace_timeline.rs:496`. `intern()` keeps `Vec<String>` and
`BTreeMap<String, u32>` with a `clone` into each:
```rust
self.strings.push(value.clone());
self.string_ids.insert(value, id);
```
Acknowledged in plan 012 as "real but small, optional". The fix is a
`HashMap<Box<str>, u32>` keyed on interned slices, or using
`indexmap::IndexSet`. Not a gate on earlier phases.

## Non-goals

- Changing any output bytes, caps, truncation flags, or `.utix` format.
- Multi-threading the decode (Epic's ingest is also single-threaded per trace;
  parallelism is query-time only — see TMonotonicTimeline async enumeration).
- Adopting Epic's paged/slab timeline storage (`TMonotonicTimeline`) — it is
  the right long-term direction if the 1M-interval cap ever becomes a product
  limit, but is not in scope here.
- WASM or NEON SIMD; these are follow-ups.

## Phase 1 — Trivial wins (no behavior change, no new dependencies)

### 1.1 Hoist streaming registry build out of the event loop
In `decode_normal_frame_events` (`utrace_session.rs:555`), move the
`bootstrap_registry.iter().collect::<BTreeMap<_,_>>()` to before the `loop`
block. The registry is append-only and is never modified by the decode path.

Test: existing progressive CLI tests and fixture tests must produce identical
output.

**Stop if** any code path inside the loop writes to `bootstrap_registry` —
confirm with a grep before moving.

### 1.2 Release-profile tuning and inline hints
In `Cargo.toml`, add:
```toml
[profile.release]
lto = "thin"
codegen-units = 1
```
Add `#[inline]` to `VarintReader::read_u64` (`utrace.rs:9787`),
`VarintReader::read_u32`, `Reader::read_u8`/`read_u16`/`read_u32`/`read_u64`
(`archive.rs` scalar reads), and any other single-expression helpers that the
compiler cannot see across crate boundaries (check with `#[inline(always)]`
on the batch varint path if LTO alone doesn't close the gap).

Check: `cargo check --lib --target wasm32-unknown-unknown --no-default-features --features wasm` still passes (`lto = "thin"` is safe for WASM).

### 1.3 LZ4 decompress into the stream buffer directly
Replace the fresh-`Vec` allocation in both paths:
- `utrace.rs:3044`: `let mut decoded = vec![0u8; ...]` → `stream.resize(stream.len() + decoded_size, 0); lz4_flex::decompress_into(data, &mut stream[start..])?`
- `utrace_session.rs:362`: same pattern.

Ensure `lz4_flex::decompress_into` is available (it already is per `Cargo.toml`).
Equivalence test: byte-identical decompressed streams vs. current.

### 1.4 Single-pass `dispatched_events` dispatch
Merge the two walks at `utrace.rs:3952` and `:3969` into one ordered loop.
Route on `EventKind` from a pre-built `UID → EventKind` array (see phase 2 for
the table; for now, the `(logger, event)` string-compare chain is fine — just
run it once).

Test: dashboard JSON byte-identical on fixture.

## Phase 2 — UID-based dispatch table (no new dependencies)

Build a `Vec<EventKind>` indexed by UID at event-registry time, where
`EventKind` is an `enum` (or `u8` discriminant) covering every handled
`(logger, event)` pair plus an `Unknown` variant. Replace the 20+ arm
string-compare chain in `read_dashboard_events` with a `match event_kind[uid]`
lookup.

```rust
// At registration (read_event_registry):
event_kinds.resize(uid + 1, EventKind::Unknown);
event_kinds[uid] = derive_event_kind(&logger, &event);

// In the dispatch loop:
match event_kind.get(uid).copied().unwrap_or(EventKind::Unknown) {
    EventKind::CpuProfilerEventBatchV3 => { ... }
    EventKind::FrameMarker => { ... }
    ...
    EventKind::Unknown => { unmodeled_events.entry(...).or_insert(0) += 1; }
}
```

`derive_event_kind` does the string comparisons once at registration.
Unmodeled-event accounting no longer needs `.entry((logger.clone(),
event.clone()))` per event — accumulate into `HashMap<u16, u64>` keyed by UID,
decode to names only at finalize.

Tests:
- Dashboard JSON identical on fixture and tiny corpus.
- Unmodeled-event counts identical.

## Phase 3 — Fast maps on hot per-interval collectors

Add `rustc-hash` (already a zero-footprint crate, `no_std`-compatible):
```toml
rustc-hash = "2"
```

Replace `BTreeMap` with `FxHashMap` / `FxHashSet` for every map indexed by a
scalar key that is touched per-interval:
- `scope_totals: FxHashMap<u32, (u64, u64)>` (was `BTreeMap<u32, ...>`)
- `thread_scope_totals: FxHashMap<u16, FxHashMap<u32, (u64, u64)>>`
- `frame_scope_totals: FxHashMap<u32, FxHashMap<u32, (u64, u64)>>`
- `frame_cycle_bounds: FxHashMap<u32, (u64, u64)>`
- `cpu_batch_thread_states: FxHashMap<u16, CpuBatchThreadState>`
- `metadata_stack_contexts: FxHashMap<u16, ...>`
- `memory.outstanding: FxHashMap<u64, ...>` (was plain `HashMap`)

Any map whose output is serialised in sorted key order must be sorted at
finalize time, not per-insert — these maps are already aggregated and emitted
in a separate step, so this is mechanical.

Keep `BTreeMap` for maps that are: (a) iterated in key order during output
serialization and (b) not on the per-interval hot path (e.g. the event
registry itself).

Tests: dashboard JSON identical (output ordering must not change — validate
with the fixture). Clippy clean.

**Stop if** `FxHashMap` changes any observable output ordering. Fix by sorting
at finalize rather than weakening to `BTreeMap`.

## Phase 4 — Cache `active_frame_number` on metadata stack change

In each `CpuBatchThreadState`, add a `cached_active_frame: Option<u32>` field.
Set it on push/pop of the metadata stack (the same arms that modify `active`).
Replace the `active_frame_number()` call at `utrace.rs:9056`, `:9104`,
`:9284` with a read of `state.cached_active_frame`.

The cache must be invalidated (set to `None`, then eagerly re-resolved) on any
metadata stack modification. An `Option<u32>` where `None` means "not yet
computed for this stack state" avoids re-running the scan; the scan itself
(`active.iter().rev().find_map(...)`) runs at most once per stack push/pop, not
per interval.

Tests: dashboard frame-correlation output identical on fixture.

**Stop if** the cached value ever differs from the live-computed value. Add a
debug assertion `debug_assert_eq!(cached, live_computed)` gated on
`#[cfg(debug_assertions)]` and run the fixture in debug mode to confirm.

## Phase 5 — Zero-copy dispatch events (remove per-event `to_vec`)

Replace the owned `Vec<u8>` in `ThreadEvent` and `DispatchedNormalEvent` with
a slice reference tied to the per-thread stream lifetime, or — if lifetime
threading through the merge sort is painful — with `(thread_id: u16,
start: u32, end: u32)` index pairs into the `streams: BTreeMap<u16, Vec<u8>>`
that already outlive the dispatch phase.

Suggested shape:
```rust
struct DispatchedNormalEvent<'s> {
    uid: u16,
    serial: u32,
    scope_cycle: Option<u64>,
    data: &'s [u8],          // points into streams[thread_id][start..end]
}
```

If lifetime annotation is too viral, use the index form:
```rust
struct DispatchedNormalEvent {
    uid: u16,
    serial: u32,
    scope_cycle: Option<u64>,
    thread_id: u16,
    data_start: u32,
    data_end: u32,
}
```
and re-slice `&streams[&thread_id][data_start as usize..data_end as usize]`
in the analysis loop.

Aux blobs: store one additional `(thread_id, offset, end)` per aux blob
instead of extending into `data`. The aux count per event is bounded by 64,000
but in practice almost always 0–2; `SmallVec<[AuxRef; 2]>` keeps this
allocation-free for the common case.

Apply the same fix to the streaming path (`utrace_session.rs:576, 586`).

Tests: dashboard JSON identical. Valgrind / ASAN (if available) to catch
dangling refs. At minimum, the existing fixture suite plus a stress run with
adversarial chunk boundaries.

**Stop if** the borrow lifetime cannot be threaded through the merge-sort
(`BinaryHeap<Reverse<...>>` pops) without `unsafe`. Use the index form instead
— it is safe, almost as fast, and still eliminates the copies.

## Phase 6 — Timeline string dictionary (optional cleanup)

Replace the double-storage `(Vec<String>, BTreeMap<String, u32>)` in
`CpuTimelineIndexBuilder` (`utrace_timeline.rs:331–332`) with a single
`IndexMap<Box<str>, u32>` (from `indexmap`, which is already a transitive
dep) or an `FxHashMap<Box<str>, u32>` with the `Box<str>` stored by value
(interning by pointer equality is not needed here because keys are hashed by
content anyway). Removes the extra `clone` at `:501-502`.

This is a small cleanup, not a performance gate. Do it when already touching
`utrace_timeline.rs` for another reason.

## Measure

There is currently no automated benchmark. Before starting phase 1, add a
timed fixture run:

```rust
// benches/utrace_decode.rs  (criterion)
fn bench_dashboard(c: &mut Criterion) {
    let path = std::env::var("UTRACE_FIXTURE").expect("UTRACE_FIXTURE not set");
    let bytes = std::fs::read(&path).unwrap();
    c.bench_function("dashboard_from_bytes", |b| {
        b.iter(|| utrace::dashboard_from_bytes(black_box(&bytes), Default::default()).unwrap())
    });
}
```

Record wall time before phase 1 and after each subsequent phase on the 259 MB
capture. Report in the PR description. Expected shape after all phases:
- Phase 1 streaming hoist: measurable win on progressive decode latency.
- Phase 2 dispatch table: small but consistent win on every non-CPU event.
- Phase 3 FxHashMap: likely the largest single batch-decode improvement.
- Phase 4 frame cache: secondary to phase 3, visible with many metadata scopes.
- Phase 5 zero-copy: wins proportionally on traces with many large aux payloads.

## Verification (every phase)

```text
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo check --lib --target wasm32-unknown-unknown --no-default-features --features wasm
cd web && npm run build
UTRACE_REQUIRE_FIXTURE=1 cargo test --test utrace_fixture --features utrace -- --ignored
```

Output-equivalence assertion: on the `UTRACE_FIXTURE` capture, dashboard JSON
output is byte-identical before and after each phase. For phase 3 (map order),
serialize sorted if needed.

## Deferred / rejected

- **Multi-threaded decode**: Epic's analysis is also single-threaded per trace;
  parallelism is query-time enumeration, not ingest. Not in scope.
- **`TMonotonicTimeline`-style paged storage**: correct long-term direction for
  removing the 1M-interval cap and enabling zero-alloc timeline ingest; deferred
  until the cap is a product constraint.
- **SIMD varint decode**: the bottleneck is map contention (C2), not raw
  varint throughput. Revisit after phase 3 if the varint loop is still visible
  in profiles.
- **Incremental record writing for `.utix`**: already rejected in plan 012
  (requires global sort; in-memory sort at 1M cap is ~100–200 ms, acceptable).
- **`unsafe` pointer tricks**: not needed to achieve the zero-copy goal in
  phase 5; the index form is safe and nearly as fast.

## Acceptance criteria

- All existing tests pass, dashboard JSON byte-identical on identical inputs.
- Streaming registry rebuild is gone (C1): `decode_normal_frame_events` holds
  the registry outside its loop.
- Release builds use `lto = "thin"` and `codegen-units = 1` (C6).
- Per-interval `BTreeMap` traversals replaced with `FxHashMap` (C3): no
  `scope_totals`, `frame_scope_totals`, `thread_scope_totals`, or
  `frame_cycle_bounds` remain as `BTreeMap` in the hot interval path.
- `active_frame_number` is a cached field read per interval, not a per-interval
  scan + string parse (C4).
- LZ4 decompresses directly into the thread stream buffer (C5): no per-packet
  intermediate `Vec`.
- Dispatch is a `match` on a pre-built `EventKind` enum, not string comparison
  per event (C7); `dispatched_events` is walked once (C8).
- Benchmark on 259 MB capture recorded in PR.
