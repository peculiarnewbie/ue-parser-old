---
name: utrace-parser-roadmap
description: Plan for adding a UTrace (.utrace) inspector to the ue-parser CLI
metadata:
  type: plan
---

> Historical design note. The implementation has since moved into this
> UTrace-focused repository. The CLI target retains a legacy compatibility name,
> but asset parsing is not part of the current product scope.
>
> The feature snippets and pre-split file paths below describe the original
> implementation plan; current `Cargo.toml`, `src/utrace.rs`, and the active
> plans are authoritative.

Goal: provide UTrace (`.utrace`) parsing so tools can quickly extract
dashboard-grade performance facts from Unreal Insights traces, while preserving
enough identity to deep-dive in the original trace file.
Format reference: [[utrace-format-deepdive]].

The first product milestone is **not** a full Unreal Insights replacement. It is
a fast extraction layer:

- `utrace inspect`: parser/debug surface for headers, packets, event registry,
  decoded core events, and raw fallbacks.
- `utrace dashboard --format json`: stable, curated dashboard contract for
  frame timings, CPU scope summaries, thread names, regions/bookmarks, and trace
  provenance.

Both commands follow the repo's existing integration rules: schema-versioned
JSON on stdout, diagnostics on stderr, shared exit-code ladder. Dashboard rows
must carry enough provenance to reopen/correlate with the source trace: trace
path/id where available, thread id/name, frame index, scope/event ids, cycles,
seconds, and source file/line when `CpuProfiler.EventSpec` provides it.

## Decision: one binary, feature-gated LZ4

Do **not** ship a second binary. Do **not** shell out to an external LZ4 tool.

- Rationale: UTrace packets use LZ4 *block* decompression with the decoded
  size carried in the packet header (`FTidPacketEncoded.DecodedSize`); the
  standard `lz4` CLI tool only handles the LZ4 *frame* format, so it is
  useless here. Per-packet process spawning is catastrophic on Windows
  (~10-50ms/create × thousands of packets). A pure-Rust block crate has no
  system dependency and no transitive deps, so "avoid pulling deps" is not
  a real constraint here.
- Mechanism: Cargo feature gate. `uasset`-only builds compile zero LZ4 code
  and pull no LZ4 crate. `utrace` builds opt into the crate.

```toml
[features]
default = ["uasset"]
uasset  = []
utrace  = ["dep:lz4_flex"]
```

The `uasset` default keeps current behavior bit-for-bit. Adding `--no-default-features
--features utrace` (or `--all-features`) enables the trace side. One
entrypoint, one exit-code ladder, one schema-versioned JSON contract; shared
`src/archive.rs`, `src/codec.rs`, error types, and CLI plumbing.

LZ4 crate pick: `lz4_flex` (pure Rust, no C, no unsafe by default, block API
matches `LZ4_decompress_safe(input, input_len, out, out_cap)` exactly).
Reject `lz4` (wraps the C lib) to keep the build portable.

## Phasing (mirrors how `uasset` rolled out)

Each phase ends with a green `cargo test --all-targets` and
`cargo clippy --all-targets -- -D warnings` and a fixture test where a
`.utrace` file exists. Phases are ordered so each one is independently
verifiable against the local UE 5.7 tree.

### Phase 1 — Skeleton + header parse
- `src/utrace/mod.rs` (or `src/utrace.rs`) gated by `#[cfg(feature = "utrace")]`.
- `src/bin/main.rs` (or existing entrypoint) dispatches on the first arg:
  `uasset` (default) vs `utrace`.
- Bounded LE reader reused from `src/archive.rs`.
- Parse raw 4-byte magic → `2CRT` (modern TRC2; skip `u16`-prefixed metadata)
  / `ECRT` (TRCE, no metadata) / `0x00000001` (legacy, protocol 0/transport 1)
  / reject swapped `TRC2`/`TRCE` (big-endian). Read `{u8 transport, u8 protocol}`; reject
  `transport != 4` and `protocol > 7` with exit code 3 (unsupported format).
- CLI output: a `header` summary (magic variant, transport, protocol,
  metadata blob length if present). Schema-versioned JSON.

### Phase 2 — Packet walk + LZ4 inflate
- Add `lz4_flex` under `[dependencies]` gated by the `utrace` feature.
- Walk the packet stream: `FTidPacketBase { u16 packet_size, u16 thread_id }`.
- Implement marker-bit decode: bit 15 → LZ4 block with trailing `u16
  decoded_size` then `packet_size - 6` compressed bytes; bit 14 → optional
  `u64` verification serial (guarded by a build-time flag we can leave off);
  bits 0..13 → thread id. Demultiplex by tid: 0=Events, 1=internal, 2..0x3ffe
  =user threads, 0x3fff=sync (bump counter, no payload).
