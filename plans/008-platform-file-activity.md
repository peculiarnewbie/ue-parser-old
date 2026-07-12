# Plan 008: Decode bounded PlatformFile activity

> **Executor instructions**: Follow this plan step by step. Run every verification
> command before continuing. Update this plan's row in `plans/README.md` when done.

## Status

- **Status**: DONE (implementation complete pending review)
- **Priority**: P1
- **Effort**: M
- **Risk**: LOW (fixed field layout; Insights pairing rules are well documented)
- **Depends on**: none (independent of plans 004–007)
- **Category**: direction
- **Planned at**: commit `d0a7466`, 2026-07-12

## Why this matters

Coverage-matrix gap #5 (asset-loading critical paths) needs filesystem I/O
evidence alongside LoadTime and IoStore. UE 5.7 emits `PlatformFile.*` on
`FileChannel` (`PlatformFileTrace.cpp`); Insights pairs opens by thread and
reads/writes by op handle (`PlatformFileTraceAnalysis.cpp`). Without this
provider, loading hitches that are pure disk stalls stay invisible.

## Scope

**In scope**:

- `src/utrace_platform_file.rs` (new)
- Dashboard types + `EVENT_COVERAGE` + dispatch in `src/utrace.rs`
- `src/lib.rs`
- `tests/utrace_fixture.rs` (explicit zero surface + optional live fixture)
- `memory/utrace-coverage-matrix.md`
- `plans/README.md`

**Out of scope**:

- Joining PlatformFile intervals to LoadTime/IoStore/CPU scopes
- Unbounded full-trace file timelines
- Legacy logger aliases (none in UE 5.7; logger is `PlatformFile`, not `File`)

## Acceptance

- Open/reopen/close paired by thread; read/write paired by handle
- `FileHandle == u64::MAX` counted as failed open
- Bounded path catalog (4,096), open-handle map (65,536), activity samples (40)
- Overflow and unpaired-end counters surfaced
- CPU-frame fixture asserts empty `platform_file` surface
- Unit tests cover happy path, failed open, unpaired ends, sample cap

## Verification

```text
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```
