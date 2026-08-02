# Plan 010: Run UAsset and UTrace through native and browser-WASM backends

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update this plan's status row in
> `plans/README.md` unless a reviewer says they maintain the index.
>
> **Drift check (run first)**:
> `git diff --stat 039b112..HEAD -- Cargo.toml Cargo.lock src/lib.rs src/bin/uasset.rs src/output.rs src/wasm.rs tests/cli.rs tests/tiny_corpus.rs web/package.json web/package-lock.json web/vite.config.ts web/vite.parse-plugin.ts web/src/lib/api.ts web/src/lib/parser-backend.ts web/src/lib/wasm-worker.ts web/src/lib/wasm-worker-client.ts web/src/lib/types.ts web/src/routes/Uasset.tsx web/src/routes/Utrace.tsx web/src/index.css web/README.md`
>
> Several of these paths already have uncommitted user changes at planning
> time. Preserve them. If an in-scope file changed after `039b112`, compare the
> current-state descriptions below with the live file before proceeding. Stop
> if the intended ownership boundaries no longer fit.

## Status

- **Priority**: P1
- **Effort**: L (roughly 7–12 focused engineering days)
- **Risk**: HIGH — moves the versioned output contract and adds a second runtime
- **Depends on**: none; plans 001–009 are already DONE
- **Category**: migration / perf / tests
- **Planned at**: commit `039b112`, 2026-07-12

## Why this matters

The local SolidJS app currently parses both `.uasset` and `.utrace` files by
uploading bytes to Vite middleware, writing a temporary file, and spawning the
native `uasset` CLI. Both Rust library feature sets already compile for
`wasm32-unknown-unknown`, so the parser can also run wholly inside a browser
worker. Keeping both paths selectable provides a safe fallback and makes it
possible to compare native process/file overhead with WASM transfer, parse,
serialization, and result-transfer costs using identical inputs and options.

This plan must preserve the CLI's versioned JSON contract. Do not create an
independent WASM DTO tree: extract one shared contract implementation and make
the CLI and WASM facade call it.

## Product decisions fixed by this plan

- The local app exposes three typed backend modes: `native`, `wasm`, and
  `compare`. Do not represent these as arbitrary strings beyond the UI/storage
  boundary.
- `compare` executes native and WASM sequentially, alternating which backend
  runs first between repetitions. Do not run them concurrently: contention
  would make the measurements misleading.
- Compare mode reports each backend result and timing, plus parity status. It
  renders the preferred result only after parity checking; default preferred
  result is native while this feature is experimental.
- Output parity is semantic JSON parity after removing only explicitly
  nondeterministic envelope fields (`path`) and timing metadata. Array order,
  numeric values, status, truncation flags, schema versions, decoded assets,
  and errors must match exactly.
- The comparison UI supports UAsset inspect and these UTrace operations:
  inventory, dashboard, dashboard-with-selected-CPU-frame, and
  dashboard-with-selected-GPU-frame.
- Capture-wide CPU range queries backed by `.utix` remain native-only in this
  plan. `src/utrace_timeline.rs` uses `std::fs`, `File`, `Seek`, and atomic
  rename semantics. The UI must disable the WASM/compare option for that
  operation with a capability explanation; it must not silently switch
  backends or label incomparable work as a comparison.
- PDB symbolization remains native-only. A request with symbol paths must be
  rejected by the WASM facade as an unsupported capability.
- WASM parsing runs in a dedicated module Web Worker. Never parse on the UI
  thread.
- The first version may copy `File -> ArrayBuffer -> WASM linear memory`, but
  must expose transfer/copy/parse/serialize/return phases separately and apply
  an explicit input-size guard before allocating in WASM.

## Current state

- `Cargo.toml` defines one library and the `uasset` binary. Features are
  `uasset`, `utrace`, and native-oriented `utrace-symbols`; there is no WASM
  facade or `wasm-bindgen` dependency.
