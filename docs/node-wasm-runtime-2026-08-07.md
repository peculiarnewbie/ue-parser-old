# Node and Bun WASM runtime findings — 2026-08-07

This note records runtime-specific behavior of the published portable
`@ue-shed/utrace-parser-wasm` build. It does not change the parser or dashboard
contract.

## Reference capture and contract

The Funguys capture used for release validation is approximately 65 MB:

```text
C:\Users\Ryzen\git\swag\tools\.local\traces\funguys-build-30229-dev-20260807.utrace
```

With `maxFrames: 120`, the expected result has schema version 2, 28 dashboard
families, a compact JSON size of 10,428,903 bytes, and SHA-256
`de496867f675bad98db00d55f2b1c0ef386bd36ce331bc184dd4b079b99571b6`.

## Version 0.2.0 measurements

| Runtime | Raw dashboard call | Memory observation |
|---|---:|---:|
| Node 26 / V8 | ~2.23 s | 310 MiB WASM, 525 MiB RSS |
| Bun 1.3.13 / JSC | ~42.0 s cold | ~1.35 GB RSS in the repeat benchmark |
| Bun 1.3.14 / JSC | 41.75 s cold | ~1.22 GB RSS |
| Bun 1.4.0-canary.1 / JSC | 42.21 s cold | ~873 MB RSS |
| Native serial Rust | ~1.76 s | native process memory |
| Native parallel feature | ~2.01 s | guarded serial fallback on this trace |
| Node package 0.1.2 | ~3.87 s | 817 MiB WASM, 984 MiB RSS |

Version 0.2.0 is about 42% faster than 0.1.2 under V8 and uses much less
memory. The large slowdown is specific to Bun/JSC execution of this WASM
workload, not a general 0.2.0 parser regression.

The Bun breakdown was 16 ms file read, 1.01 s for `Uint8Array.from`, 23 ms WASM
initialization, 42.0 s in `dashboardUtrace`, and 24 ms in `JSON.parse`. The raw
call includes wasm-bindgen's input transfer, Rust parsing/aggregation, and Rust
JSON serialization. Because everything outside that call is small, filesystem
I/O, caller-side copying, initialization, and output-string parsing are ruled
out as primary causes. Linear-memory growth may contribute to VM behavior, but
it does not explain the timing by itself; the same artifact grows far less
expensively under V8.

A repeat run through the committed benchmark measured Node 26.5 at 2.23 s in
the raw WASM call, 325,451,776 bytes of linear memory, and about 540 MB RSS.
Bun 1.3.13 measured 41.61 s, the same 325,451,776-byte linear memory, and about
1.35 GB RSS after the one-shot call. The public Node CLI completed in 2.40 s,
including subprocess startup, input/output I/O, and atomic publication. A fresh
native serial release build completed the parser command in 1.83 s.

Upgrading did not change the conclusion. Bun 1.3.14 revision `0d9b296af`
measured 41.75 s in the raw call and 28.88 s in progressive finish. Bun
1.4.0-canary.1 revision `45ee9556a` measured 42.21 s raw and 29.31 s finishing.
The canary improved one-shot RSS substantially, but not execution time. Both
produced the exact reference dashboard hash. The development host was returned
to stable Bun 1.3.14 after the comparison.

The `--profile-stages` benchmark uses the existing progressive API to separate
packet ingest/eager timeline construction from final dispatch, provider
aggregation, and serialization. On the same run, Node spent 1.32 s ingesting
and 1.48 s finishing; Bun spent 1.63 s ingesting and 28.16 s finishing. The
probe is diagnostic only and does not add fields to stable output.

This narrows the Bun regression primarily to the finish phase rather than file
I/O, input copying, WASM initialization, output transfer, or packet ingest.
That phase combines serial event dispatch, provider aggregation, allocation,
and JSON serialization, so the evidence does not isolate i64 operations or the
allocator as the cause. The supported minimal conclusion is a JSC WebAssembly
execution/tiering/code-generation issue expressed by the finish workload; a
JavaScript-side parser rewrite would be speculative.

## Supported runtime seams

The browser root retains wasm-bindgen's URL/fetch initializer. The explicit
`@ue-shed/utrace-parser-wasm/node` entry reads the same bundled WASM file and
passes its bytes to the initializer, avoiding unsupported `fetch(file://...)`
behavior without adding `node:*` imports to browser bundles.

The Node CLI writes output in the destination directory and publishes only a
completed file. It emits one-line JSON diagnostics on stderr and refuses to
overwrite an existing path. This avoids sending roughly 10 MB of JSON through
a parent-process pipe in the normal SWAG Auto integration.

The published browser WASM is portable and single-threaded. The repository web
viewer also has a threaded build using shared linear memory and Rayon. Browsers
only expose the required `SharedArrayBuffer` to cross-origin-isolated pages, so
deployments must send `Cross-Origin-Opener-Policy: same-origin` and
`Cross-Origin-Embedder-Policy: require-corp`. That build remains repository-only
and should not be confused with `DashboardOptions`, which selects bounded
output views rather than an execution strategy.
