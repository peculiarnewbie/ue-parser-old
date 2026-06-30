# UTrace coverage matrix

Audience: future agents working on the UTrace parser. This is planning and
orientation context, not part of the CLI output contract.

Status meanings:

- `parsed`: decoded directly from the trace into a stable parser structure.
- `partial`: some useful data is decoded, but Unreal Insights provider semantics
  are incomplete.
- `derived`: computed by this parser from lower-level trace events; not stored
  as ready-made tables in the `.utrace` file.
- `not parsed`: known event family, not decoded yet.

| Area | Status | Source events | Current outputs | Notes |
| --- | --- | --- | --- | --- |
| Container | parsed | Trace header + TidPacket stream | `header`, packet summary, thread streams | Reads TRC2/TRCE headers, TidPacket transport packets, sync packets, and LZ4 blocks. |
| Event registry | parsed | `$Trace.NewEvent` | `events`, field declarations | Builds the Protocol 5/6/7 event type registry used by later decoders. |
| Trace prologue | parsed | `$Trace.NewTrace` | `start_cycle`, `cycle_frequency`, `pointer_size`, `start_date_time` | Used for cycle-to-seconds conversion. |
| Threads | partial | `$Trace.ThreadInfo` | `thread_info`, `cpu.threads` | Thread names and ids are decoded; thread groups are not decoded yet. |
| CPU profiler | partial | `CpuProfiler.EventSpec`, `CpuProfiler.EventBatchV3` | `cpu.specs`, `cpu.scopes`, `cpu.threads` | Plain scope specs and V3 scope batches are decoded and aggregated. Metadata scopes, coroutine restoration, and exact Insights provider semantics are incomplete. |
| Frames | partial | `Misc.BeginFrame`, `Misc.EndFrame` | `frames` | Raw frame markers are decoded. Frame windows and per-frame CPU attribution are not emitted yet. |
| Dashboard aggregates | derived | `CpuProfiler.EventBatchV3`, `Misc.BeginFrame`, `Misc.EndFrame` | `cpu.scopes`, `cpu.threads` | These summaries are computed by this parser from lower-level trace events. |
| Counters | not parsed | `Counters.Spec`, `Counters.SetValueInt`, `Counters.SetValueFloat` | none | Counter declarations and time series are not decoded yet. |
| GPU profiler | not parsed | `GpuProfiler.*` | none | GPU queues, GPU timing events, and CPU/GPU calibration are not decoded yet. |
| Bookmarks/regions | not parsed | `Misc.Bookmark*`, `Misc.Region*` | none | Bookmark and region timelines are not decoded yet. |
| Memory | not parsed | `Memory.*`, `LLM.*` | none | Memory and LLM tag streams are not decoded yet. |
| Loading/assets | not parsed | `LoadTime.*`, `AssetMetadata.*`, `Object.*` | none | Asset, object, package, and load-time analyzers are not decoded yet. |
| IO/file | not parsed | `IoStore.*`, `File.*` | none | File activity and IO store events are not decoded yet. |
| Logs/diagnostics | not parsed | `Logging.*`, `Diagnostics.*` | none | Log messages and session diagnostics beyond the trace prologue are not decoded yet. |
| Callstacks | not parsed | `Callstack.*` | none | Callstack symbols and stack references are not decoded yet. |

## Current dashboard subset

The CLI dashboard is intentionally narrower than the full matrix. Today it
emits:

- trace header and prologue
- thread info
- CPU scope specs
- global CPU scope summaries
- per-thread CPU scope summaries
- raw frame markers

Do not add the full coverage matrix back into the CLI JSON unless there is a
runtime consumer that needs it. Prefer keeping parser capability notes here and
keeping `utrace dashboard` focused on decoded performance data.
