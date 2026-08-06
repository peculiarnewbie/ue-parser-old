# Agent Guidance

This parser has solid low-level defenses. Keep applying the same discipline in
the UTrace layers. The shared bounded reader and archive errors belong to the
version-pinned UE Shed `uasset-parser` dependency; do not copy them into this
repository.

## Habits To Avoid

- Do not bypass `Reader` limits when allocating from file-provided counts.
  Counts from package headers, property payloads, trace payloads, and table
  metadata must be checked before `Vec::with_capacity`.

- Do not add unbounded recursion driven by file content. Package outer chains,
  property type trees, tagged property streams, field tails, and trace nesting
  need explicit depth limits or cycle detection.

- Do not infer local element shape from a shared container reader. Container
  element decoders should use fixed-size bounded readers when the element wire
  size is known.

- Do not put eager `format!` calls on hot successful read paths when adding new
  parser loops. Prefer existing path helpers, reusable local path strings, or a
  lazy context approach if touching shared reader APIs.

- Do not grow stringly typed dispatch. Resolve file names, property kinds,
  event kinds, and JSON status/kind fields into enums at boundaries when the
  set of values is known.

- Do not copy-paste parser helpers or CLI drivers. Shared byte-reading
  behavior belongs to the `uasset-parser` dependency; trace transport,
  provider aggregation, render dispatch, and trace command plumbing should
  have one implementation here.

- Do not represent domain ids and sentinel states as raw primitives when a
  newtype or `Option` captures the invariant. Follow the existing `Span`,
  `NameRef`, `PackageIndex`, and flag-newtype pattern.

- Do not add flag-bag output structs where variants are mutually exclusive.
  Prefer tagged enums for stable JSON shapes and decoded asset outputs.

- Do not let tests pass silently when they asserted nothing. Fixture-gated tests
  must be explicit skips, explicit `#[ignore]` tests, or required in CI for the
  contract they are meant to protect.

- Do not expand `utrace.rs` as a dumping ground. New trace transport, registry,
  event decoding, aggregation, and dashboard behavior should move toward
  smaller modules with clear ownership.

- Do not duplicate raw event payloads unless ownership is required. Prefer
  borrowed event views for normal trace streaming and copy only at API seams
  that need owned values.

- Do not expose public enums without thinking about future engine variants.
  Parser-facing public enums should usually be `#[non_exhaustive]` before a
  public release.

## Verification Expectations

- Run `cargo fmt`.
- Run `cargo clippy --all-targets --all-features -- -D warnings` for parser or
  CLI changes.
- Run `cargo test --all-targets --all-features` when changing decoding behavior,
  public output contracts, or fixture assertions.
