# `@ue-shed/utrace-parser-wasm`

Browser and Node.js WebAssembly bindings for the repository's read-only Unreal
Engine UTrace parser. Parsing is local; this package does not upload captures
or use a backend. Browser consumers should use a web worker for large captures.

The published package uses portable single-thread WebAssembly and does not
require `SharedArrayBuffer` or cross-origin isolation. The repository's web
viewer additionally builds an opt-in shared-memory variant and selects it when
the browser supports cross-origin-isolated workers.

`DashboardOptions` bounds which frame and timeline rows are retained in the
output. It does not select serial or parallel execution. The published WASM is
always the portable serial build.

## Install

```text
npm install @ue-shed/utrace-parser-wasm
```

## Use

Each operation initializes the bundled WebAssembly module on first use. Call
`init()` earlier if eager loading is useful.

```js
import { dashboard, inventory } from "@ue-shed/utrace-parser-wasm";

const bytes = new Uint8Array(await file.arrayBuffer());
const result = await dashboard({
  bytes,
  filename: file.name,
  options: { maxFrames: 120 },
});

const eventInventory = await inventory({ bytes, filename: file.name });
```

## Node.js API and CLI

Node consumers must use the explicit `node` entry. It initializes the same
generated WASM artifact from the package filesystem and does not call
`fetch(file://...)`. Keeping Node built-ins out of the root entry prevents
browser bundlers from pulling in filesystem shims.

```js
import { readFile } from "node:fs/promises";
import { dashboard } from "@ue-shed/utrace-parser-wasm/node";

const input = "C:\\traces\\capture.utrace";
const result = await dashboard({
  bytes: await readFile(input),
  filename: input,
  options: { maxFrames: 120 },
});
```

The Node entry exposes `init`, `inspect`, `inventory`, `dashboard`,
`dashboardBundle`, the progressive API, `parserManifest`, and the same DTO
types as the browser root.

For subprocess integrations, the package also installs a CLI:

```text
utrace-parser-wasm dashboard --input trace.utrace \
  --output trace-dashboard.json --max-frames 120
```

The CLI writes a temporary file in the destination directory, flushes it, and
publishes it atomically. It refuses to replace an existing output. A single
JSON status record is written to stderr; exit codes are `2` for usage, `3` for
input, `4` for initialization/parsing/validation, and `5` for output errors.

`inspect`, `inventory`, `dashboard`, and `dashboardBundle` return the stable
JSON envelopes emitted by the Rust parser. Their top-level
`schema_version` is currently `2`; treat a version change as a wire-contract
change. The nested trace DTOs are intentionally typed as JSON objects because
the parser supports expanding Unreal Engine event families between releases.

## Worker API

For a turnkey one-shot Worker, import `createUtraceParserWorker` from the root
package. The helper creates a module Worker and keeps the WASM instance off the
UI thread. It supports `inspect`, `inventory`, `dashboard`, and
`dashboardBundle`; the progressive session API remains available from
the root package.

```js
import { createUtraceParserWorker } from "@ue-shed/utrace-parser-wasm";

const parser = createUtraceParserWorker();
try {
  const result = await parser.dashboard({
    bytes: new Uint8Array(await file.arrayBuffer()),
    filename: file.name,
    options: { maxFrames: 120 },
  });
  // Render result.dashboard here.
} finally {
  parser.terminate();
}
```

The generated build metadata is available as a typed API and as the package
subpath `@ue-shed/utrace-parser-wasm/manifest`:

```js
import { parserManifest } from "@ue-shed/utrace-parser-wasm/manifest";

console.log(parserManifest.wasmSha256, parserManifest.maxInputBytes);
```

`dist/parser-manifest.json` is runtime/debug metadata for the exact build. Its
WASM hash does not replace npm’s package metadata or registry integrity data.

For streamed input, `createProgressiveDashboard` owns a WASM session. Call
`finish()` on success or `dispose()` when cancelling so its WASM allocation is
released.

```js
import { createProgressiveDashboard } from "@ue-shed/utrace-parser-wasm";

const session = await createProgressiveDashboard({
  filename: file.name,
  totalBytes: file.size,
  options: { maxFrames: 120 },
});

try {
  const reader = file.stream().getReader();
  for (;;) {
    const { done, value: chunk } = await reader.read();
    if (done) break;
    for (const event of session.pushChunk(chunk)) {
      // Render bootstrap and snapshot events as they arrive.
    }
  }
  reader.releaseLock();
  const completed = session.finish();
} catch (error) {
  session.dispose();
  throw error;
}
```

## Runtime choices and benchmark

- Use the portable root package in a browser Worker for broad compatibility.
- Use `/node` or the CLI for Node subprocesses. On the measured 65 MB Funguys
  trace, V8 runs the WASM close to the native serial CLI while using more
  memory.
- Bun 1.3.13, 1.3.14, and 1.4.0-canary.1 have a severe cold-WASM execution
  issue on that workload.
  File I/O, explicit input copying, initialization, and JSON parsing are not the
  dominant costs; almost all elapsed time is inside the raw WASM parser call.
  Prefer Node or native Rust for production subprocess parsing for now.
- The repository-only threaded browser build uses shared WebAssembly memory.
  It requires a cross-origin-isolated page (`COOP: same-origin` and
  `COEP: require-corp`) because browsers expose `SharedArrayBuffer` only in
  that security context. It is not the artifact published by this package.

Native Rust has the lowest runtime and memory overhead and can use native-only
features. WASM provides one portable parser artifact and schema across browser
and Node, at the cost of linear-memory duplication and runtime-specific JIT
behavior.

After `npm run build`, run the optional fixture-gated benchmark with either
runtime:

```text
npm run benchmark:real -- --input C:\\traces\\capture.utrace
bun ./scripts/benchmark-real-trace.mjs --input C:\\traces\\capture.utrace
```

Add `--profile-stages` to separate progressive ingest/eager-timeline work from
finish/aggregation, or `--native-cli <path-to-uasset>` for a native comparison.
When the fixture is absent the script emits an explicit JSON `skipped` result.

## Publishing this repository checkout

The package has no JavaScript dependencies. `npm publish` runs its WASM build
and artifact checks automatically.

```text
cd packages/utrace-parser-wasm
npm login
npm publish
```

Before a release, run the repository checks from the root:

```text
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

To build and validate the package locally without publishing it:

```text
cd packages/utrace-parser-wasm
npm install
npm run build
npm run check
```

The build requires `wasm-pack` and Rust's `wasm32-unknown-unknown` target.