- `src/package.rs:326` exposes `Package::parse(source: &[u8])`.
- `src/utrace.rs:1846`, `:1861`, and `:3249` expose byte-slice based inspect,
  inventory, and configurable dashboard functions. The parser core therefore
  does not require browser filesystem access.
- `src/bin/uasset.rs:2520+` privately owns `InspectOutput`, UTrace envelope
  structs, `InspectOutput::from_package`, asset/property conversion DTOs, and
  JSON rendering. This is the current schema-versioned integration contract.
- `src/bin/uasset.rs:710+` duplicates orchestration around reading bytes,
  parsing, building those output envelopes, serializing, and mapping errors to
  exit codes. Keep CLI argument parsing, filesystem/stdin/stdout, HTML/text
  rendering, and exit-code mapping in the binary.
- `web/vite.parse-plugin.ts:159` spawns the CLI;
  `web/vite.parse-plugin.ts:322` writes uploads to a temporary file. It already
  returns detailed `X-Ue-Parse-Timing` fields and caches `.utix` sidecars.
- `web/src/lib/api.ts:126+` contains the native HTTP transport and exports
  `inspectUasset`, `utraceDashboard`, `utraceInventory`, and `utraceTimeline`.
  Preserve these native functions as a transport implementation rather than
  deleting them.
- `web/src/routes/Uasset.tsx` and `web/src/routes/Utrace.tsx` call the API
  functions directly. Route code should depend on a backend dispatcher, not
  know worker protocol details.
- `web/src/lib/types.ts` manually defines the JSON contract consumed by the UI.
  Do not casually rename or widen these stable result shapes.
- `src/utrace_timeline.rs:1-8` explicitly owns a bounded disk-backed timeline
  index and imports native filesystem/seek APIs. It is out of the WASM build.
- `tests/tiny_corpus.rs` has committed tiny `.uasset` and `.utrace` fixtures;
  `tests/cli.rs` asserts the CLI JSON contract. Reuse these bytes for cross-
  backend parity tests.
- Repository parser discipline from `AGENTS.md` applies: all file-provided
  counts remain bounded before allocation, recursion remains bounded, enums
  replace stringly dispatch, mutually-exclusive output uses tagged enums, and
  fixture tests never silently assert nothing.

The following compile probes succeeded at planning time:

```text
cargo check --lib --target wasm32-unknown-unknown --no-default-features --features uasset
cargo check --lib --target wasm32-unknown-unknown --no-default-features --features utrace
```

## Target architecture

```text
SolidJS route
    -> typed ParserBackend dispatcher (native | wasm | compare)
       -> native transport -> Vite POST middleware -> native CLI
       -> wasm transport   -> module Worker -> wasm-bindgen facade -> shared output contract
       -> compare          -> sequential native + wasm calls -> normalized parity + timing report
```

Use a discriminated union for requests and results. The operation variant must
carry only its valid options; do not introduce a bag of unrelated optional
flags.

Suggested TypeScript shape (names may change, invariants may not):

```ts
export type ParserBackend = "native" | "wasm" | "compare";

export type ParseOperation =
  | { kind: "uasset-inspect" }
  | { kind: "utrace-inventory" }
  | { kind: "utrace-dashboard"; options: UtraceDashboardQuery }
  | { kind: "utrace-timeline"; options: UtraceTimelineQueryOptions };

export type BackendRun<T> = {
  backend: "native" | "wasm";
  data: T;
  timing: ParseTiming;
};

export type ParseRun<T> =
  | { mode: "single"; run: BackendRun<T> }
  | {
      mode: "compare";
      preferred: BackendRun<T>;
      native: BackendRun<T>;
      wasm: BackendRun<T>;
      parity: { equal: boolean; first_difference?: string };
    };
```

The WASM boundary should return a UTF-8 JSON string plus a small timing record,
not a deeply converted `JsValue`. This preserves exact serde JSON semantics and
makes JSON parsing an explicit measurable phase on both paths.

## Timing contract

