# Changelog

## 0.3.0 - 2026-08-07

### Added

- A supported `@ue-shed/utrace-parser-wasm/node` entry that initializes the
  bundled WebAssembly bytes from the package filesystem without
  `fetch(file://...)`.
- A turnkey `utrace-parser-wasm dashboard` subprocess CLI with atomic
  output-file publication, machine-readable diagnostics, and distinct failure
  exit codes.
- Packed-package Node consumer tests covering initialization, dashboard output,
  manifests, browser and Worker exports, package contents, and CLI failure
  behavior.
- An optional Node/Bun/native real-trace benchmark with explicit fixture skips,
  stage timings, memory reporting, output size, and SHA-256 verification.

### Changed

- Document browser, threaded-browser, Node, Bun, and native runtime tradeoffs.
- Clarify that `DashboardOptions` bounds output selection rather than choosing
  serial or parallel execution.
- Keep schema version 2 and the existing dashboard semantics unchanged.

### Performance

- On the 65.5 MB Funguys capture, Node 26.5 completed the raw WebAssembly call
  in about 2.22 seconds and the public CLI in about 2.40 seconds, versus about
  1.73 seconds for the native serial CLI.
- Bun 1.3.13, 1.3.14, and 1.4.0-canary.1 all required approximately 42 seconds
  for the same cold WebAssembly call. Stage instrumentation localized most of
  the discrepancy to final dispatch, provider aggregation, and serialization.
- Node, Bun, and native output matched the 10,428,903-byte reference dashboard
  SHA-256 `de496867f675bad98db00d55f2b1c0ef386bd36ce331bc184dd4b079b99571b6`.

Detailed runtime measurements are recorded in
[`docs/node-wasm-runtime-2026-08-07.md`](docs/node-wasm-runtime-2026-08-07.md).

## 0.2.0 - 2026-08-07

### Added

- Exact paged CPU timeline indexing, built eagerly during progressive trace
  ingestion and retained for frame-bounded browser queries.
- Exact retained GPU timeline queries.
- Optional per-trace-thread parallel CPU aggregation through the
  `utrace-parallel` feature.
- Automatic single-thread WebAssembly fallback in the repository web viewer
  when cross-origin isolation or `SharedArrayBuffer` is unavailable.

### Changed

- Stream event dispatch without duplicating raw payloads on the hot path.
- Use dense event lookup tables and cached CPU metadata attribution during
  aggregation.
- Keep schema version 2 and the existing JSON output contract.

### Performance

- On the 259.6 MB real-trace benchmark, native parallel aggregation reduced
  end-to-end processing from roughly 12.6 seconds to 11.6 seconds with about
  66 MiB additional peak memory.
- In the embedded-browser single-thread fallback, the same capture parsed in
  24.3 seconds; a representative frame-bounded query completed in 1.5 ms.

Detailed measurements and rejected experiments are recorded in
[`docs/utrace-performance-experiments-2026-08-07.md`](docs/utrace-performance-experiments-2026-08-07.md).
