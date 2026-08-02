# uasset-parser-utrace-wasm

Browser WebAssembly bindings for the repository's read-only Unreal Engine
UTrace parser. Parsing happens in the browser; this package does not upload
captures or use a backend.

It is browser-only in the initial release. Consumers should run parsing in a
web worker for large captures.

## Install

```text
npm install uasset-parser-utrace-wasm
```

## Use

Each operation initializes the bundled WebAssembly module on first use. Call
`init()` earlier if eager loading is useful.

```js
import { dashboard, inventory } from "uasset-parser-utrace-wasm";

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

For streamed input, `createProgressiveDashboard` owns a WASM session. Call
`finish()` on success or `dispose()` when cancelling so its WASM allocation is
released.

```js
import { createProgressiveDashboard } from "uasset-parser-utrace-wasm";

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

The build requires `wasm-pack` and Rust's `wasm32-unknown-unknown` target.
