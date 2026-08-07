# UTrace real-capture performance experiments — 2026-08-07

This is the durable record of the indexing and aggregation experiments run on
the real capture `20260712_233058_3BF790.utrace`. The capture is 259,630,176
bytes and contains 33,376,033 completed CPU scopes across 76 trace threads.

The conclusion is deliberately modest: the retained parallel aggregation path
is correct and measurable, but it is only about a one-second improvement on an
approximately thirteen-second operation. It does not close the gap with Unreal
Insights by itself.

## Measurement method and caveats

- Machine: the Windows/Ryzen development host used for this repository.
- Native probe: release `utrace_memory_probe <trace> monotonic`.
- Browser-equivalent worker count: `RAYON_NUM_THREADS=8`; the web worker caps
  its Rayon pool at eight workers.
- Peak memory: `PeakWorkingSet64`, sampled every 100 ms. The sampler and cold
  filesystem/cache state noticeably perturb wall time, so those runs are used
  for memory, not the primary timing comparison.
- Dashboard equivalence: terminal `dashboard-progress` dashboard and inventory
  objects were compared structurally with `jq`.
- The user's approximately five-second Unreal Insights timing was informal,
  not an instrumented apples-to-apples benchmark.

Wall-clock numbers varied by roughly a second between runs. Treat sub-200 ms
differences as noise unless a controlled benchmark says otherwise.

## Representative pre-parallel breakdown

This baseline includes the exact eager monotonic timeline from `c7f2aba` and
the revert of prepared event boundaries in `0a2fcac`:

| Stage | Time | What it includes |
|---|---:|---|
| Progressive push | ~5.25 s | File reads/chunks, packet framing and decompression, retained thread streams, frame updates, and eager exact CPU timeline construction |
| Finish | ~7.31 s | Registry/important-event reads, global Protocol 5 serial dispatch, provider decoding, CPU dashboard aggregation, reductions, and JSON-facing dashboard construction |
| First exact timeline query | ~0.45–0.60 ms | Read from the already-built Rust monotonic index |
| Total Rust decode | ~12.57 s | Push plus finish; excludes UI rendering |

The expensive “indexing” was therefore not JavaScript. Both the eager timeline
index and final dashboard aggregation were Rust/WASM work. JavaScript mainly
scheduled the worker, transferred chunks, parsed returned JSON, and rendered
the result.

## Experiments

### 1. Exact paged/columnar CPU timeline

Commits: `3c55621` and `c7f2aba`.

The first implementation added a paged monotonic CPU timeline. The second moved
its construction into progressive ingest so the UI no longer triggers a second
timeline build when it asks for data.

On the real capture the retained index reports:

- 66,752,066 begin/end entries for 33,376,033 completed scopes.
- 1,075 pages across 76 threads.
- 222,348,598 allocated bytes in total.
- 144,972,056 bytes in event columns, 344,992 bytes in page metadata, and
  77,031,550 bytes in timer/metadata catalogs.
- Approximately five allocated column bytes per scope begin.
- A representative 1,292-interval frame query completes in about 0.5 ms.

This was a product win—exact queries became effectively instant and the index
is built eagerly—but it mostly moved work into `push`; it did not make the full
parse approach Unreal's time.

### 2. Reuse every progressive event boundary

Commits: `2aac013`, reverted by `0a2fcac`.

This experiment retained a compact column of every normal event's wire length
during progressive ingest. Dispatch could then skip reparsing variable event
and auxiliary boundaries at finish. Common lengths used `u16`; exceptional
lengths used sparse `u32` overflow index/value columns.

It was reverted because:

- It moved extra work and allocation into every progressive event.
- Finish improved only slightly while end-to-end time was flat or slightly
  worse in the observed real-trace runs.
- It retained boundaries for all event families even though CPU batches were
  the target bottleneck.
- The added transport/dispatch complexity was too large for the measured win.

The exact per-run console output from this intermediate experiment was not
retained. The commits preserve the implementation, and the observed decision
was recorded at the time as “kinda worse ... for a small win.” No more precise
number should be inferred.

This does not prove boundary columns are always wrong. It shows that eagerly
indexing every event boundary is the wrong granularity here. A CPU-batch-only
offset column may still be worth testing if it replaces the parallel rescan.

### 3. Reaggregate the eager timeline columns

This was investigated but not implemented. The existing monotonic timeline is
excellent for queries, but it is not a lossless substitute for raw CPU batch
aggregation:

- Coroutine suspension can split one logical scope into timeline segments.
- Restored `MetadataStack` attribution is not represented in those columns.
- Metadata IDs are introduced in global serial order; using the final catalog
  would incorrectly resolve scopes that occurred before their metadata record.
- The dashboard keeps globally ordered bounded metadata samples.

Adding all missing semantic columns would duplicate a substantial amount of
state and complicate the very compact query index. Raw batch decoding remained
the source of truth for the dashboard.

### 4. Assume physical byte order is serial order

This is valid only within one trace thread. Protocol 5 appends each thread's
events to its own byte stream, so its local CPU state can be replayed in physical
order. Events across threads still require the 24-bit serial merge used by
`FProtocol5Stage`; concatenated file or map iteration order is not a global
semantic order.

That distinction enabled the retained parallel design: sequential per-thread
CPU replay plus global metadata/order preparation.