- CLI output: per-thread byte counts, sync count, compression ratio. No event
  decode yet.

### Phase 3 — Type registry (`NewEvent` decode)
- Parse `FNewEventEvent` per Protocol4 (`Engine.cpp:1254`) and Protocol6
  (`Engine.cpp:1312`). Discriminate on `protocol >= 6` for the
  `EFieldFamily`-aware layout.
- Build `TypeRegistry: HashMap<u16, TypeInfo>` keyed by `EventUid`, with
  per-field `{offset, size, type_info, name}` decoded via the Protocol0
  `TypeInfo` byte masks (size, signed, float, string, array).
- CLI output: `events` subcommand listing every declared event with logger
  name, event name, and decoded field types.

### Phase 4 — Important events + prologue
- Decode the Events stream (tid 0) using `FImportantEventHeader { u16 uid,
  u16 size }` framing (`Engine.cpp:3760`). Recognize the `$Trace.NewTrace`
  prologue (`Writer.cpp:77`/`952`): `StartCycle`, `CycleFrequency`,
  `Endian=0x524d`, `PointerSize`, `StartDateTime`. Surface these in
  `inspect` output.
- Decode `$Trace.ThreadInfo` / `$Trace.ThreadGroupBegin/End` as the registry
  matures. These are important events and provide the thread id/name/system id
  data needed for dashboard grouping.

### Phase 5 — Normal events + serial-ordered dispatch
- Per-thread event framing per protocol (P5/6/7 share): 1-2 byte uid
  (`Flag_TwoByteUid` in bit 0, real uid shifted by 1), 24-bit serial after
  the uid for sync'd events, fixed size for well-known uids, registry size
  otherwise.
- Implement the min-heap serial dispatcher (`OnDataNormal`, `Engine.cpp:3854`)
  and sync-point gap detection (`DetectSerialGaps`, `Engine.cpp:4689`). Need
  ≥3 syncs to settle genuine gaps from temporary ones.
- Handle Protocol4 `EnterScope_T`/`LeaveScope_T` (7-byte relative timestamp)
  and Protocol7 `EnterScope_TA/TB`/`LeaveScope_TA/TB` known events
  (`Engine.cpp:5045`/`5062`).

### Phase 6a — Dashboard MVP (core subset + raw fallback)
- Per-field value decode matching `IAnalyzer::FEventData::GetValue/GetString/GetArray`
  (`Engine.cpp:862`/`879`/`844`). Integer/float/pointer from `SizeAndType`
  sign and width; ansi/wide strings; arrays from aux blobs keyed by
  `FieldIndex`; Protocol6 reference fields resolved to their target event uid.
- **Decode only the dashboard-critical subset first; report everything else as
  raw `{uid, logger, event, size, hex}`.** This bounds Phase 6 to a known
  surface and gets useful performance data before broad event-family coverage.
- Implement the CPU timing path end-to-end:
  - `$Trace.NewTrace` for cycle-to-seconds conversion and base date.
  - `$Trace.ThreadInfo` / thread groups for thread labels.
  - `Misc.BeginFrame/EndFrame` for frame intervals.
  - `Misc.RegionBegin[WithId]` / `RegionEnd[WithId]` and
    `Misc.BookmarkSpec` / `Bookmark` for annotations.
  - `CpuProfiler.EventSpec` for `SpecId -> {name, file, line}`.
  - `CpuProfiler.EventBatchV3` for per-thread scope begin/end intervals.
- Add `utrace dashboard --format json` with schema version 1. This is the
  stable dashboard contract and should emit aggregated frame/thread/scope facts
  plus provenance. Keep `utrace inspect --format json` as the broader parser
  inspection/debug contract.
- Fixture test against a `.utrace` captured from the local UE 5.7 tree.
  Resolution order mirrors `uasset`: `UTRACE_FIXTURE_DIR` env var, then a
  studio default; skip when no fixture exists so portable builds stay green.
  `UTRACE_REQUIRE_FIXTURE=1` for fixture-backed CI.

### Phase 6b — Deferred event families (opt-in)
Each family is a self-contained module that can be added independently
behind its own feature flag or `--event-family=<name>` CLI opt-in. No
family in 6b blocks 6a from shipping.

Candidates after the dashboard MVP: `Counters`, `GpuProfiler`, `Memory`/`LLM`,
`LoadTime`/`IoDispatcher`, `PlatformFile`, `Logging`, `CsvProfiler`, `Tasks`,
and `NetTrace`.

## Event family discovery — where the schemas live

The UE source has a clean two-sided structure that makes event families
trivially discoverable:

