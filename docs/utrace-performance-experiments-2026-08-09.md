# UTrace browser optimization experiments — 2026-08-09

This round used the production threaded WASM bundle, a fresh headless Chromium
process per repetition, and the 65,450,435-byte
`funguys-build-30229-dev-20260807.utrace` capture. The original reference is the
three-sample cold-start baseline in `utrace-e2e-performance.md`; retained worker
sweeps use five samples. Generated reports live under the gitignored
`web/benchmark-results/` directory.

## Finish attribution

The first instrumentation split the 2.03-second `finish_and_serialize` span.
The initial real-trace sample attributed 792 ms to normal-event dispatch,
933 ms to CPU aggregation, 157 ms to provider finalization, 54 ms to CPU
timeline finalization, and 42 ms to JSON serialization. All other finish phases
were small. This ruled out transport, inventory, and final JSON construction as
primary targets.

The WASM session now exports a validated, ordered phase profile. The worker
imports those phases as completed OpenTelemetry spans. The Playwright report
also retains phase durations and medians for every repetition.

## Experiments

| Experiment | Main result | Decision |
| --- | --- | --- |
| Borrow CPU batch views from serial dispatch instead of replaying thread framing | Three-run finish median increased from 2,029 to 2,180 ms; process-to-timeline increased to 6,329 ms | Reverted. Extra retained batch state and poorer locality cost more than the avoided framing pass. |
| Update one per-thread scope map and derive global totals during reduction | Native release finish median increased from 1,046 to 1,084 ms | Reverted. The extra reduction outweighed the removed hot-loop lookup on this trace. |
| Build timeline columns inside the parallel CPU aggregation pass | Push fell from 1,406 to 721 ms, but finish rose to 2,850 ms and file-to-dashboard rose by 321 ms | Reverted. Summary maps and timeline columns contend for cache and memory bandwidth when fused. |
| Resolve raw/modeled/provider routing once per event UID | Removed the remaining coverage-table string scan from the normal-event loop; eight-worker finish median moved from 2,029 to 2,017 ms | Retained. The speedup is small but the boundary enum removes stringly hot-path dispatch. |
| Raise Rayon cap from 8 to 16 on a 32-logical-CPU host | Five-run finish median 1,943 ms in the sweep and 1,912 ms in the final confirmation | Retained. |
| Raise Rayon cap from 16 to 24 | Finish median regressed from 1,943 to 2,283 ms; process-to-timeline regressed by 609 ms | Reverted. Sixteen is the observed knee on this host. |

## Retained result

The final five-sample confirmation, with precomputed UID routing and at most 16
Rayon workers, measured:

| Metric | Original | Final | Change |
| --- | ---: | ---: | ---: |
| WASM finish + serialize | 2,029 ms | 1,912 ms | -117 ms (-5.8%) |
| File selection → dashboard paint | 3,710 ms | 3,607 ms | -104 ms (-2.8%) |
| Chromium launch → first timer paint | 5,893 ms | 5,789 ms | -104 ms (-1.8%) |

Final finish medians were 708 ms normal dispatch, 933 ms CPU aggregation,
147 ms provider finalization, 54 ms CPU timeline finalization, and 29 ms JSON
serialization. CPU aggregation is therefore the next high-value target, but the
rejected experiments show that reducing passes or hash lookups is not sufficient
by itself: locality and concurrent memory pressure must be measured explicitly.
