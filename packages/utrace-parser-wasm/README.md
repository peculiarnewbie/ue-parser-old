# `@ue-shed/utrace-parser-wasm`

Browser WebAssembly bindings for the repository's read-only Unreal Engine
UTrace parser. Parsing happens in the browser; this package does not upload
captures or use a backend.

It is browser-only in the initial release. Consumers should run parsing in a
web worker for large captures.

The published package uses portable single-thread WebAssembly and does not
require `SharedArrayBuffer` or cross-origin isolation. The repository's web
viewer additionally builds an opt-in shared-memory variant and selects it when
the browser supports cross-origin-isolated workers.

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
