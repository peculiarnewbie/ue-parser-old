# UE parser web UI

SolidJS + Vite frontend for inspecting Unreal Engine `.utrace` captures in a
browser Worker backed by Rust/WASM.

## Routes

- `/` — landing
- `/utrace` — progressive dashboard, in-browser timeline index, and charts
- `/utrace/compare` — baseline/candidate frame distributions, CPU attribution,
  independent local range aggregation, and timeline evidence

The comparison worker retains two captures and releases them on replacement or
navigation. See [the comparison contract](../docs/utrace-comparison-contract.md)
for normalization, timer matching, and incomplete-evidence rules.

Analytical charts use
[`@tanstack/solid-charts`](https://tanstack.com/charts/latest/docs/framework/solid/adapter).
The dense CPU/GPU interval timelines remain purpose-built SVG renderers because
they encode nested intervals rather than ordinary statistical series.

## Setup

From the repo root:

```bash
rustup toolchain install nightly-2025-11-15 --profile minimal --component rust-src --target wasm32-unknown-unknown
cd web
npm install
npm run dev
```

Open http://localhost:5173. Files are read by a browser Worker and are not
posted to a Vite parsing endpoint.

## Notes

The current documented product surface is UTrace-only. Captures stay local;
the browser route parses them in a dedicated worker. `npm run dev` and
`npm run build` generate the browser bindings with `wasm-pack`, so Rust's
`wasm32-unknown-unknown` target and `wasm-pack` must be installed.

CPU dashboard aggregation uses shared-memory WASM workers when the page is
cross-origin isolated. Vite development and preview responses include the
required COOP/COEP headers; production hosts should likewise serve
`Cross-Origin-Opener-Policy: same-origin` and
`Cross-Origin-Embedder-Policy: require-corp` to enable the threaded build. The
worker automatically loads a single-thread build when `SharedArrayBuffer` is
unavailable, including embedded browser contexts.

## End-to-end performance benchmark

The Playwright harness launches a fresh Chromium process for every repetition,
measures process launch through the painted empty application, dashboard, and
worst-frame timer timeline, and retains the detailed file-selection and query
spans. It emits JSON medians, an OpenTelemetry-compatible Chrome trace, and a
full-page screenshot.

Use an already-built production `dist` bundle for a quick browser-only run:

```powershell
$env:UTRACE_E2E_TRACE = "C:\traces\capture.utrace"
$env:UTRACE_E2E_REPEAT = "1"
npm run test:e2e:quick
```

Run the reproducible browser-versus-Unreal comparison, including a clean WASM
build, with:

```powershell
node scripts\benchmark-utrace-e2e.mjs `
  --input "C:\traces\capture.utrace" `
  --repeat 3 `
  --unreal "C:\Program Files\Epic Games\UE_5.7\Engine\Binaries\Win64\UnrealInsights.exe"
```

Results are written under `benchmark-results/` by default. The web server and
build are outside the cold-browser sample. Unreal's exact internal analysis
duration is recorded separately from process launch through the first completed
Timers aggregation. The latter is a frontend-data-ready proxy: Unreal Insights
does not expose a Slate paint-complete event. See
[`docs/utrace-e2e-performance.md`](../docs/utrace-e2e-performance.md) for the
milestone contract and current baseline.
