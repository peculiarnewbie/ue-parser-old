# UE parser web UI

SolidJS + Vite frontend for dropping `.uasset` / `.utrace` files and inspecting
them with the Rust `uasset` CLI.

## Routes

- `/` — landing
- `/uasset` — package inspect (`uasset inspect --format json`)
- `/utrace` — dashboard + inventory charts (`uasset utrace dashboard|inventory`)

Charts use [`peculiar-charts`](https://charts.peculiarnewbie.com/).

## Setup

From the repo root, build the CLI with utrace enabled (recommended once):

```bash
cargo build --features utrace
```

Then:

```bash
cd web
npm install
npm run dev
```

Open http://localhost:5173. The Vite middleware accepts raw file uploads on:

- `POST /api/uasset/inspect`
- `POST /api/utrace/dashboard`
- `POST /api/utrace/inventory`

It prefers `target/release/uasset` or `target/debug/uasset`, and falls back to
`cargo run --features utrace` when no binary is present.

## Notes

Parsing is local-only via the CLI bridge — this is not a WASM build yet. Large
`.utrace` files are better served that way than in-browser for now.
