# UTrace frontend end-to-end performance

This is the reproducible comparison contract for the repository web viewer and
Unreal Insights. It deliberately separates parser, application-state, and
render milestones so a fast decoder cannot hide a slow interface, and a fast
paint cannot hide incomplete analysis.

## What the harness measures

The browser run uses a real `.utrace` file in headless Chromium with the same
cross-origin isolation headers as the threaded application. Every repetition:

1. Records a host timestamp immediately before `chromium.launch()` and starts a
   fresh Chromium process.
2. Creates a page, navigates to `/utrace`, waits for the visible capture control,
   and waits through two animation frames for the empty application paint.
3. Selects the file through the real `<input type="file">`.
4. Streams it into the Worker and progressive Rust/WASM session.
5. Finishes provider analysis and serializes/parses the final JSON contract.
6. Commits Solid state and waits through two animation frames for the dashboard.
7. Opens **Timers**, selects the capture's worst frame marker, queries the retained
   CPU timeline index, commits it, and waits through two more animation frames.
8. Closes Chromium before the next repetition.

The production `dist` bundle is already running under `vite preview` when the
timestamp is taken. Server and build startup are deployment infrastructure, not
browser-viewer startup, and remain outside the sample. `browser_launch_ms`
covers the launch call until Playwright connects to Chromium;
`browser_launch_to_app_ready_ms` also includes context/page creation, navigation,
and the initial paint.

The test is fixture-gated. A missing real trace is an explicit Playwright skip,
not a passing test that asserted nothing.

The Unreal run likewise records a host timestamp immediately before spawning the
installed `UnrealInsights.exe` with the same trace and reads two native log
milestones:

- `Analysis has completed in ...`: Unreal's own analysis timer. It excludes
  process startup.
- `[Timers] Aggregated stats computed ...`: the first complete Timers
  aggregation. The runner records process launch through this line.

The second Unreal value is the closest stable frontend boundary exposed by the
stock application, but it is **not** a paint-complete event. A true Slate paint
comparison would require Unreal source instrumentation or an OS-level visual
automation probe. The report calls it `process_to_timers_ready_ms` so it cannot
be mistaken for pixels-on-screen timing.

## Run it

From `web/`:

```powershell
node scripts\benchmark-utrace-e2e.mjs `
  --input "C:\traces\capture.utrace" `
  --repeat 3 `
  --unreal "C:\Program Files\Epic Games\UE_5.7\Engine\Binaries\Win64\UnrealInsights.exe"
```

`--repeat` accepts 1–10 samples and reports medians. `--browser-only` omits
Unreal. `--skip-build` reuses the current production `dist` bundle for local
iteration. The equivalent environment variables are `UTRACE_E2E_TRACE`,
`UTRACE_E2E_REPEAT`, `UTRACE_E2E_OUTPUT_DIR`, and `UNREAL_INSIGHTS_EXE`.

The runner builds the deterministic threaded and fallback WASM modules and the
production web bundle before starting timed work. Build time and preview-server
startup are not part of a sample.

For fast iteration with an already-built production bundle:

```powershell
$env:UTRACE_E2E_TRACE = "C:\traces\capture.utrace"
$env:UTRACE_E2E_REPEAT = "1"
npm run test:e2e:quick
```

## Artifacts

Each durable result directory contains:

| Artifact | Purpose |
| --- | --- |
| `comparison.json` | Browser and Unreal samples, medians, milestone definitions, and ratios |
| `browser-performance.json` | Selected browser phase timings and runtime/memory observations |
| `browser-chrome-trace.json` | All in-page browser, Worker, and remote WASM spans for the final repetition, for Chrome/Perfetto |
| `browser-timeline.png` | The exact final browser state used by the test |
| `unreal-insights-N.log` | Source log for each Unreal sample |

The page also exposes `window.__UTRACE_BENCHMARK__`. Its `snapshot()` and
`chromeTrace()` methods return the same typed span stream used by Playwright.
Process launch and navigation begin before that page exists, so those cold-start
durations live in `browser-performance.json`, not the in-page Chrome trace.

## Span hierarchy

The OpenTelemetry span names are a typed contract in
`web/src/lib/perf-span-types.ts`. The important boundaries are:

```text
utrace.load
├─ utrace.file.stream
├─ utrace.worker.round_trip
│  ├─ utrace.worker.request
│  ├─ utrace.wasm.module_import / init / thread_pool_init
│  ├─ utrace.session.start
│  ├─ utrace.session.push (one per file chunk)
│  ├─ utrace.session.analyzing
│  └─ utrace.session.finish_and_serialize
│     ├─ transport_finalize / inventory
│     ├─ event_registry / important_events / provider_important_events
│     ├─ normal_event_dispatch
│     ├─ cpu_aggregation
│     ├─ provider_finalize
│     ├─ cpu_timeline_finalize / gpu_timeline_finalize
│     └─ output_construction / json_serialize
├─ utrace.json.decode
├─ utrace.dashboard.state_commit
└─ utrace.dashboard.paint