**Writer side** (event declarations with field types):
`UE_TRACE_EVENT_BEGIN(LoggerName, EventName, Flags)` +
`UE_TRACE_EVENT_FIELD(CppType, FieldName)` +
`UE_TRACE_EVENT_END()`. These compile into the `FNewEventEvent` type
declarations that the parser's `TypeRegistry` (Phase 3) decodes at
runtime — the field names and types are **in the trace file itself**, not
hardcoded in the parser. So the parser doesn't need to know about
specific events to decode their fields generically; it only needs
family-specific logic when the event carries an **encoded blob** (like
`CpuProfiler.EventBatchV3`) or when we want to present a curated JSON
shape instead of a raw field dump.

**Reader side** (which events each subsystem cares about):
`Developer/TraceServices/Private/Analyzers/*.cpp` — each analyzer file
calls `Builder.RouteEvent(routeId, "LoggerName", "EventName")` in
`OnAnalysisBegin`, then reads fields by name in `OnEvent`. The analyzer
file list is the authoritative catalog of event families.

### Family catalog (from Analyzers/ + writer declarations)

| Logger | Events | Writer declaration | Analyzer | Priority |
|--------|--------|--------------------|----------|----------|
| `$Trace` | `NewTrace`, `TraceStall`, `ThreadInfo`, `ThreadGroupBegin/End`, `ChannelAnnounce/Toggle` | `TraceLog/Private/Trace/Trace.cpp` + `Writer.cpp` (minimal API) | `MiscTraceAnalysis.cpp` | **6a — core** |
| `Misc` | `BeginFrame/EndFrame` + `BeginGameFrame/EndGameFrame` + `BeginRenderFrame/EndRenderFrame` | `Core/Private/ProfilingDebugging/MiscTrace.cpp` (minimal API, auto-registered) | `MiscTraceAnalysis.cpp` | **6a — core** (frame boundaries) |
| `Misc` | `RegionBegin[WithId]` / `RegionEnd[WithId]` | `MiscTrace.cpp` | `MiscTraceAnalysis.cpp` | **6a — core** (simple, useful) |
| `Misc` | `BookmarkSpec` / `Bookmark` | `MiscTrace.cpp` | `BookmarksTraceAnalysis.cpp` | 6a (cheap) |
| `CpuProfiler` | `EventSpec` (timer name declarations, Important) | `Core/Private/ProfilingDebugging/CpuProfilerTrace.cpp` | `CpuProfilerTraceAnalysis.cpp` | **6a — core** (needed for timing) |
| `CpuProfiler` | `EventBatchV3` (encoded scope enter/leave blob) | `CpuProfilerTrace.cpp` | `CpuProfilerTraceAnalysis.cpp` | **6a — core** (the bulk of timing data; see note below) |
| `CpuProfiler` | `MetadataSpec` / `Metadata` | `CpuProfilerTrace.cpp` | `CpuProfilerTraceAnalysis.cpp` | 6a |
| `Logging` | `LogCategory` / `LogMessageSpec` / `LogMessage` | `Core/Private/Logging/LogTrace.cpp` | `LogTraceAnalysis.cpp` | 6b |
| `Memory` | `Init` / `Alloc` / `Free` / `Realloc*` / `HeapSpec` / `TagSpec` / etc. | `Core/Private/ProfilingDebugging/MemoryTrace.cpp` | `AllocationsAnalysis.cpp` | 6b |
| `LLM` | `TagsSpec` / `TrackerSpec` / `TagSetSpec` / `TagValue` | `Core/Private/ProfilingDebugging/MemoryTrace.cpp` | `MemoryAnalysis.cpp` | 6b |
| `GpuProfiler` | `Init` / `QueueSpec` / `EventBeginWork` / `EventEndWork` / `EventWait` / etc. | `RHI/Private/GpuProfilerTrace.cpp` | `GpuProfilerTraceAnalysis.cpp` | 6b |
| `Counters` | `Spec` / `SetValueInt` / `SetValueFloat` | `Core/Private/ProfilingDebugging/CountersTrace.cpp` | `CountersTraceAnalysis.cpp` | 6b |
| `CsvProfiler` | `RegisterCategory` / `Define*Stat` / `BeginStat` / `EndStat` / etc. | `Core/Private/ProfilingDebugging/CsvProfilerTrace.cpp` | `CsvProfilerTraceAnalysis.cpp` | 6b |
| `LoadTime` / `IoDispatcher` | `StartAsyncLoading` / `PackageSummary` / `BeginCreateExport` / etc. | `Core/Private/ProfilingDebugging/IoStoreTrace.cpp` + engine | `LoadTimeTraceAnalysis.cpp` | 6b |
| `PlatformFile` | `BeginOpen` / `EndOpen` / `BeginRead` / etc. | `Core/Private/ProfilingDebugging/PlatformFileTrace.cpp` | `PlatformFileTraceAnalysis.cpp` | 6b |
| `PlatformEvent` | `Settings` / `ContextSwitch` / `StackSample` / `ThreadName` | `Core/Private/ProfilingDebugging/PlatformEvents.cpp` | `PlatformEventTraceAnalysis.cpp` | 6b |
| `CookTrace` | `Package` / `PackageStat` / etc. | `Core/Private/ProfilingDebugging/CookStats.cpp` | `CookAnalysis.cpp` | 6b |
| `NetTrace` | `InitEvent` / `NameEvent` / `PacketEvent` / etc. | (networking module) | `NetTraceAnalyzer.cpp` | 6b |
| `MetadataStack` | `ClearScope` / `SaveStack` / `RestoreStack` | `Core/Private/ProfilingDebugging/TagTrace.cpp`? | `MetadataAnalysis.cpp` | 6b |
| `Diagnostics` | `Session[2]` / `ModuleInit` / `ModuleLoad` / `ModuleUnload` | `Core/Private/Modules/ModuleManager.cpp` + `ModuleDiagnostics.h` | `DiagnosticsAnalysis.cpp` / `ModuleAnalysis.cpp` | 6b |
| `Tasks` | task graph events | `Core/Private/Async/TaskTrace.cpp` | `TasksAnalysis.cpp` | 6b |

