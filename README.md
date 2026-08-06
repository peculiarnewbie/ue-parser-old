# UTrace parser

Read-only parsing and bounded analysis for Unreal Engine `.utrace` captures.

This repository owns the UTrace transport decoder, event registry, provider
aggregation, dashboards, progressive sessions, timeline indexes, and the
browser WebAssembly binding. Common bounded byte-reading and archive error
behavior comes from the pinned UE Shed dependency in
[`ue-shed`](https://github.com/ue-shed/ue-shed).

## Rust crate

The library exposes the UTrace API from `src/utrace.rs` and its focused helper
modules. The native command-line target retains a legacy name for
compatibility; its UTrace commands are:

```text
cargo run -- utrace inspect Trace.utrace --format json
cargo run -- utrace inventory Trace.utrace --format json
cargo run -- utrace dashboard Trace.utrace --format json
cargo run -- utrace timeline index Trace.utrace --output Trace.utix --format json
cargo run -- utrace timeline query Trace.utix --format json
cargo run -- utrace coverage Trace.utrace --format json
cargo run -- utrace html Trace.utrace --output Trace.html
```

Run the native checks with:

```text
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```

Real-capture tests are feature-gated and accept `UTRACE_FIXTURE`,
`UTRACE_FIXTURE_DIR`, and the provider-specific fixture variables documented
in `tests/utrace_fixture.rs`.

## Browser WebAssembly package

[`@ue-shed/utrace-parser-wasm`](packages/utrace-parser-wasm) is browser-only
and should be used from a web worker for large captures. It exposes
`inspect`, `inventory`, `dashboard`, `dashboardBundle`, and
`createProgressiveDashboard`.

```text
cd packages/utrace-parser-wasm
npm install
npm run build
npm run check
npm publish
```

The package build requires `wasm-pack` and the
`wasm32-unknown-unknown` Rust target. `npm publish` runs the repository Rust
checks, builds the WASM artifact, and validates the package contents.

The local SolidJS web UI is in [`web/`](web/). It is documented and supported
as a UTrace viewer:

```text
cd web
npm install
npm run dev
```

## Shared byte-reader dependency

`Cargo.toml` pins the shared UE Shed reader dependency to an exact Git
revision. UTrace uses only its bounded reader and archive error types. If that
reader seam becomes unstable, extracting a small shared Rust crate is the next
step.