utrace.timeline.load
├─ utrace.timeline.query_and_serialize
├─ utrace.json.decode
├─ utrace.timeline.state_commit
└─ utrace.timeline.paint
```

GPU timeline queries have the same query/state/paint split. Remote Worker spans
retain their absolute browser time origin and are imported under the initiating
browser span, so the Chrome trace shows overlap rather than incorrectly adding
parallel work.

## 2026-08-09 three-sample baseline

Host: Windows, 16-core/32-thread Ryzen system, Chrome 151 headless, UE 5.7.
Capture: `funguys-build-30229-dev-20260807.utrace`, 65,450,435 bytes. These are
medians from three sequential samples, not release claims:

| Milestone | Web | Unreal |
| --- | ---: | ---: |
| Process launch call | 53 ms to Chromium connection | included in total below |
| Empty app ready | 403 ms from Chromium launch | n/a |
| Analysis only | finish + serialize 2.03 s | native analysis 1.16 s |
| Initial frontend, cold | Chromium launch → dashboard paint 4.33 s | process launch → Timers data ready 2.17 s |
| First timer view, cold | Chromium launch → timeline paint 5.89 s | no stock paint signal |
| File-selected dashboard | 3.71 s | n/a |
| Timer query | query + serialize 35 ms | first Timers aggregation 398 ms |
| Timer rendering | 201 ms for 2,500 SVG intervals | no stock paint signal |

At the closest available cold-start frontend milestone, dashboard paint versus
Timers data ready, the web viewer was about 2.00× slower. Requiring the web's
first timer pixels raises that to about 2.72×, but stock Unreal still stops at a
data-ready log rather than a paint-complete event. Comparing the web file-load
total only to Unreal's internal analysis timer produces an apparent 3.20× ratio;
those boundaries are also deliberately kept separate.

## Optimization readout

The five-sample retained run after detailed finish instrumentation, precomputed
UID routing, and a 16-worker cap measured:

| Metric | Original baseline | Retained run | Change |
| --- | ---: | ---: | ---: |
| Finish + serialize | 2,029 ms | 1,912 ms | -117 ms (-5.8%) |
| File selection → dashboard paint | 3,710 ms | 3,607 ms | -104 ms (-2.8%) |
| Chromium launch → first timer paint | 5,893 ms | 5,789 ms | -104 ms (-1.8%) |

The retained finish-phase medians were:

| Finish phase | Median |
| --- | ---: |
| Normal event dispatch | 708 ms |
| CPU aggregation | 933 ms |
| Provider finalization | 147 ms |
| CPU timeline finalization | 54 ms |
| JSON serialization | 29 ms |
| Registry, important providers, transport, inventory, GPU, and output construction | 29 ms combined |

The spans now set this priority order:

1. CPU aggregation and normal-event dispatch remain the dominant parser stages.
   The report persists every finish phase for every repetition under
   `wasm_finish_phases_ms`, plus phase medians under
   `medians_ms.wasm_finish_phases`.
2. Progressive pushes total roughly 1.4 s and overlap the roughly 1.5 s browser
   file stream. They are meaningful, but do not add linearly to total wall time.
3. Dashboard state and paint are about 30 ms after the chart migration, so
   the initial UI is not the primary bottleneck.
4. A retained CPU timeline query is about 35 ms, while painting 2,500 nested SVG
   intervals costs roughly 201 ms. Rendering—not querying—is the first-timer
   interaction target. Solid's TanStack adapter is SVG-only today, so the dense
   interval renderer should be evaluated separately for Canvas or viewport
   virtualization rather than forced through a statistical chart abstraction.
5. Earlier hot-path candidates such as direct LZ4 decompression, enum dispatch,
   fast scalar-key maps, cached active-frame lookup, and borrowed dispatch views
   are already present in the current tree. Re-measure before reviving older
   optimization proposals.

The worker sweep confirmed a knee on this 16-core/32-thread host: 16 workers
improved finish by 86–117 ms across five-sample runs, while 24 workers regressed
finish by 340 ms versus 16 because contention outweighed additional parallelism.
See `utrace-performance-experiments-2026-08-09.md` for rejected experiments.

Do not sum overlapping child spans to derive wall time. Use the root spans for
wall-clock comparisons and children for attribution.
