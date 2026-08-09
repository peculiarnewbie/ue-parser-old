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
| Raise Rayon cap from 8 to 16 on a 32-logical-CPU host | Five-run finish median 1,943 ms in the sweep and 1,912 ms in the final confirmation | The cap remains 16, but the later metadata-stack discovery showed that this capture still selected the serial CPU fallback. Do not interpret this sweep as evidence about Rayon scaling. |
| Raise Rayon cap from 16 to 24 | Finish median regressed from 1,943 to 2,283 ms; process-to-timeline regressed by 609 ms | Reverted. The metadata-stack discovery makes this capture's worker-count result inconclusive. |
| Replay `MetadataStack` events in physical thread order during parallel CPU aggregation | The capture's 265 `ClearScope` and one `SaveStack` events had disabled the parallel path. Enabling their thread-local replay reduced native finish from 1,358 to 878 ms and the ten-sample browser CPU aggregation median from 933 to 541 ms. | Retained. Serial and parallel dashboard JSON were byte-for-byte equivalent after newline normalization (10,395,737 characters; SHA-256 `511049F5B9A7417E2A726194EEDECAD0FF4712058B51B4B00F4AEFF5C1429F54`). |
| Resolve GPU event variants and fixed-field layouts once per UID | Nine native release samples reduced finish from 905 to 805 ms; the variant-only midpoint was 872 ms. Across two five-launch browser runs, normal dispatch moved from 744 ms before GPU routing to 731.5 ms with variant routing and 722.5 ms with cached layouts. | Retained. Required field existence, declared size, range, and payload bounds remain checked when each event is decoded. Dashboard JSON still matched the 10,395,737-character reference and SHA-256 exactly. |

## Retained result

The metadata-stack change was confirmed in two independent five-launch browser
runs. Their combined ten-sample medians, with precomputed UID routing and at
most 16 Rayon workers, measured:

| Metric | Original | Before metadata-stack replay | Final | Change from prior retained result |
| --- | ---: | ---: | ---: | ---: |
| WASM finish + serialize | 2,029 ms | 1,912 ms | 1,666 ms | -247 ms (-12.9%) |
| CPU aggregation finish phase | 933 ms | 933 ms | 541 ms | -393 ms (-42.1%) |
| File selection → dashboard paint | 3,710 ms | 3,607 ms | 3,362 ms | -245 ms (-6.8%) |
| Chromium launch → first timer paint | 5,893 ms | 5,789 ms | 5,243 ms | -546 ms (-9.4%) |

At the metadata-stack checkpoint, finish medians were 744 ms normal dispatch,
541 ms CPU aggregation, 246 ms provider finalization, and 1,666 ms for finish
plus serialization. CPU aggregation was no longer the largest finish phase.

The subsequent GPU route and fixed-layout optimization reduced the combined
ten-sample normal-dispatch median to 722.5 ms. Its current ten-sample full
finish median was 1,684 ms, however, because CPU aggregation and provider
finalization varied independently between runs. File-to-dashboard and
process-to-timeline medians likewise measured 3,379 and 5,467 ms. Treat this as
a repeatable 21.5 ms dispatch improvement and a 100 ms native finish
improvement, not as a demonstrated cold end-to-end browser win. Provider
finalization is the next target, but the rejected experiments show that
reducing passes or hash lookups is not sufficient by itself: locality and
concurrent memory pressure must be measured explicitly.
