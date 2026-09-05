# UTrace opening and query continuation — 2026-09-06

This continues from the pushed `9f40c6a` checkpoint and its
[streaming results](utrace-performance-experiments-2026-09-06.md). Experiments
target complete capture loading and the existing bounded selected-frame view.

## Final alternating comparison

Five control/candidate pairs on 65 MB and three on 94 MB all favored the new
build. Each pair reverses order relative to the preceding pair, and every sample
starts a fresh Chromium process. No builds or native probes ran during sampling.

| Application milestone | Fresh checkpoint control | Final | Reduction |
| --- | ---: | ---: | ---: |
| 65 MB: file to dashboard | 1,572 ms | 1,341 ms | 14.7% |
| 65 MB: through selected timeline | 1,714 ms | 1,468 ms | 14.4% |
| 94 MB: file to dashboard | 2,173 ms | 1,878 ms | 13.6% |
| 94 MB: through selected timeline | 2,333 ms | 2,029 ms | 13.0% |
| 65 MB: selected query and serialization | 23 ms | 8 ms | 65.2% |
| 94 MB: selected query and serialization | 33 ms | 12 ms | 63.6% |

The through-timeline figure sums the three application milestone medians.
Individual paired dashboard improvements were 116-263 ms on 65 MB (median
192 ms), and 283-298 ms on 94 MB (median 295 ms). Absolute timings drifted:
the 65 MB control's first four samples fell from 1,788 to 1,473 ms, while each
adjacent comparison still favored the candidate. The full sample set is retained;
the table does not select the fastest batch.

File streaming fell from 407 to 313 ms on 65 MB and 562 to 445 ms on 94 MB.
WASM pushes remained near 303/300 ms and 422/416 ms, respectively. Finish and
serialization medians were 1,029/903 ms and 1,425/1,283 ms. Those complete-bundle
finish differences should not be attributed to one isolated optimization.
Relative to the original pre-optimization dashboard controls of 2,894 and
3,959 ms, these final medians are approximately 54% and 53% lower.

## Retained implementation

- Initialize WASM and its worker pool when the empty UTrace viewer mounts.
  Preserve the prepared worker when selecting a file; release the previous
  completed session when replacing a capture. Interrupted loads still cancel.
  The comparison viewer also prepares the shared worker. Preparation has its
  own OpenTelemetry span and remains visible in saved benchmark traces.
- Read the `File` inside the parser worker. Bounded 1 MiB views go directly to
  WASM, and progress messages go to the page without a per-chunk acknowledgement.
  Cancellation closes the reader and releases the session. An identity check
  after each asynchronous read prevents use of a released/replaced WASM object.
  Commands and progress/success/error responses share TypeScript boundary types.
- Borrow GPU interval names until a retained row or a new interned string needs
  ownership. Move the GPU collector/fanout into the GPU timeline module. Its
  independent limits and public JSON remain unchanged.
- Use 8,192-entry CPU timeline pages instead of 65,536. Resolve names only for
  text filtering and returned rows, and replace the bounded heap's latest item
  in place. Queries still count all matching intervals and retain the same
  earliest bounded result, including unknown timer identities. For deeply nested
  traces, pages grow to amortize stack checkpoints and keep retained checkpoint
  storage linear in decoded entries.

## Experiments and decisions

| Experiment | Evidence | Decision |
| --- | --- | --- |
| Skip callbacks for unconsumed normal-event UIDs | Alternating three-pair browser runs showed no dispatch or opening gain. | Removed. |
| Borrow GPU interval names | The first native 65 MB probe moved from 702.86 to 669.96 ms, including pushes and finish; expanded GPU-query snapshots matched. | Retained; final paired opening results measure the complete change, not this isolated single probe. |
| Prewarm WASM; read one chunk ahead; transfer complete buffers without slicing | Three-run dashboard median was 1,440 ms. | Worker preparation retained. The input implementation was superseded by direct worker reading. |
| Dense per-thread plain CPU counters, capped at 1 MiB per active job | Native total 673.97 ms versus the GPU-only probe's 669.96 ms; browser CPU aggregation 274 ms versus roughly 272 ms. | Removed; no demonstrated gain. |
| Smaller pages and deferred query-name resolution | Selected query/serialization fell from about 23 ms to 8–9 ms on 65 MB. Expanded output remained byte-identical. | Retained. |
| Read the file directly in the worker | Streaming fell from roughly 400 to 325 ms while WASM pushes stayed near 305 ms. | Retained. |
| Cache CPU frame attribution | Initial browser aggregation measured 249-259 ms, but later alternating controls were faster: 242/250 ms without the cache versus 258/251 ms with it. | Removed. The initial batch was not causal evidence. |
| Sweep worker caps 4, 8, 12, 16, 24 | Lower caps did not establish a consistent improvement; 24 increased CPU aggregation to 317–382 ms in its sweep. | Keep the cap at 16. |
| Schedule largest streams first | CPU aggregation rose to 383-407 ms with ordinary Rayon splitting. One-stream-per-job splitting still took 368/373 ms in alternating runs versus 251/258 ms in its earlier control. | Removed both scheduling variants. The scheduling bundle also included extra GPU collection lookups, which were then tested separately. |
| Avoid repeated GPU collection string allocations with borrowed lookups | Separate three-run dispatch times were 492, 541, and 619 ms; this noisy batch did not demonstrate an improvement over the earlier implementation. | Removed. |

