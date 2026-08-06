# UE parser web UI

SolidJS + Vite frontend for inspecting Unreal Engine `.utrace` captures in a
browser Worker backed by Rust/WASM.

## Routes

- `/` — landing
- `/utrace` — progressive dashboard, in-browser timeline index, and charts

Charts use [`peculiar-charts`](https://charts.peculiarnewbie.com/).

## Setup

From the repo root:

```bash
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
