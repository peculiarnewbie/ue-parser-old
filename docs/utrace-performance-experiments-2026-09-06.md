# UTrace streaming follow-up — 2026-09-06

This follows the [September 5–6 optimization round](utrace-performance-experiments-2026-09-05.md),
using its retained production bundle as the control. The browser, captures,
worker cap, and [measurement boundaries](utrace-e2e-performance.md) are unchanged.

## Measured result

The final comparison alternates the saved control and candidate bundles, reversing
their order every pair. Each sample starts a fresh Chromium process. No builds
or native probes ran during these measurements. All eight pairs improved.

| Application milestone | Previous implementation, fresh control | Final | Reduction |
| --- | ---: | ---: | ---: |
| 65 MB: file → dashboard, five pairs | 1,757 ms | 1,507 ms | 14.2% |
| 65 MB: application through selected timeline | 1,905 ms | 1,650 ms | 13.4% |
| 94 MB: file → dashboard, three pairs | 2,390 ms | 2,092 ms | 12.5% |
| 94 MB: application through selected timeline | 2,552 ms | 2,251 ms | 11.8% |

The through-timeline value sums the dashboard, Timers-tab, and selected-timeline
medians, excluding automation gaps. It does not imply that every CPU interval is
painted; the benchmark opens the existing bounded selected-frame timeline.

On 65 MB, WASM pushes fell from 487 to 299 ms and finish/serialization from 984
to 920 ms. On 94 MB, pushes fell from 743 to 425 ms while finish stayed roughly
flat at 1,330 versus 1,334 ms. Timeline query/serialization remained about 23 ms
and 34 ms respectively. Most of the gain comes from streaming work.

Relative to the original pre-optimization controls (2,894 and 3,959 ms), current
dashboard medians are about 48% and 47% lower. Those original controls were
measured in the preceding round; the paired table above is the direct evidence
for this follow-up. Unreal's earlier native analysis-only measurement remains
narrower than file-to-dashboard, and no equivalent Unreal paint measurement has
been added.

All paired samples, exploratory runs, and control/candidate WASM hashes are in
[`utrace-performance-results-2026-09-06.json`](utrace-performance-results-2026-09-06.json).
Raw browser reports, traces, and screenshots remain under `web/benchmark-results/`.

## Implementation

- Resolve the live-chart event subset at declaration time into a bounded UID
  table. The 65 MB capture contains 1,832,162 CPU batch events; these no longer
  look up an owned event definition or compare logger/event strings while
  streaming. A local cursor avoids per-event thread-map updates.
- Decode live frame and GPU work fields from borrowed stream slices into typed
  actions. Event definitions and payloads are no longer cloned for the 230,804
  GPU work events. Inventory samples still own their retained payloads, with
  the same normalized auxiliary encoding and deterministic sample selection.
- Reject GPU submission timestamps outside the bounds of all published frames
  before searching frame history. Bounds are conservative when repeated frame
  ends move backward; the existing reverse search still decides overlaps.
  Pending GPU work is filtered in place when a new frame is published.
- Force inline the shared normal-event framer so callers can optimize through
  its bounded Reader operations. This adds about 8 KB per WASM binary. The
  ordinary inline hint showed no clear benefit and changed only nine bytes in
  each same-size binary; forced inlining produced different executable code.
  The paired result measures the complete retained change, not an isolated
  causal contribution from this hint.

Exploratory five-run dashboard medians ranged from 1,544 to 1,753 ms on effectively
the same code, while a control run drifted from 2,105 to 1,703 ms across samples.
This motivated alternating the bundles instead of selecting the fastest batch.

The first native probe reduced streaming from the control's three-run median
of 367.90 ms to 269.69 ms, before the frame-bound and pending-queue changes.
That probe retained byte-identical dashboard, inventory, and sampled CPU query
output. Native probe timings exclude file I/O, JSON, queries, and the later
eager-equivalence check; they are diagnostic, not browser load times.

Temporary metadata-cache instrumentation found 248,738 GPU and 149,224 CPU
cache hits. All 3,713 GPU and 3,679 CPU unique values fit the existing bounds.
The caches were left unchanged and the instrumentation was removed.

## Validation

Regression cases cover sparse two-byte event UIDs, reordered fixed fields,
auxiliary payloads, every packet split through live GPU events, overlapping
frame types, frame-end extension and contraction, and pending work delivery.
The existing real-capture snapshots include the completed progressive frame
chart as well as provider and CPU query results.

Final validation passed `cargo fmt`, strict all-target/all-feature Clippy,
`cargo test --all-targets --all-features` (183 active tests), both production
WASM builds, and the six-test browser suite. All five real captures passed
release-mode comparisons against their previous-round snapshots and against
the eager timeline path with different chunk boundaries. Snapshot SHA-256s
are retained in the results JSON. The 65 MB and 94 MB retained CPU index
allocations remain 42,653,576 and 63,472,141 bytes.

Final native diagnostic probes measured 212.37 ms pushing plus 479.84 ms
finishing the 65 MB capture, and 311.45 plus 634.73 ms for 94 MB. These are
single validation probes, not additional browser medians or peak-memory figures.

The paired run used the performance spec alone on both captures. As documented
in the preceding round, the separate table-size UI test has expectations tied
to the default 65 MB fixture. Each pair copied a saved production bundle into
`web/dist`, ran `e2e/utrace-performance.spec.ts` with `UTRACE_E2E_REPEAT=1` in
a fresh browser, and switched bundles for its partner. Pair order alternated
control/candidate and candidate/control. The final candidate bundle is restored
in `web/dist`.

To benchmark the current implementation normally:

```powershell
node web/scripts/benchmark-utrace-e2e.mjs --browser-only --repeat 5
```