Extend `ParseTiming` without deleting current native fields. Every run must
identify `backend` and provide `client_ms` (total wall time). Use optional
backend-specific phases:

| Field | Native | WASM | Meaning |
|---|---:|---:|---|
| `input_read_ms` | optional | required | `File.arrayBuffer()` time |
| `worker_startup_ms` | — | required on first call | worker + module initialization |
| `transfer_to_worker_ms` | — | required | request until worker receives buffer |
| `wasm_copy_ms` | — | required | copying bytes into WASM-visible input |
| `parse_ms` | CLI timing may alias | required | Rust parse + DTO construction |
| `serialize_ms` | included in CLI unless instrumented | required | Rust `serde_json` serialization |
| `transfer_from_worker_ms` | — | required | worker response delivery |
| `json_parse_ms` | required | required | browser JSON parse |
| existing server/write/CLI/index/query fields | required where applicable | — | preserve current native diagnostics |

Use `performance.now()` for browser/worker phases. In Rust WASM, either accept
timestamps from JavaScript or add a target-specific monotonic clock dependency;
do not use wall-clock time. Document that single-run figures include warmup.

For compare mode add configurable repetitions with a conservative default of
three and a hard maximum of ten. Report all samples and median; do not report
only an average. Alternate execution order per repetition and show cold-start
WASM initialization separately from warm parse samples.

## Commands you will need

| Purpose | Command | Expected on success |
|---|---|---|
| Rust format | `cargo fmt --check` | exit 0, no diff |
| Rust lint | `cargo clippy --all-targets --all-features -- -D warnings` | exit 0 |
| Rust tests | `cargo test --all-targets --all-features` | all tests pass |
| WASM compile | `cargo check --lib --target wasm32-unknown-unknown --no-default-features --features wasm` | exit 0 |
| Web install | `cd web && npm install` | exit 0; lockfile remains consistent |
| Web build | `cd web && npm run build` | TypeScript and Vite exit 0 |
| Local run | `cargo build --release --features utrace; cd web && npm run dev` | Vite serves both backends |

If the implementation uses `wasm-pack`, add a deterministic npm script such as
`build:wasm`; the ordinary `npm run build` must invoke or validate the generated
bindings rather than depending on uncommitted generated output.

## Scope

**In scope** (modify/create only as needed):

- `Cargo.toml`, `Cargo.lock`
- `src/lib.rs`
- `src/output.rs` (create; shared JSON contracts and orchestration)
- `src/wasm.rs` (create; thin `wasm-bindgen` facade)
- `src/bin/uasset.rs`
- `tests/cli.rs`, `tests/tiny_corpus.rs`
- `web/package.json`, `web/package-lock.json`
- `web/vite.config.ts`, `web/vite.parse-plugin.ts`
- `web/src/lib/api.ts`, `web/src/lib/types.ts`
- `web/src/lib/parser-backend.ts` (create)
- `web/src/lib/wasm-worker.ts` (create)
- `web/src/lib/wasm-worker-client.ts` (create)
- `web/src/routes/Uasset.tsx`, `web/src/routes/Utrace.tsx`
- `web/src/index.css`, `web/README.md`, root `README.md`
- focused browser/worker test files and checked-in WASM build configuration
- CI workflow only if one exists by execution time
- `plans/README.md` status only

**Out of scope**:

- Replacing or removing the native Vite/CLI path
- Making `.utix` filesystem indexing browser-compatible
- Browser PDB discovery or symbolization
- WASM threads, `SharedArrayBuffer`, or cross-origin isolation
- Streaming/chunked UTrace parsing
- Changing existing schema versions or JSON field names merely to simplify WASM
- Relaxing any `Reader` allocation/depth limits
- Publishing packages, deploying the web app, or committing generated benchmark results

## Git workflow

- Suggested branch: `advisor/010-browser-wasm-native-comparison`
- Preserve unrelated and pre-existing working-tree changes.
- Match the repository's imperative commit style, e.g. `Add bounded callstack catalog...`.
- Commit by logical unit: shared contract; WASM facade; worker/dispatcher;
  comparison UI; parity/performance tests and docs.
