# Progressive UTrace protocol

The native dashboard endpoint uses newline-delimited JSON. Every line is one
complete protocol event and stdout never contains diagnostics.

- `protocol_version` is currently `1`.
- `sequence` is an unsigned, monotonically increasing integer scoped to one
  request. Consumers discard events whose sequence is not newer.
- Byte and packet totals originate as Rust `u64` values. Browser adapters reject
  values outside JavaScript's safe integer range.
- `bootstrap` contains the validated trace header and whatever declaration,
  prologue, and bounded thread metadata has been decoded so far.
- `snapshot` contains a tagged coherent replacement patch. Version 1 defines
  `transport` for packet/thread totals and `frames` for the current bounded set
  of completed frame-marker timings. Frame patches include absolute totals and
  truncation state, so consumers replace rather than append blindly.
- `complete` contains the unchanged dashboard envelope and the inventory
  envelope produced from the same packet decode. When the caller requested an
  index, it also includes `timeline_index` (the bounded sidecar metadata and
  path and metadata). The native middleware adds its content-addressed
  `source_hash` after atomically placing the sidecar in its cache. A
  sidecar-write failure is reported as `timeline_index_warning` and does not
  change the successful dashboard completion.
- `failed` is terminal and contains a displayable error. Diagnostics remain on
  stderr.

The native middleware pipes request chunks directly into the Rust CLI's stdin;
it does not wait for upload completion or create a temporary capture. Parser
stdout is consumed concurrently, so decoded bootstrap and transport snapshots
can reach the page while later request chunks are still arriving. Middleware
rewrites all event sequence numbers into one request-wide sequence and applies
backpressure in both directions. Individual lines are capped at 64 MiB.

The incremental Rust session caps caller chunks at 1 MiB, buffers at most an
incomplete header or `u16`-sized packet, caps decoded thread streams at 1 GiB,
and retains at most 4,096 thread rows in bootstrap snapshots. Final dashboard
and inventory contracts retain their existing independent bounds and shapes.

The WASM adapter exposes the same Rust session through `wasm_bindgen`.
`File.stream()` is read on the page side and at most one bounded transferable
chunk is outstanding: the next read waits for the worker to acknowledge that
Rust consumed the previous chunk. Bootstrap, frame, and transport events are
posted between acknowledgments; cancellation drops the worker session.
