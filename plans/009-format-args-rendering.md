# Plan 009: Render typed FormatArgs for bookmarks and logs

> **Executor instructions**: Follow this plan step by step. Run every verification
> command before continuing. Update this plan's row in `plans/README.md` when done.

## Status

- **Status**: DONE (implementation complete pending review)
- **Priority**: P1
- **Effort**: M
- **Risk**: LOW/MED (wire format is documented; full printf parity is intentionally subset)
- **Depends on**: none (independent of 004–008)
- **Category**: direction
- **Planned at**: commit `d0a7466`, 2026-07-12

## Why this matters

Bookmarks and log points already carry `FormatArgs` blobs and format strings, but
the dashboard only counted bytes (bookmarks) or did a heuristic `%s` scrape
(logs). Hitch markers like `Loading %s` are useless without the substituted
path. Insights formats both through `FFormatArgsHelper`.

## Scope

**In scope**:

- `src/utrace_format_args.rs` — typed decode + render (`%s/%d/%u/%x/%p/%f/…`, `%%`)
- Wire bookmarks + logs to shared renderer; keep heuristic fallback
- `BookmarkSummary.sample_args` / `sample_message`
- Coverage matrix + plans README

**Out of scope**:

- Full glibc printf width/precision parity for exotic specs
- Per-event annotation/log timelines
- Callstack joins on bookmark `CallstackId` (plan 006)

## Acceptance

- Typed stream `[count][type_codes…][payload…]` decoded without panic on truncation
- Bookmark dashboard emits rendered `sample_message` for `"Loading %s"` + wide arg
- Log dashboard uses the same path; heuristic `%s` still works for non-typed blobs
- Unit tests cover int/float/string/%%/malformed/heuristic fallback

## Verification

```text
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-targets --all-features
```
