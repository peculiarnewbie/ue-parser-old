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

Important distinction: `utrace inventory` can generically decode sampled scalar
and string fields for many event families. That is parser visibility, not the
same as implementing the corresponding Unreal Insights provider/analyzer.

## This table is narrative; the machine-checked coverage lives in code

The per-event decode classification (decoded / partial / raw, plus a short note
on what each decoder drops) is the `EVENT_COVERAGE` table in `src/utrace.rs` —
the single source of truth that `decode_status_for` derives from. Do not restate
that per-event mapping here by hand; it drifts. Instead run:

```text
uasset utrace coverage <trace> [--universe <file>] --format json
```

`utrace coverage` cross-references three inputs and computes the gaps for you:

1. **Decoded** — the `EVENT_COVERAGE` table (what we decode, and at what status).
2. **Declared** — what a given trace's `$Trace.NewEvent` registry contains.
3. **Universe** — every `UE_TRACE_EVENT_BEGIN` in an engine tree, harvested by
   `scripts/harvest-ue-trace-events.sh` (best-effort; see the script's caveats).

It reports raw families ranked by observed volume, decoders that never fired for
the trace, and engine events the trace never declared. A stale hand-maintained
list cannot survive next to that command, so keep this file for *area-level*
orientation and rationale, not per-event bookkeeping.

As of the last harvest, the engine universe is ~299 distinct trace events; the
CPU-frame fixture declares 66 of them, of which 5 are decoded, 61 partial, and 0
raw. The fixture currently has no raw declared families. The whole-engine gap
(events no fixture here has exercised) spans loggers this parser does not touch
at all: `Animation.*`, `Audio.*`, `NetTrace.*`, `MassTrace.*`, `Object.*`,
`TaskTrace.*`, `SlateTrace.*`, and more.

The area-level status table below is the human narrative layer on top of that.

| Area | Status | Source events | Current outputs | Notes |
| --- | --- | --- | --- | --- |
| Container | parsed | Trace header + TidPacket stream | `header`, packet summary, thread streams | Reads TRC2/TRCE headers, TidPacket transport packets, sync packets, and LZ4 blocks. |
| Event registry | parsed | `$Trace.NewEvent` | `events`, field declarations | Builds the Protocol 5/6/7 event type registry used by later decoders. |
| Event inventory | parsed | All observed declared events | `inventory.summary`, `inventory.events[].observed_count`, `inventory.events[].samples` | Counts observed event families and stores one generic decoded payload sample per event type. Fixed scalar fields and ANSI/wide aux strings are decoded; arrays and unsupported payloads use raw byte summaries. |
| Unmodeled event families | partial | Any declared event not in `EVENT_COVERAGE` | `unmodeled.events` | Dashboard surfaces raw/unimplemented trace families with observed counts and one generic decoded payload sample per event type (e.g. `Object.*`, `NetTrace.*`, `File.*`) before dedicated provider semantics exist. |
| Trace prologue | parsed | `$Trace.NewTrace` | `start_cycle`, `cycle_frequency`, `pointer_size`, `start_date_time` | Used for cycle-to-seconds conversion. |
| Threads | partial | `$Trace.ThreadInfo`, `$Trace.ThreadGroupBegin`, `$Trace.ThreadGroupEnd`, `$Trace.ThreadTiming` | `thread_info`, `cpu.threads`, `thread_groups`, `trace_timing` | Thread names/ids, base timing, group begin/end totals, and per-thread membership (`groups` / `active_group`) when ThreadInfo is registered inside an open ThreadGroupBegin stack. |
| Frames | partial | `Misc.BeginFrame`, `Misc.EndFrame`, CPU metadata frame scopes, GPU frame boundaries | `frames`, `frame_correlation`, `gpu.frames`, `cpu.timeline`, `gpu.timeline` | Raw frame markers are decoded. Bounded frame correlation joins CPU metadata `Frame N` scopes with queue-local GPU frame buckets by frame number. `frame_correlation` and `gpu` report uncapped frame counts plus truncation status; `--max-frames` controls the retained rows (default 120). Optional `--frame N` and `--gpu-frame N` dashboard output retain capped CPU and queue-local GPU interval timelines respectively; their frame-number spaces remain separate. |
| GPU profiler | partial | `GpuProfiler.Init`, `QueueSpec`, `EventBreadcrumbSpec`, `EventBeginBreadcrumb`, `EventEndBreadcrumb`, `EventBeginWork`, `EventEndWork`, `EventWait`, `EventStats`, `EventFrameBoundary`, `SignalFence`, `WaitFence` | `gpu.version`, `gpu.queues`, `gpu.frames`, `gpu.timeline`, `gpu.work`, `gpu.breadcrumbs`, `gpu.submission_latency`, inventory samples | Dashboard decodes queue specs, breadcrumb field names, typed breadcrumb metadata samples, pairs begin/end work and breadcrumb events, and summarizes waits, stats, fences, frame boundaries, metadata-bearing breadcrumb begins, timestamp bounds per queue, and bounded queue-local frame buckets. `--gpu-frame` emits a bounded work/breadcrumb timeline. Zero-timestamp breadcrumb ends are ignored without touching the open stack (Insights parity). Negative durations are counted but still close the interval. `EventBeginWork` CPU/GPU pairs are summarized as submission latency (GPU-start minus CPU-submit), not clock-domain calibration. Full unbounded GPU timelines are not emitted. |
| CPU profiler | partial | `CpuProfiler.EventSpec`, `CpuProfiler.MetadataSpec`, `CpuProfiler.Metadata`, `CpuProfiler.EventBatchV3`, `CpuProfiler.EndThread` | `cpu.specs`, `cpu.metadata`, `cpu.scopes`, `cpu.threads`, `cpu.end_threads`, `cpu.timeline` | Plain scope specs, metadata specs/records, bounded CBOR metadata values, rendered metadata names, V3 scope batches, and thread-end markers are decoded and aggregated. `EventBatchV3` keeps timer stacks, coroutine stacks, reconstructed cycle state, and late-connect base cycles from enclosing known scope events or `$Trace.NewTrace` start cycle (Insights `ProcessBufferV2` semantics). Preamble→absolute timeline jumps rebase open stack starts so global `cpu.scopes` totals stay span-checked. Optional bounded per-frame CPU timelines are available via dashboard `--frame` and `--timeline-limit`. Full unbounded CPU timelines remain incomplete. |
| Dashboard aggregates | derived | `CpuProfiler.EventBatchV3`, `Misc.BeginFrame`, `Misc.EndFrame` | `cpu.scopes`, `cpu.threads` | These summaries are computed by this parser from lower-level trace events. |
| Counters | partial | `Counters.Spec`, `Counters.SetValueInt`, `Counters.SetValueFloat` | `counters.specs`, `counters.counters`, inventory samples | Dashboard decodes counter specs and summarizes integer/float samples with min/max/latest values plus bounded per-counter sample points. Full unbounded counter time series output is not emitted yet. |
| Bookmarks/regions | partial | `Misc.BookmarkSpec`, `Misc.Bookmark`, `Misc.RegionBegin`, `Misc.RegionBeginWithId`, `Misc.RegionEnd`, `Misc.RegionEndWithId` | `annotations.bookmarks`, `annotations.regions`, inventory samples | Dashboard decodes bookmark specs/events and pairs named/id regions into aggregate summaries. Bookmark `FormatArgs` samples are rendered against the spec format string (typed Insights stream, with heuristic `%s` fallback). Full annotation timelines are not emitted yet. |
| CPU named scopes | partial | `Cpu.Frame` and other `Cpu.<Name>` events | `cpu.named_events` | The `Cpu` logger declares one event type per named CPU marker (e.g. `Cpu.Frame`), each carrying a generic payload sample such as `Name` and optional scalar fields like `SizeInBytes`. This is separate from the `CpuProfiler` spec/batch pipeline; full timeline reconstruction is not emitted yet. |
| Stats | partial | `Stats.Spec`, `Stats.EventBatch2` | `stats.specs`, `stats.groups`, `stats.stats`, `stats.samples` | Spec catalog plus EventBatch2 opcode samples (min/max/latest, ≤40 points, top 64 hot stats). Per-stat aggregation is capped at 4,096 distinct ids and reports `sample_state_overflow`; samples without a decoded spec report through `unresolved_samples`. CPU-frame/targeted studio fixtures currently have catalogs only (`sample_events=0`). |
| CSV profiler | partial | `CsvProfiler.RegisterCategory`, `DefineDeclaredStat`, `DefineInlineStat`, `BeginStat`, `EndStat`, `CustomStatInt`, `CustomStatFloat` | `csv.*`, `csv.duration_samples`, `csv.value_samples` | Catalog plus non-exclusive Begin/End durations and CustomStat samples (bounded). Exclusive CSV nesting and full CSV tables deferred. Studio fixtures currently lack sample events. |
| Tasks | partial | `TaskTrace.Init` / lifecycle / `WaitingStarted` / `WaitingFinished` | `tasks` | Lifecycle counts, bounded named tasks, and wait interval pairing by thread. No full subsequent graph; WaitForTasks CPU-scope overlap correlation deferred. Requires `UTRACE_TASKS_FIXTURE` for live waits. |
| Trace channels | partial | `Trace.ChannelAnnounce`, `Trace.ChannelToggle` | `channels.channels` | Dashboard decodes trace channel declarations, read-only flags, latest enabled state, and toggle counts. Distinct from the `$Trace` logger used for the prologue and threads. |
| Memory + LLM | partial | `Memory.Init`, `TagSpec`, `MemoryScope`, `Alloc*`, `Free*`, `ReallocAlloc*`, `ReallocFree*`; `LLM.TagsSpec`, `TrackerSpec`, `TagSetSpec`, `TagValue` | `memory.init`, `memory.tags`, `memory.scopes`, `memory.allocs`, `memory.llm` | Dashboard validates supported Memory trace versions (1–2), resolves scope tag names, unpacks sizes using `SizeShift`, and summarizes allocation/free/reallocation bytes by root heap. Retained allocation samples (40), outstanding address tracking (262,144 entries), LLM catalogs (4,096 tags), and latest LLM values (4,096 tracker/tag pairs) are explicitly bounded; overflow is surfaced in the dashboard. Allocation-to-current-tag attribution needs scoped Memory event style support and remains deferred. The current provider capture declares no LLM events, so the LLM wire path is synthetic-tested pending a MemTag capture. |
| Loading/assets | partial | `LoadTime.ClassInfo`, `LoadTime.StartAsyncLoading`, `LoadTime.SuspendAsyncLoading`, `LoadTime.ResumeAsyncLoading`, `LoadTime.PackageSummary`, `LoadTime.BeginRequest`, `LoadTime.EndRequest`, `LoadTime.NewAsyncPackage`, `LoadTime.DestroyAsyncPackage` | `loading.classes`, `loading.packages`, `loading.requests`, `loading.async_loading` | Class catalog, package summaries, request begin/end pairing with bounded samples, and async loading start/suspend/resume counts are decoded. Full Unreal Insights load-time analyzer semantics (object graphs, package state machines) are not emitted yet. This fixture currently exercises only `ClassInfo`. |
| IO/file | partial | `IoStore.*`, `PlatformFile.*` | `io_store.*`, `platform_file.*` | IoStore backend catalog and request create/start/complete/fail lifecycle are decoded with bounded samples. PlatformFile open/reopen/close/read/write intervals are paired (Insights thread/handle semantics) into path catalogs, byte totals, and ≤40 activity samples with explicit overflow counters. Full unbounded file timelines and LoadTime↔IoStore↔PlatformFile joins remain deferred. The current CPU-frame fixture declares no IoStore or PlatformFile traffic. |
| Logs/diagnostics | partial | `Logging.LogCategory`, `Logging.LogMessageSpec`, `Logging.LogMessage`, `Diagnostics.Session2` | `logging.categories`, `logging.message_specs`, `logging.verbosity`, `logging.top_categories`, `logging.top_messages`, `session` | Dashboard decodes the log category catalog (name + default verbosity), message specs (log points) resolved to file/line/format/category, per-verbosity spec and message counts, and message counts per log point. It renders typed `FormatArgs` samples (shared with bookmarks) plus heuristic `%s` fallback, and formats the `Diagnostics.Session2` instance id as a GUID. Per-message timelines and `Diagnostics.Session` (v1) are not decoded. |
| Metadata stack | partial | `MetadataStack.ClearScope`, `MetadataStack.SaveStack`, `MetadataStack.RestoreStack` | `metadata_stack`, `cpu.metadata` | Clear-scope events, saved stack ids, restored stack ids, per-id save/restore counts, and restores without an observed save are counted. Per-thread restored metadata contexts are conservatively applied to later plain CPU profiler scopes and surfaced through `cpu.batches.restored_metadata_scopes`; event ordering is preserved across CPU batch flushes. |
| Slate | partial | `SlateTrace.AddWidget` | `slate.widgets` | Widget add events are counted by widget id with cycle bounds. Full Slate analysis is not decoded yet. |
| Callstacks | partial | `Memory.CallstackSpec`, `Diagnostics.ModuleInit` / `ModuleLoad` / `ModuleUnload` | `callstacks`, `modules`, `memory.allocs.samples.callstack`, `annotations.bookmarks[].callstack_samples` | Bounded raw PC catalog + id joins (`none` / `resolved` / `missing` / `catalog_truncated`). Module mapping fills `mapped_frames` (module+offset / unmapped / ambiguous). Optional Windows PDB symbolization via feature `utrace-symbols` + CLI `--symbol-path` (GUID+age checked; no network). |

## Current dashboard subset

The CLI dashboard is intentionally narrower than the full matrix. Today it
emits:

- trace header and prologue
- thread info (with optional thread-group membership)
- thread group summaries
- TaskTrace lifecycle/wait summaries when present
- CSV profiler category/stat summaries with sample-event counts and bounded duration/value samples
- Stats catalog plus EventBatch2 sample summaries when present
- Counters with bounded sample points
- raw/unmodeled declared event families with counts and representative decoded samples
- thread timing base timestamps
- CPU scope specs
- CPU metadata spec/record summaries with field-name strings, typed metadata samples, rendered names, rendered-scope summaries, and bounded interval samples
- generic `Cpu.*` named event counts and samples
- CPU profiler thread-end markers
- global CPU scope summaries
- per-thread CPU scope summaries
- raw frame markers and bounded CPU/GPU frame correlation summaries with explicit total counts and truncation status (`--max-frames N`)
- optional bounded CPU timeline for one metadata frame (`--frame N`, `--timeline-limit N`) and queue-local GPU timeline (`--gpu-frame N`, `--gpu-timeline-limit N`)
- GPU queue/work/frame/breadcrumb summaries with field names, typed metadata samples, rendered breadcrumb names, representative metadata strings, and metadata byte accounting
- GPU submission-latency samples from `EventBeginWork` (GPU-start vs CPU-submit; not clock calibration)
- serial-ordered normal-event dispatch summary with wrap-aware gap accounting
  (`genuine` after three syncs, otherwise `provisional`)
- counter spec/value summaries with bounded sample points when samples are present
- stat spec/group summaries with explicit sample-event counts
- CSV profiler category/stat summaries with explicit sample-event counts
- load-time class catalog plus package/request/async-loading summaries when present
- IoStore backend/request lifecycle summaries when present
- PlatformFile open/read/write/close summaries with bounded activity samples when present
- memory scope tag counts
- bounded raw callstack catalog with hex program-counter frames, module-mapped
  `mapped_frames`, optional PDB enrichment via `--symbol-path`, and id joins on
  allocation/bookmark samples
- module catalog (`Diagnostics.Module*`) with GUID+age identity when present
- metadata stack clear/save/restore counts
- Slate widget add counts
- trace channel summaries
- bookmark and region annotation summaries with rendered FormatArgs samples
- log category/message-spec catalog with verbosity breakdown and message counts
- rendered FormatArgs log message samples (typed stream + `%s` fallback)
- session identity from `Diagnostics.Session2`
- raw/unmodeled declared event families with counts and representative decoded samples

## Current inventory subset

`utrace inventory` is parser-oriented and currently emits:

- declared event types from the event registry
- observed count per declared event type
- observed count per well-known protocol event
- decode status per declared event type (`decoded`, `partial`, `raw`)
- one generic payload sample per observed event type
- decoded fixed scalar fields (`uint*`, `int*`, `float*`)
- decoded ANSI and wide aux strings
- raw byte summaries for arrays and unsupported payloads

The current fixture no longer has raw declared event families. CPU metadata,
GPU breadcrumb metadata, generic `Cpu.*` named events, CSV profiler catalogs, trace channels, thread
groups, `LoadTime.ClassInfo`, and the simple declared-only families
(`ThreadTiming`, `EndThread`, `MemoryScope`, `MetadataStack.*`,
`SlateTrace.AddWidget`) are decoded or counted (partial). Within the
already-partial families, coroutine restoration, metadata stack restoration,
and full unbounded timelines remain the biggest semantic gaps. Serial-ordered
dispatch, GPU submission-latency samples, bounded CPU/GPU frame timelines, and
bounded Memory allocation summaries are now emitted. Provider-specific real captures remain separate from the CPU-frame
fixture. Run the ignored `targeted_utrace_fixtures_exercise_provider_lifecycles`
test with `UTRACE_TARGETED_FIXTURE` or `UTRACE_TARGETED_FIXTURE_DIR`; its
combined corpus contract requires LoadTime requests, counter values, memory
scopes, and metadata-stack restoration. Memory allocation aggregation is checked
separately with `UTRACE_MEMORY_FIXTURE`; IoStore request lifecycles require a
cooked capture and are checked separately with `UTRACE_IOSTORE_FIXTURE`.
Provider modules also carry always-on synthetic wire and resource-boundary tests.
CI environments with access to the studio fixture corpus must additionally run:

```text
cargo test --all-features --test utrace_fixture -- --ignored
```

with the applicable `UTRACE_*_FIXTURE` variables set. Fixture absence must remain
an explicit ignored contract; it must not turn into a silently passing test.

Do not add the full coverage matrix back into the CLI JSON unless there is a
runtime consumer that needs it. Prefer keeping parser capability notes here and
keeping `utrace dashboard` focused on decoded performance data.

## Practical gaps before this can replace Unreal Insights

The parser is already useful for automated summaries and first-pass hitch
triage. For day-to-day performance investigation, these are the main remaining
gaps, in priority order:

1. **Readable callstack depth** — module+offset and optional PDB names land on
   retained catalog stacks; still no capture-wide symbol index, DWARF backends,
   or Insights-parity UI for every dropped stack beyond the retain caps.
2. **Capture-wide timeline queries** — **CPU first slice delivered**:
   `utrace timeline index` writes a capped sidecar, and `utrace timeline query`
   supports arbitrary inclusive cycle ranges, thread filtering, and
   case-insensitive scope search without reparsing the trace. Correlated frame
   rows now carry CPU bounds for direct indexed navigation, and the web
   workbench caches the sidecar per uploaded capture. GPU remains aggregate-only
   here because its timestamps are a separate clock domain; arbitrary GPU-range
   queries and linked CPU↔GPU zoom remain open.
3. **Task and wait causality** — TaskTrace waits and lifecycle counts decode;
   still missing subsequent-edge graphs and WaitForTasks CPU-scope overlap
   attribution on a live TaskTrace fixture.
4. **Unified performance curves** — Counters/Stats/CSV samples decode when
   present; still need frame-aligned time series with units and a studio
   capture that actually emits Stats.EventBatch2 / CSV sample events.
5. **Asset-loading critical paths** — join LoadTime, IoStore/PlatformFile activity,
   CPU work, and package dependencies so a loading hitch can be attributed to
   the responsible asset and stage. PlatformFile lifecycle summaries now decode;
   cross-provider joins remain open.
6. **Memory attribution** — join allocations to active tags and resolved
   callstacks, add frame-aligned allocation/live-memory curves, and clearly
   surface when bounded outstanding tracking makes totals incomplete.
7. **Regression comparison** — compare baseline and candidate traces using
   frame percentiles, scope/counter/memory/loading deltas, bookmark alignment,
   and machine-readable CI thresholds.

The frontend is secondary to these parser and query-model gaps. Once the data
exists, the useful interaction is a linked path from frame to scope to task,
allocation, file activity, or asset—not a collection of disconnected summary
panels.