### 5. Parallel per-thread CPU aggregation

Commit: `f6804f1`.

The global serial pass continues to decode metadata and non-CPU providers. It
records compact `(metadata_generation, batch_order)` contexts for CPU batches.
Each trace thread is then replayed sequentially, while different threads run in
parallel with Rayon. Workers own their maps and stacks; the main thread performs
one deterministic reduction. There is no shared per-scope insertion.

The two context fields are consumed together for every batch, so an eight-byte
row is preferable to two separate columns here. The high-volume begin/end
timeline remains columnar. This is the row/column tradeoff used in the retained
implementation, rather than applying a columnar shape indiscriminately.

Correctness constraints:

- Final metadata is gated by its introduction generation.
- Metadata samples carry batch/local order keys and are sorted before applying
  the existing 40-sample cap.
- Totals are reduced with saturating additions; frame bounds use min/max.
- Metadata ID redefinitions, `MetadataStack` save/restore traffic, legacy CPU
  timeline sinks, and unsupported compact-index sizes use the serial fallback.

Three baseline and three initial parallel runs:

| Variant | Push (ms) | Finish (ms) | Total (ms) |
|---|---:|---:|---:|
| Serial | 5,330 | 7,505 | 12,836 |
| Serial | 5,967 | 7,339 | 13,307 |
| Serial | 5,257 | 7,296 | 12,553 |
| Parallel | 5,213 | 6,194 | 11,408 |
| Parallel | 5,249 | 6,239 | 11,489 |
| Parallel | 5,204 | 6,224 | 11,428 |

Median improvement: finish 7.34 s to 6.22 s; total 12.84 s to 11.49 s. That
is about 1.35 s or 10.5% end to end in this run set.

Worker-count sweep:

| Rayon workers | Finish (ms) | Total (ms) |
|---:|---:|---:|
| 1 | 8,059 | 13,430 |
| 2 | 8,176 | 13,685 |
| 4 | 7,672 | 12,999 |
| 8 | 6,395 | 11,672 |
| 16 | 6,251 | 11,582 |

Eight workers are close to the knee. Sixteen workers gained only ~144 ms in
this sweep, so the browser cap remains eight rather than consuming twice as
many workers.

Peak-working-set runs measured approximately 1,683 MiB for the serial path and
1,749 MiB for the eight-worker path: about 66 MiB more. The instrumented
eight-worker run finished in 6,427 ms. The memory increase is accepted but is
not free, especially near WASM's configured 4 GiB maximum.

The final serial and parallel terminal dashboard and inventory objects compared
exactly equal on the capture. Scope output also gained a stable `spec_id`
tie-breaker because parallel map insertion order exposed an existing unstable
zero-duration tie.

### 6. Shared-memory browser WASM

The first atomics build with `nightly-2026-07-15` failed during
`wasm-bindgen-rayon` preparation because `__heap_base` could not be found. The
pinned/tested `nightly-2025-11-15`, `rust-src`, `-Z build-std`, atomics,
bulk-memory, imported shared memory, and TLS exports produced a working build.

The retained web changes:

- Build with `utrace-wasm-threads` and `wasm-bindgen-rayon`.
- Initialize up to eight Rayon workers inside the existing WASM worker.
- Serve COOP `same-origin` and COEP `require-corp` in Vite dev/preview.
- Configure a 4 GiB maximum shared memory because this trace already peaks near
  1.75 GiB natively.
- Pin the special build command in `web/scripts/build-wasm.mjs` and document the
  required toolchain.

Production compilation and Vite bundling succeeded; the bundle contains the
Rayon worker helper. The in-app automation preview could not complete a runtime
test because its wrapper was not `crossOriginIsolated`, and transferring the
`SharedArrayBuffer` failed there. A normal top-level browser remains the needed
runtime confirmation. This limitation must not be reported as an end-to-end
browser benchmark.

## Unreal source comparison

The local UE 5.7 source was inspected under
`C:\Program Files\Epic Games\UE_5.7\Engine\Source\Developer`:

- `TraceAnalysis` performs Protocol 5 per-thread parsing and global serial
  dispatch (`FProtocol5Stage`, `DispatchNormalEvents`, serial-gap handling).
- `CpuProfilerTraceAnalysis` decodes each CPU buffer with thread-local stack and
  cycle state.
- `TraceServices` uses monotonic timelines and paged/slab-style storage for
  efficient append and enumeration.

The retained design copies the useful invariant—not the implementation:
within-thread CPU order is sequential, while different CPU threads can be
aggregated independently after global metadata/order preparation. Unreal's
roughly five-second UI timing is not evidence that our remaining gap is
JavaScript; its native ingestion, analyzers, storage, and UI integration differ
as a whole.

## Verification completed

- `cargo fmt --check`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `cargo test --all-targets --all-features`
- 128 library tests plus CLI/integration suites passed.
- 24 web tests passed.
- Threaded release WASM and the Vite production bundle built successfully.
- Full real-capture dashboard and inventory equality passed.

## Current assessment

The parallel path is worth retaining, but it is a marginal optimization rather
than a breakthrough. It trades roughly 66 MiB of peak memory and a substantial
amount of implementation/build complexity for about one to 1.35 seconds on
this capture. The next experiment should first obtain a real browser CPU profile
and stage timings; otherwise it is too easy to optimize another plausible but
secondary parser seam.
