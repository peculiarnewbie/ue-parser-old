# Code Quality Review Plan

This plan tracks the implementation work from the code quality review. The
priority is to preserve the parser's existing low-level discipline while making
the upper layers use the same safety, typing, and testing standards.

## 0. Lock Current Baseline

- Keep the recent correctness fixes as the first slice.
- Record the baseline verification commands:
  - `cargo fmt`
  - `cargo clippy --all-targets --all-features -- -D warnings`
  - `cargo test --all-targets --all-features`

## 1. Finish Remaining Actual Bugs

- Confirm current fixes cover:
  - package-index cycle detection
  - recursion depth caps
  - `ByteProperty` container decoding
  - checked allocations
  - legacy-version `unreachable!`
- Still address:
  - [x] strict UTF-16 failure policy in `utrace.rs`
    - Completed: wide trace sample decoding now rejects invalid UTF-16 instead
      of silently surfacing it as a raw sample.
  - [x] error taxonomy for `ArchiveErrorKind::AllocationLimit` versus malformed data
    - Completed: allocation-limit archive failures now map to `ResourceLimit`
      parser kinds and CLI JSON kind `resource_limit`, distinct from malformed
      input.
- [x] Add corrupt and truncated input tests for package parse and utrace parse paths.
  - Completed: committed tiny corpus fixtures now have parser-level mutation
    tests covering truncated bytes and corrupt magic for `Package::parse` and
    utrace inspect.

## 2. Make Tests Trustworthy

- [x] Stop silent green fixture skips.
- [x] Convert optional fixture tests to either:
  - explicit `#[ignore]` fixture tests, or
  - CI-required fixture tests with clear environment setup.
  - Completed: real fixture-dependent package, CLI, electroswag, and utrace
    tests are now explicit ignored tests, and their fixture helpers fail loudly
    when those ignored tests are run without configured fixtures.
- [x] Commit a tiny fixture corpus for always-on integration coverage.
  - Completed: added reviewable hex fixtures for a minimal current UE5 package
    summary and a minimal utrace prologue/thread trace, with always-on library
    and CLI integration tests.
- [x] Add CLI-level malformed input tests.
  - Completed: added stdin-driven JSON error contract tests for malformed
    package and utrace inputs, asserting stderr output and exit code 2.
- [x] Add fuzz targets for:
  - `Package::parse`
  - tagged property stream parsing
  - utrace header and packet parsing
  - Completed: added a `cargo-fuzz` crate with `package_parse`,
    `property_stream`, and `utrace_packets` targets.

## 3. Centralize Allocation And Count Handling

- [x] Audit all `Vec::with_capacity` calls.
  - Completed: reviewed package, property, asset, codec, archive, utrace,
    example, and test capacity sites; derived/in-memory capacities are left
    outside the file-count allocation path.
- [x] Replace raw capacity calls from file-controlled counts with `Reader` helpers.
  - Completed: parser allocations from serialized counts now allocate from
    checked capacity values or explicit local caps before `Vec::with_capacity`.
- [x] Add test cases for absurd counts in package, asset, codec, and utrace paths.
  - Completed: added regression tests for name-map counts, DataTable row
    counts, decoded array payload counts, and utrace NewEvent field counts.
- [x] Treat unchecked file-driven capacity as a review blocker.
  - Completed: the plan now treats unchecked serialized-count allocation as a
    blocker, backed by the centralized helper changes and regression tests above.

## 4. Add Lazy Error Context

- [x] Design a small path/context abstraction for `Reader`.
  - Completed: `Reader` and `ArchiveError` path inputs now accept
    `Display`, allowing lazy `format_args!` path construction without allocating
    on successful reads.
- [x] Avoid eager `format!` on successful reads.
  - Completed: migrated the first hot `Reader` loops to lazy
    `format_args!` paths.
- Migrate hot loops first:
  - [x] name map
  - [x] property streams
  - [x] curve keys
  - [x] wide strings
  - [x] utrace event loops
- [x] Measure allocation behavior before and after if practical.
  - Assessed: no allocator instrumentation or parser benchmark harness exists
    yet, so this slice was verified with targeted code review plus
    fmt/clippy/tests rather than a noisy one-off measurement.

## 5. Replace Stringly Dispatch

- Add enums for known domains:
  - `PropertyKind`
  - `TraceEventKind`
  - JSON status/kind enums where stable contract matters
- Resolve strings once at boundaries.
- Update property decoding to match on `PropertyKind`.
- Update utrace event routing to avoid repeated string-table scans and tuple
  string matches.

## 6. Deduplicate Repeated Helpers

- Extract shared helpers for:
  - archive bool decoding
  - name-reference validation and resolution
  - asset class preambles
  - important-event stream walking
  - CLI render and command drivers
- Remove duplicated error-output structs where one shared shape works.

## 7. Strengthen Domain Types

- Replace sentinel primitives with domain types where they reduce invalid states:
  - `parent_index: Option<_>` instead of a documented `-1`
  - thread-id newtypes or one consistent integer width
  - named structs for `(count, cycles)` aggregations
  - flags newtypes for raw `u32` flag fields
- Apply these where they remove repeated conversion code or impossible states.

## 8. Fix Flag-Bag Output Shapes

- Convert `AssetOutput` to a `#[serde(tag = "kind")]` enum.
- Align `PropertyOutput::from_record` and `value_output` for raw size semantics.
- Decide which JSON structs are stable CLI contract versus internal library
  shapes.
- Require schema-version bumps for stable output changes.

## 9. Split `utrace.rs`

- Split by ownership:
  - `types`
  - `error`
  - `transport`
  - `registry`
  - `cbor`
  - `cpu`
  - `gpu`
  - `dashboard`
  - `coverage`
- Move tests with the module they validate where practical.
- Keep the public API stable during the split.

## 10. Reduce Utrace Copies

- Make normal event streaming use borrowed `RawEvent<'_>`.
- Only materialize owned payloads at API and output seams.
- Avoid reparsing aux payloads multiple times per event.
- Add memory-oriented regression tests or benchmarks for large traces.

## 11. Public API Future-Proofing

- Add `#[non_exhaustive]` to public enums before release.
- Revisit `From<ArchiveError>` mappings so allocation/resource-limit errors are
  not reported as generic corruption.
- Document semver expectations for parser enums and CLI JSON.

## Suggested Order

1. Test reliability.
2. Remaining actual bugs.
3. Allocation audit.
4. Lazy error context.
5. String dispatch enums.
6. Deduplication.
7. Output, type, and API cleanup.
8. Utrace split and borrowed event model.