- Do not push or open a PR unless explicitly instructed.

## Steps

### Step 1: Extract the versioned output contract from the CLI

Create `src/output.rs`. Move—not copy—the serializable UAsset and UTrace JSON
envelopes, asset/property DTO conversions, schema constants, and byte-oriented
orchestration out of `src/bin/uasset.rs`. Expose narrowly scoped functions that
accept `(path_label: &str, source: &[u8], typed options)` and return either a
serializable output or a typed contract error. Include a typed partial-success
state for UAsset rather than making callers infer it from an unrelated flag.

Keep text/HTML rendering, CLI flags, filesystem/stdin/stdout, native timeline
index/query, PDB path handling, and exit-code selection in the binary. Update
the CLI to call the shared functions. Do not make all DTO internals public;
expose only what the WASM facade and contract tests require.

Add Rust tests that run the shared UAsset inspect and UTrace operations on the
committed tiny fixtures and compare `serde_json::Value` with CLI stdout after
normalizing only `path`.

**Verify**:

```text
cargo test --test tiny_corpus --all-features
cargo test --test cli --all-features
```

Expected: exit 0; existing schema versions and fields are unchanged, and new
shared-contract-versus-CLI parity assertions pass.

### Step 2: Add a browser-only WASM facade

Add a `wasm` feature that implies `uasset`, `utrace`, and target-specific
`wasm-bindgen` support, but never implies `utrace-symbols`. Gate `src/wasm.rs`
with both the feature and `target_arch = "wasm32"`; fail clearly at compile time
if the feature is misconfigured rather than leaking browser types into native
code.

Export one typed-dispatch entry point or a small set of operation-specific
functions. Inputs are bytes, filename/path label, and bounded options. Outputs
contain the exact shared-contract JSON string and Rust-side parse/serialize
timings. Map malformed, unsupported, resource-limit, partial, and internal
states into a stable tagged error payload. Do not expose raw Rust panic text.

Enforce a named maximum input length before copying/decoding. Choose the limit
from measured fixture sizes and document it; it must be configurable at build
time or centralized in one constant, and rejection must distinguish UAsset and
UTrace limits if they differ.

**Verify**:

```text
cargo check --lib --target wasm32-unknown-unknown --no-default-features --features wasm
cargo clippy --lib --target wasm32-unknown-unknown --no-default-features --features wasm -- -D warnings
```

Expected: both exit 0. `cargo tree --target wasm32-unknown-unknown --features wasm`
must not contain `pdb-addr2line`.

### Step 3: Establish a reproducible WASM build in Vite

Add deterministic scripts and dependencies to `web/package.json`. Generate or
bundle the WASM bindings into an ignored build directory during `npm run dev`
and `npm run build`; do not require developers to remember a manual pre-step.
Configure Vite to load the module from a module Worker without inlining a large
WASM binary into application JavaScript.

Keep `ueParserApiPlugin()` registered so native mode still works. Update
`web/README.md` with prerequisites, exact commands, supported operations,
limits, and a troubleshooting note for missing Rust WASM tooling.

**Verify**: `cd web && npm run build`

Expected: exit 0 from TypeScript, WASM generation, and Vite; the output contains
one `.wasm` asset and a worker chunk, and the native API plugin still compiles.

### Step 4: Implement the worker protocol and WASM transport

Create a discriminated request/response protocol in
`web/src/lib/wasm-worker.ts` and a lifecycle-owning client in
`web/src/lib/wasm-worker-client.ts`. Initialize the WASM module once per worker.
Transfer the input `ArrayBuffer` rather than cloning it. Assign monotonically
increasing request IDs and ignore stale responses after a new file selection.

The client must support cancellation by terminating and lazily recreating the
worker. Ensure every pending promise settles on worker error or termination.
Return structured `ParseRequestError` equivalents so route error rendering is
backend-independent.