The expanded fixture probe compares dashboard, inventory, sampled CPU timeline
queries, sampled timer statistics, and GPU queries for every retained dashboard
frame against the checkpoint. It also compares parallel and eager decoding with
different input chunk boundaries. GPU queries retain their existing 10,000-row
limit. These snapshots are broader than the prior round's CPU-only query probe.

## Measurement boundaries and remaining tradeoffs

Each browser sample starts a fresh Chromium process. The primary opening span
begins at file selection and ends after dashboard paint. The through-timeline
figure adds the Timers-tab and selected-timeline spans, excluding automation
gaps. Worker preparation can precede file selection; browser startup and its
preparation trace are retained separately. Preparation moves initialization
earlier rather than eliminating its cost.

Persistent indexes could accelerate reopening an already analyzed capture, but
would need a separately measured warm-cache path and validated cache/version
identity. They do not reduce the first analysis of a new capture. An earlier
interactive CPU preview would require splitting the synchronous finish API
into explicit ownership phases: provider dispatch establishes metadata before
full CPU aggregation and provider projection complete. That is a different
readiness milestone from the complete dashboard measured here. Neither change
is included or credited as a cold-opening speedup.

CPU timeline construction already overlaps ordered provider dispatch. Previous
experiments combining it with CPU aggregation or retaining every borrowed batch
descriptor regressed locality and memory pressure. This round keeps the existing
independent passes. The worker and scheduling sweeps likewise show that extra
parallelism alone is not a reliable improvement.

Machine-readable samples, hashes, and final comparisons are in
[`utrace-performance-results-2026-09-06-continuation.json`](utrace-performance-results-2026-09-06-continuation.json).
Raw reports, OpenTelemetry/Chrome traces, screenshots, and Unreal logs remain
under `web/benchmark-results/`.

## Validation

Final native validation passed `cargo fmt`,
`cargo clippy --all-targets --all-features -- -D warnings`, and
`cargo test --all-targets --all-features` (186 active tests). All five real
captures matched their expanded saved snapshots and the eager decoder with
different chunk boundaries. Regression cases cover borrowed GPU-name ownership,
independent sink truncation, unknown timer names and filters, and deep nesting
with linear checkpoint storage. Frontend unit tests passed all 55 cases.

Retained CPU index allocations are 42,474,032 bytes for 65 MB and 63,555,429
bytes for 94 MB, versus 42,653,576 and 63,472,141 at the checkpoint. Smaller
pages therefore save about 180 KB on the former and add about 83 KB on the
latter; these are retained-index figures, not peak process memory.


Both production WASM builds and the final nine-test browser suite passed.
The browser suite includes preparing/reusing the single-capture worker, releasing
replaced sessions, parsing with and without shared-memory threads, cancelling
an active file read followed by two new captures, and large-table/timeline
interaction. Performance samples also assert that push spans account for every
input byte. The 94 MB alternating run uses the performance spec; the unrelated
large-table fixture assertions remain targeted at the default 65 MB capture.

## Fresh Unreal comparison

A separate five-run comparison with Unreal Insights 5.7 measured a 1,250 ms
analysis median and 2,449 ms from process launch to Timers data readiness.
The browser's independent five-run dashboard median was 1,535 ms and its
application through-selected-timeline median was 1,687 ms. These browser values
are higher than the alternating study's candidate medians; they are retained
rather than replaced with the fastest batch. Neither study provides an
identical Unreal Slate paint milestone, so the results do not establish a
paint-to-paint win over Unreal. The alternating study establishes this round's
improvement over our own checkpoint.


After the timed study, final lifecycle review added a client-side identity guard
so cancellation cannot recreate a worker that route cleanup has already
terminated. The new regression test fails against the saved measured bundle
(two parser workers) and passes with the guard (one worker). The complete
nine-test suite was rerun with the guard. This changes cancellation cleanup;
the production WASM binaries are identical. The results JSON distinguishes the
measured bundle from the final verified bundle.