### `CpuProfiler.EventBatchV3` — the encoded blob

The one event in the 6a subset that needs family-specific decode logic.
`EventBatchV3` carries a single `uint8[] Data` field (an aux blob) containing
a variable-length-encoded stream of timer scope enter/leave events. The
encoding (from `CpuProfilerTrace.cpp` and `CpuProfilerTraceAnalysis.cpp`):
- Each entry starts with an unsigned 7-bit varint. The upper bits are a cycle
  delta (`decoded >> 2`); the low two bits are an opcode.
- Opcode `01`: begin scope. The next unsigned 7-bit varint is the id payload.
  In V3, payload bit 0 selects metadata id vs plain spec id; plain spec id is
  `payload >> 1` and maps through `CpuProfiler.EventSpec`.
- Opcode `00`: end scope. No following id payload.
- Opcode `11`: coroutine resume. Followed by coroutine id and timer-scope
  depth varints.
- Opcode `10`: coroutine suspend. Followed by timer-scope depth varint.
- `ActualCycle` is reconstructed like UE's analyzer: start with the current
  thread `LastCycle`; add the decoded delta when needed, and use the event
  context's base cycle to recover late-connect streams.
- The analyzer (`CpuProfilerTraceAnalysis.cpp`, ~40KB) decodes this into
  scope enter/leave pairs with timestamps. This is the most complex single
  decoder in the 6a subset but it's self-contained — no dependency on other
  families beyond `EventSpec` (which provides the timer name → id mapping).

7-bit varint and zigzag helpers both exist in `MiscTrace.h:37`, but normal
`EventBatchV3` CPU scope timing uses unsigned 7-bit varints, not zigzag.
Zigzag remains relevant to other encoded payloads only if a later family uses
it. The Rust 7-bit varint decoder should be hand-rolled with bounds checks
rather than pulling a crate for this small format.

### Retired events (still routed for old files)

`MiscTraceAnalysis.cpp:216` labels `RegisterGameThread`, `CreateThread`,
`SetThreadGroup`, `BeginThreadGroupScope`, `EndThreadGroupScope` as
"retired events." The writer-side declarations have been removed; these
events only appear in pre-5.x trace files. Modern files use
`$Trace.ThreadInfo` + `$Trace.ThreadGroupBegin/End` instead. The parser
should handle both if encountered, but doesn't need to prioritize the
retired forms.

## Open questions (defer until Phase 1 starts)

- Whether `utrace` should be its own `src/bin/utrace.rs` (still the same
  Cargo package, same crate, just a separate `[[bin]]` target) or a subcommand
  of the existing `uasset` binary. Lean: subcommand of one binary, per the
  "one entrypoint" decision above. Reconsider only if the dispatch logic in
  `main.rs` grows past trivial.
- Whether to expose the legacy `.ue4stats` reader at all. Verdict from the
  deepdive: 5.7 can still *write* them but ships no consumer; a reader is
  feasible (format fully documented in `StatsFile.h`) but only matters for
  legacy archives. Defer until someone actually has a `.ue4stats` file in
  hand. If ever added, gate it behind its own `ue4stats` feature, since it
  pulls in nothing heavy but adds surface area.
- LZ4 verification serial handling: leave `UE_TRACE_PACKET_VERIFICATION` off
  initially (matches the default write path). Add a `--strict` flag later if
  we ever want to validate packet serials.
- Dashboard aggregation shape: decide during Phase 6a whether schema v1 emits
  raw scope intervals only, pre-aggregated per-frame/per-thread summaries, or
  both. Lean: both, with intervals behind an explicit flag or bounded top-N
  list so normal dashboard ingestion stays small.
