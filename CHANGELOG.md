# Changelog

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
