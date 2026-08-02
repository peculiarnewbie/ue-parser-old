# UE parser web UI

SolidJS + Vite frontend for dropping `.uasset` / `.utrace` files and inspecting
them in a browser Worker backed by Rust/WASM.

## Routes

- `/` — landing
- `/uasset` — package inspection
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

The app has no backend picker and no native CLI bridge. `npm run dev` and
`npm run build` generate the browser bindings with `wasm-pack`, so Rust's
`wasm32-unknown-unknown` target and `wasm-pack` must be installed.