Measure all phases listed in the timing contract. Worker startup/module compile
time must be reported separately and must not be silently folded into warm
parse time.

Add focused tests for request/response discrimination, stale result rejection,
worker failure, cancellation, oversized input, malformed parser input, and
successful tiny UAsset/UTrace operations. Prefer a real generated WASM module
for contract tests; mock only Worker lifecycle mechanics that a unit test cannot
exercise reliably.

**Verify**: `cd web && npm run build`

Expected: exit 0 with no unused protocol variants and no TypeScript casts from
`unknown` directly to successful parser results without boundary validation.

### Step 5: Add the typed backend dispatcher and semantic comparison

Create `web/src/lib/parser-backend.ts`. Move the route-facing operation API
behind this dispatcher while retaining `api.ts` as the native implementation.
Implement `native`, `wasm`, and `compare` as an enum/discriminated union resolved
at the UI/local-storage boundary.

For compare mode:

1. Read the file once into a source buffer outside the measured backend parse
   phases.
2. Create independent transferable copies outside each backend's parse timer;
   report copy time rather than hiding it.
3. Run backends sequentially for 1–10 repetitions, alternating order.
4. Normalize only allowed nondeterministic fields and recursively compare JSON.
5. Return the first differing JSON path and compact native/WASM values when
   unequal; cap diagnostics so a large payload is never duplicated in UI state.
6. Store raw timing samples and calculate median per phase.

Do not use `JSON.stringify(a) === JSON.stringify(b)` unless object keys are
canonicalized first. Array order remains significant.

Add unit tests for parity normalization, object-key ordering, array-order
mismatch, numeric mismatch, missing fields, bounded difference diagnostics,
median calculation, and alternating backend order.

**Verify**: `cd web && npm run build`

Expected: exit 0; dispatcher tests pass; TypeScript exhaustiveness checks cover
every operation/backend combination.

### Step 6: Expose backend and comparison controls in both routes

Add a shared compact backend selector or equivalent consistent UI to
`Uasset.tsx` and `Utrace.tsx`. Options are Native, WASM, and Compare. Persist the
selection locally, defaulting to Native until comparison has proven parity.
Compare mode adds repetitions (1–10, default 3), a parity indicator, per-backend
median total and parse times, and expandable phase/sample details.

Keep the primary decoded view unchanged. When outputs differ, show a visible
parity failure and render the native result as preferred; never merge two
outputs. For unsupported operations, disable invalid modes and explain why:
capture-range timeline queries and PDB symbolization are native-only. Selected
CPU/GPU frame dashboard requests remain comparable.

Prevent stale runs from replacing current results when the user changes file,
backend, or options. Disable controls that would start overlapping measurements
while compare mode is running; expose cancel, which terminates the worker and
abandons the pending native response without pretending the native process was
killed.

**Verify**: `cd web && npm run build`

Expected: exit 0; both routes compile and every existing native action remains
reachable.

### Step 7: Add end-to-end parity and measurement smoke tests

Add an automated browser smoke test if the repository already has or can add a
lightweight browser-test dependency without introducing a second test stack.
It must open the local Vite app, select Compare, load the committed tiny UAsset
and UTrace fixtures, and assert successful native and WASM runs, equal semantic
output, timing presence, and no console errors.

If spawning a browser is not portable in CI, add a deterministic Node/Vitest
integration harness around the built worker plus a manual smoke checklist in
`web/README.md`. Do not claim browser E2E coverage if only unit tests exist.

Run a manual representative-trace comparison and record only methodology and
observed peak memory/phase availability in the PR description—not benchmark
numbers in source docs. Verify that a large rejected trace produces a resource-
limit error rather than a tab crash.

**Verify**:

```text
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
cargo check --lib --target wasm32-unknown-unknown --no-default-features --features wasm
cd web && npm run build
```

Expected: every command exits 0. The browser/integration command added by the
executor also exits 0 and is documented in `web/README.md`.

