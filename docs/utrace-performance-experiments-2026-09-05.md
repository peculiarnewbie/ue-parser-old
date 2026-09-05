# UTrace load-path experiments — 2026-09-05

This round ran across September 5–6 (Asia/Jakarta). It measures file selection
through the dashboard and then a selected CPU timeline, using the production
threaded WASM viewer and fresh Chromium processes.
The starting revision is `f67d32631934c7971536b925bcd2efbffa6d3715`.
The measurement boundaries remain those in [the E2E contract](utrace-e2e-performance.md).

## Final measured result

The original revision was rebuilt in a detached checkout after optimization and
measured again as a control. Its 65 MB dashboard median was 2,893.86 ms, versus
2,890.78 ms in the initial three-run baseline. Final numbers below use that fresh
control and a clean build of the retained implementation. Both versions use the
same dependencies, Chromium, fixtures, and worker cap.

| Application milestone | Original | Final | Reduction |
| --- | ---: | ---: | ---: |
| 65 MB capture: file → dashboard, five runs each | 2,894 ms | 1,726 ms | 40.3% |
| 65 MB capture: application through first selected timeline | 3,038 ms | 1,865 ms | 38.6% |
| 94 MB capture: file → dashboard, three runs each | 3,959 ms | 2,408 ms | 39.2% |
| 94 MB capture: application through first selected timeline | 4,114 ms | 2,573 ms | 37.4% |

The application-through-timeline value is the sum of the dashboard, Timers-tab,
and selected-timeline medians, excluding automation gaps, as in the existing
contract. Timeline query/serialization stayed at 23 ms on the 65 MB capture;
the selected timeline interaction was 96 ms versus 102 ms in the control.
The change primarily improves opening a capture.

On 65 MB, summed WASM pushes fell from 1,460 to 485 ms, file-stream wall time
from 1,590 to 576 ms, and finish/serialization from 1,153 to 972 ms. The final
dispatch median was 511 ms and CPU aggregation 279 ms. These remain the largest
analysis stages. Intermediate runs varied substantially with contention; the
retained result is a claim about the complete change, not a causal attribution
to each small patch.

Unreal Insights' five-run native analysis median was 1,190 ms in the final run
(1,240 ms with the fresh original-browser control). The browser finish phase is
shorter than that timer, while its complete file-to-dashboard duration is longer.
Stock Insights exposes no corresponding paint-complete event, so this does not
establish a browser-versus-Slate display-speed win. Its process-to-Timers log
milestone remains diagnostic only.

All primary samples and phase medians are retained in
[`utrace-performance-results-2026-09-05.json`](utrace-performance-results-2026-09-05.json).
That file also identifies the full local reports, Chrome traces, and screenshots
under `web/benchmark-results/`.

## Retained implementation

- `new_with_parallel_cpu_timeline` keeps transport and live frame markers
  incremental. At finish, independent physical-thread CPU decoders build exact
  pages alongside provider analysis. A scoped Rayon task joins before the
  catalog and pages become a queryable index. Both paths use the existing CPU
  batch decoder; global metadata introduction order still governs aggregation.
  The eager constructor and serial WASM fallback remain available.
- Streaming copies GPU payloads only for the two work events its live chart
  consumes, and borrows CPU event definitions. The shared normal-event framer
  now serves inventory, progressive decoding, and serial dispatch. Progressive
  sessions cache bounded UID layouts on declaration and use scalar-key hash maps
  for observation counts, sorting only when constructing output.
- CPU rendered-name totals look up borrowed strings within each spec. Repeated
  intervals allocate neither a temporary name nor a map key. Only new identities
  and retained samples own strings; the final projection restores ordered keys.
- Full live-frame snapshots publish at most every 50 ms, except the first and
  final updates. Unchanged or deferred snapshots do not clone the full frame
  list. The complete dashboard and exact timeline retain all previous data.

Timeline construction overlaps the provider spans. The finish root includes the
join; `cpu_timeline_finalize` includes remaining join/output work and page
finalization. Do not interpret the sum of provider spans as isolated worker CPU
time or add concurrent work to the root duration.

## Correctness evidence

`tests/utrace_progressive_performance.rs` compares the parallel path against
eager construction using different chunk boundaries. It checks every dashboard
provider, inventory, index metadata, storage statistics, and timeline and timer
queries in eleven windows distributed across the capture. All five available
captures pass (1.5, 2, 10.6, 65.5, and 94.1 MB).

Two snapshots recorded before implementation still match byte for byte:

| Capture | Snapshot bytes | SHA-256 | Retained timeline allocation |
| --- | ---: | --- | ---: |
| `funguys-build-30229-dev-20260807.utrace` | 24,488,983 | `ac015b20e0c0d9b5ab759568ee91a24d51e8dedf35e053b96f3168257a08472f` | 42,653,576 B |
| `projectsubway-editor-long-start-20260806.utrace` | 30,410,939 | `665f354bb629d2c454036a568a68477611093162b40639ab0740efc09922eb47` | 63,472,141 B |

These allocations describe the retained CPU index, not peak process memory.
The snapshots include dashboard/inventory data and sampled range-query output;
they are not hashes of the input files or exhaustive hashes of every interval.

Synthetic tests cover one- and two-worker scheduling, disjoint physical threads,
single-byte chunking, malformed CPU batches, truncated framing and auxiliary
payloads, and first/periodic/final snapshot delivery.

The older real-capture tests assumed frame 1 was absent and required a particular
"Writing trace" message. Both failed identically on the original revision with
the benchmark capture. Further assertions assumed an editor-only Concert channel,
no completed regions, and a particular sampled GPU string. The assertions now
check absent-frame queries, log-argument substitution, announced channel fields,
region accounting, and decoded GPU strings without those capture-specific values.

Final verification passed `cargo fmt`, strict all-target/all-feature Clippy,
`cargo test --all-targets --all-features`, all six `real_` fixture tests on the
65 MB capture, both WASM production builds, the six-test browser suite (including
the single-thread fallback), and the additional 94 MB performance spec. Other
fixture-specific tests retain their explicit ignore requirements.

## Reproduce

```powershell
node web/scripts/benchmark-utrace-e2e.mjs --input C:\traces\capture.utrace --repeat 5

$env:UTRACE_FIXTURE = 'C:\traces\capture.utrace'
$env:UTRACE_PROFILE_OUTPUT = 'target\capture-after.json'
# Optional snapshot saved from an earlier revision:
$env:UTRACE_PROFILE_REFERENCE = 'target\capture-before.json'
cargo test --release --features utrace-parallel --test utrace_progressive_performance -- --ignored --nocapture
```

The native probe excludes file reading, JSON serialization, range queries, and
the subsequent eager-equivalence check from its printed push/finish timings.
The browser benchmark includes input delivery, serialization, application state,
and paint, so native and browser totals should not be equated.

For another capture, the performance spec can be run on its own against an
already-built production bundle. The full UI suite also contains table-size
expectations specific to the default fixture; the 94 MB editor trace has 195
counters rather than the more than 200 expected by that separate table test.

```powershell
cd web
$env:UTRACE_E2E_TRACE = 'C:\traces\editor.utrace'
$env:UTRACE_E2E_REPEAT = '3'
$env:UTRACE_E2E_OUTPUT_DIR = 'benchmark-results\editor'
npx playwright test e2e/utrace-performance.spec.ts
```
