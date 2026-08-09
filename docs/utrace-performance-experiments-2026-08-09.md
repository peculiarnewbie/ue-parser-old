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
| Select bounded per-frame CPU and GPU rankings before allocating names | Native probes attributed 58–61 ms of provider finalization to frame correlation. Exact top-5/top-8 selection reduced that slice to 15–18 ms and the nine-sample native finish median from 805 to 783 ms. Two five-launch browser runs reduced provider finalization from 263.5 to 127.5 ms and finish from 1,684 to 1,649.5 ms. | Retained. A regression test compares bounded selection with the previous full-sort ordering, and the real dashboard length and SHA-256 remain exact. Cold file-to-dashboard and process-to-timeline medians were effectively flat in these samples. |
| Bound and fuse CPU metadata projection | On 152,903 metadata records, bounded global/per-spec strings and rendered scopes reduced native metadata projection from roughly 55 to 49 ms. Fusing global strings, per-spec summaries, and scalar counters into one record pass moved it to 32 ms and total CPU provider projection from about 64 to 42 ms. | Retained. Two five-launch browser runs reduced provider finalization from 127.5 to 88 ms, finish from 1,649.5 to 1,526 ms, file-to-dashboard from 3,366 to 3,274 ms, and process-to-timeline from 5,465 to 5,342 ms. Ordering-equivalence tests and the real-trace SHA-256 remained exact. |
| Cache decoded GPU breadcrumb metadata by spec and exact payload | Sampled native dispatch attributed roughly 153–166 ms to breadcrumb begins. The trace had 94,896 metadata-bearing begins but only 3,667 distinct `(spec, payload fingerprint)` values, implying about 96% reuse. Three independent five-launch browser runs measured 625, 683, and 712 ms dispatch medians; their combined 675 ms median is 81 ms (10.7%) below the prior 756 ms checkpoint. | Retained with a 4,096-entry and 2 MiB copied-key cap. Exact payload equality, not the exploratory fingerprint, controls reuse. The first calm run reduced finish from 1,518 to 1,494 ms and file-to-dashboard from 3,273 to 3,207 ms; later runs had simultaneous CPU-aggregation and timeline-render contention, so no broader end-to-end claim is made. Cache reuse/cap tests and the real dashboard length and SHA-256 remain exact. |
| Share decoded CPU metadata by spec and exact payload | The 152,903-record catalog contained only 3,679 exact `(spec, payload)` values: 149,224 hits (97.6%), with just 11,371 bytes of unique raw keys. Three five-launch browser runs measured 537, 524, and 567 ms dispatch; their combined 552 ms median is 123 ms (18.2%) below the post-GPU-cache checkpoint. Nine native end-to-end runs measured a 1,148 ms median. | Retained. Each record keeps its independent metadata/spec IDs but shares immutable decoded values, strings, name, and rendered name through `Arc`, so Rayon reads remain safe. CPU and GPU use one 4,096-entry/2 MiB bounded cache primitive. The 15-sample finish median was 1,337 ms and file-to-dashboard was 3,206.8 ms; the independently noisy timeline leg is not treated as a new launch-to-timeline baseline. Output length and SHA-256 remain exact. |

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

Provider-finalization probes then isolated CPU summary projection at roughly
66–72 ms native and frame correlation at 58–61 ms; all other provider groups
rounded to zero on this capture. Bounded ranking moved frame correlation to
15–18 ms. Its combined ten-sample browser provider-finalization median was
127.5 ms, down 136 ms (51.6%), while finish plus serialization moved to
1,649.5 ms. The simultaneous dispatch and CPU-aggregation medians were noisier,
so file-to-dashboard (3,366 ms) and process-to-timeline (5,465 ms) stayed flat.
The next optimization target should therefore be CPU summary projection or a
new dispatch sub-profile, not the already-small provider dashboards.

The CPU split attributed 55–60 ms of its original 64–69 ms native projection
to the metadata dashboard. The final bounded, fused pass reduced metadata to a
32 ms median and the full CPU projection to about 42 ms. In the combined ten
browser samples, provider finalization reached 88 ms, finish plus serialization
1,526 ms, file-to-dashboard 3,274 ms, and process-to-timeline 5,342 ms. Normal
dispatch is again the dominant finish phase; further work should add sub-phase
attribution there before changing more event decoders.

The dispatch sub-profile attributed roughly 153–166 ms of native work to GPU
breadcrumb begins and 41–43 ms to ends. Metadata fingerprinting then found
94,896 metadata-bearing begins collapsing to 3,667 distinct spec/payload
fingerprints, with one value reused 7,776 times. The retained cache uses exact
payload bytes and shares the decoded CBOR values, extracted strings, hex
prefix, and rendered name. It stops retaining new entries after 4,096 values
or 2 MiB of copied keys and is dropped before parallel CPU aggregation.

Across three five-launch browser confirmations, dispatch medians were 625,
683, and 712 ms. Their combined 675 ms median is 81 ms (10.7%) below the prior
756 ms checkpoint. The first, relatively calm run also moved finish from 1,518
to 1,494 ms and file-to-dashboard from 3,273 to 3,207 ms. The other two runs
showed machine-wide contention: CPU aggregation, provider finalization, and the
independent post-dashboard timeline leg all rose together. Accordingly, the
cache is retained as a repeatable dispatch improvement, not recorded as a new
cold launch-to-timeline baseline. The dashboard remains 10,395,737 characters
with SHA-256
`511049F5B9A7417E2A726194EEDECAD0FF4712058B51B4B00F4AEFF5C1429F54`.

CPU metadata showed still greater reuse: 152,903 records collapsed to 3,679
exact spec/payload pairs, or 149,224 cache hits (97.6%), and all unique payload
keys occupied only 11,371 bytes. The retained representation leaves metadata
and spec IDs on each record while sharing the decoded values, extracted
strings, static name, and rendered name through `Arc`; this preserves the
catalog's `Sync` requirement for parallel aggregation. CPU and GPU now use the
same exact-payload cache implementation and the same 4,096-entry/2 MiB bounds.

The three five-launch confirmations measured 537, 524, and 567 ms dispatch
medians. Their combined 552 ms median is another 123 ms (18.2%) below the 675
ms post-GPU-cache checkpoint and 204 ms (27.0%) below the earlier 756 ms
checkpoint. The combined finish median was 1,337 ms and file-to-dashboard was
3,206.8 ms. Nine native release end-to-end runs measured 1,148 ms (1,121–1,305
ms). Dashboard-to-timeline remained independently elevated at about 2 seconds,
so this round does not replace the launch-to-timeline baseline. Dashboard JSON
remains exact at the same length and SHA-256 above.