## Test plan

- Rust shared-contract tests:
  - tiny valid UAsset inspect equals CLI JSON;
  - partial UAsset retains exit/status semantics and decode errors;
  - malformed and unsupported UAsset map to the same error category;
  - tiny UTrace inventory and dashboard equal CLI JSON;
  - dashboard bounds and CPU/GPU selected-frame options survive the facade;
  - WASM input limits reject before parser allocation;
  - `utrace-symbols` is absent from the browser feature graph.
- Worker/transport tests:
  - cold initialization and warm reuse;
  - transferable buffer handling;
  - cancellation, stale responses, and worker crash;
  - structured malformed/resource-limit errors;
  - phase timings are non-negative and required fields are present.
- Comparison tests:
  - exact semantic equality despite object-key insertion order;
  - mismatch paths for scalar, missing key, and array order;
  - only `path`/timing normalization is permitted;
  - median and alternating order for odd/even repetitions;
  - diagnostics remain bounded.
- UI/integration tests:
  - native remains default and functional;
  - WASM handles tiny UAsset and UTrace;
  - Compare shows parity and both timing sets;
  - range timeline forces/explains native-only capability;
  - changing file during a run cannot display stale output.

## Done criteria

- [ ] CLI JSON for committed UAsset and UTrace fixtures is unchanged.
- [ ] `cargo fmt --check` exits 0.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` exits 0.
- [ ] `cargo test --all-targets --all-features` exits 0.
- [ ] Browser WASM feature compiles and excludes `pdb-addr2line`.
- [ ] `cd web && npm run build` builds WASM, worker, and SolidJS app from a clean checkout.
- [ ] UAsset inspect runs in Native, WASM, and Compare modes.
- [ ] UTrace inventory/dashboard/selected-frame requests run in all three modes.
- [ ] Compare mode alternates sequential order, retains samples, reports medians, and checks semantic parity.
- [ ] Native `.utix` range queries still work and are clearly native-only.
- [ ] Oversized/malformed input fails structurally without freezing the UI thread.
- [ ] Worker cancellation/stale-response tests pass.
- [ ] No parser output DTO or conversion logic is duplicated between CLI and WASM.
- [ ] Documentation states prerequisites, limits, comparable operations, and timing semantics.
- [ ] No unrelated pre-existing working-tree changes were overwritten.
- [ ] Plan 010 status in `plans/README.md` is updated to DONE.

## STOP conditions

Stop and report rather than improvising if:

- Extracting the CLI output contract would require changing existing schema
  versions or field meanings.
- Native and shared-contract JSON differ for committed fixtures after removing
  only `path`; diagnose before adding WASM.
- A required parser dependency cannot compile for `wasm32-unknown-unknown`
  without weakening parser limits or introducing unsafe code.
- Representative UTrace input cannot fit under the browser's observed WASM
  memory ceiling even with existing configured bounds. Report file size, peak
  memory estimate, and failing phase without including proprietary trace data.
- Vite cannot reproducibly build bindings without committing machine-specific
  generated paths or binaries.
- Full parity unexpectedly requires browser `.utix` filesystem semantics. Keep
  range query native-only and report the newly discovered coupling.
- Work overlaps uncommitted user changes in a way that cannot be preserved.
- Any verification command fails twice after a reasonable correction.

## Maintenance notes

- Reviewers should scrutinize shared-contract extraction most closely; the CLI
  schema is the repository's published integration contract.
- Timing comparisons are diagnostic, not scientific benchmarks. Browser JIT,
  WASM compilation, filesystem cache, native process startup, and execution
  order must remain visible in the data.
- If browser range queries become valuable, follow up by extracting the `.utix`
  codec/query engine from filesystem ownership and designing OPFS or bounded
  in-memory storage. Do not retrofit that into this plan.
- If traces routinely exceed the input guard, investigate streaming/chunked
  parsing before simply raising the limit.
- WASM threads and PDB support are separate capability projects with different
  deployment/security requirements.

