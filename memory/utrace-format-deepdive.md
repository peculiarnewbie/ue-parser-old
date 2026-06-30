---
name: utrace-format-deepdive
description: UE 5.7 UTrace file wire format and analysis pipeline, plus legacy .ue4stats status
metadata:
  type: reference
---

Target engine: UE 5.7.2 at `C:\Users\Ryzen\Perforce\Arif_UE-ManaBreak`. This note
records the on-disk wire format of `.utrace` files produced by the `TraceLog`
runtime and consumed by the `TraceAnalysis` engine (Unreal Insights), and the
status of the pre-5.0 `.ue4stats` files. It is the trace-side companion to
[[ue-source-reference]].

## Module layout

| Role | Module | Key files |
|------|--------|-----------|
| Runtime writer (in-process) | `Runtime/TraceLog` | `Private/Trace/Writer.cpp`, `Private/Trace/Codec.cpp`, `Public/Trace/Detail/Protocols/Protocol0..7.h`, `Public/Trace/Detail/Transport.h` |
| LZ4 codec | `Runtime/TraceLog` | `Private/Trace/LZ4/lz4.c.inl` (standard LZ4 block API; `Epic.patch` only renames `ok`→`length_ok` for MacOS SDK) |
| Analysis reader | `Developer/TraceAnalysis` | `Private/Analysis/Engine.cpp` (5548 lines, the spec), `Private/Analysis/Transport/TidPacketTransport.cpp`, `Private/Analysis/StreamReader.cpp` |
| Analyzer interface | `Developer/TraceAnalysis` | `Public/Trace/Analyzer.h` |
| Insights app | `Developer/TraceInsights`, `Developer/TraceServices`, `Developer/TraceInsightsFrontend` | layered on top of TraceAnalysis |

`TraceLog.Build.cs` is a minimal module; `TraceAnalysis` pulls in `TraceLog`
purely for the `Protocol*` headers and the `Decode` (LZ4 `decompress_safe`)
symbol — it never links the writer.

## .utrace file structure (transport = TidPacketSync, id 4)

The file is a flat byte stream. The analysis engine walks it through a small
state machine (`Engine.cpp: FMagicStage → FMetadataStage? → FEstablishTransportStage
→ FProtocolNStage`). All multi-byte fields are host-endian; the runtime only
writes little-endian platforms in practice.

### 1. Magic (4 bytes) — `FMagicStage` (Engine.cpp:5312)
- Raw bytes `2CRT` — modern TRC2 files: magic then a metadata block. The writer
  stores `FHandshake.Magic = '2' | ('C' << 8) | ('R' << 16) | ('T' << 24)`.
- Raw bytes `ECRT` — older TRCE files: magic, then directly the transport header (no metadata).
- `0x00000001` — pre-magic legacy: bytes are already `{TransportVersion=0, ProtocolVersion=1}` interpreted directly.
- Raw bytes `TRC2` / `TRCE` — swapped-endian variants; analysis rejects them ("Big endian traces are currently not supported").

### 2. Metadata (only after raw bytes `2CRT`) — `FMetadataStage` (Engine.cpp:5255)
A length-prefixed blob:
```
uint16 MetadataSize;
uint8  Metadata[MetadataSize];
```
Contents are a sequence of `(uint8 FieldId, uint8 Size, uint8 Value[Size])`
TLVs whose schema lives in `Writer.cpp` (`FHandshake`, line 769):
- FieldId 0: `uint16 ControlPort`
- FieldId 1: `uint8[16] SessionGuid`
- FieldId 2: `uint8[16] TraceGuid`

The analysis side currently just skips the whole blob (`Reader.Advance(sizeof(uint16) + *MetadataSize)`); the GUIDs/control port are only meaningful for live-tail connections, not file analysis.

### 3. Transport + Protocol header (2 bytes) — `FEstablishTransportStage` (Engine.cpp:5141)
```
uint8 TransportVersion;   // ETransport: 1=Raw, 2=Packet, 3=TidPacket, 4=TidPacketSync
uint8 ProtocolVersion;    // EProtocol: 0..7
```
`ETransport::Active == TidPacketSync (4)` for all current builds
(`Transport.h:18`). The stage instantiates the matching `FTransport`
subclass and queues the matching `FProtocolNStage`.

### 4. Packet stream — `FTidPacketTransport` (TidPacketTransport.cpp)

A stream of variable-length packets. Each packet header is `FTidPacketBase`
(`Transport.h:39`):
```
uint16 PacketSize;     // total packet size including this header
uint16 ThreadId;       // see marker bits below
```
`ThreadId` bit layout:
- bit 15 (`0x8000` `EncodedMarker`): set ⇒ packet payload is LZ4-compressed; the next `uint16 DecodedSize` gives the decompressed length, then `Data[PacketSize-6]` is the LZ4 block. Clear ⇒ `Data[PacketSize-4]` is raw.
- bit 14 (`0x4000`): `Verification` — if `UE_TRACE_PACKET_VERIFICATION` was on at write time, the packet is immediately followed by a `uint64` packet serial for integrity checking.
- bits 0..13 (`ThreadIdMask = 0x3fff`): the logical thread id.

Reserved thread ids (`ETransportTid`, `Transport.h:22`):
- `0` Events — the "important"/new-event channel (carries `NewEvent` declarations and important/cached events).
- `1` Internal / Importants — alias for the important cache stream.
- `2` Bias — first *real* thread id; `[Bias, 0x3ffe]` are user threads.
- `0x3fff` Sync — a sync marker packet; `Update()` returns `NeedMoreData` so consumers see a stable snapshot point. `FTidPacketTransport::Synced` counts these; Protocol5 uses them to detect genuine serial gaps.

On a compressed packet, analysis calls `UE::Trace::Private::Decode(Packet->Data, DataSize, Dest, DecodedSize)` which is `LZ4_decompress_safe`. **Any Rust port must use LZ4 *block* decompress (not LZ4 frame)** — `lz4::block::decompress` with `expected_size = DecodedSize`. Compression at write time is `LZ4_compress_fast(.., acceleration=1)`.

Per-thread decoded bytes are accumulated in a `FStreamBuffer` (`StreamReader.cpp:78`) so events are never fragmented across packet boundaries.

## Event framing — protocols 0..7

The protocol version selects the per-event header layout. Protocol numbers are
monotonic extensions; 5/6/7 share the same outer framing.

| Ver | Header (`FEventHeader`) | Serial | Notes |
|-----|-------------------------|--------|-------|
| 0 | `{uint16 Uid, uint16 Size}` | — | No sync, no aux. `Uid & 0x3fff` (UidMask). `NewEvent` uid=0. |
| 1 | `{uint16 Uid, uint16 Size, uint16 Serial}` | 16-bit inline | `Flag_Important=1<<14` on the Uid word. Aux via `FAuxHeader` after event. |
| 2 | `{uint16 Uid, uint16 Size}` + sync form `{uint16 Uid, uint16 SerialLow, uint8 SerialHigh}` (packed, 7 bytes) | 24-bit | `Serial` only present when the type is *not* `NoSync`. |
| 3 | same as 2 | 24-bit | `NewEvent` is `NoSync`. |
| 4 | `{uint16 Uid}` (no Size for known) | 24-bit | Introduces well-known Uids: `NewEvent=0, EnterScope, EnterScope_T, LeaveScope, LeaveScope_T, User=5`. `Uid` word has `Flag_TwoByteUid=1<<0` in bit 0 and the real uid shifted by `_UidShift=1`. `EnterScope_T`/`LeaveScope_T` carry a 7-byte (56-bit) relative timestamp read as `*(uint64*)(p-1) >> 8`. |
| 5 | `{uint16 Uid}` for normal, `{uint16 Uid, uint16 Size}` for important | 24-bit | New well-knowns: `AuxData, AuxDataTerminal`. Important events go on the Events thread (tid 0) with an explicit Size; normal events live on their own thread stream and their size comes from the type registry. 24-bit serial lives in the 3 bytes *after* the uid for sync'd events. |
| 6 | = Protocol5 | = P5 | Adds `EEventFlags::Definition`, `EFieldFamily {Regular, Reference, DefinitionId}` in `FNewEventEvent`. `FNewEventEvent` field entry grows a `FieldType` discriminator byte. Reference fields point at another event uid; DefinitionId fields are self-referential. |
| 7 | = Protocol5/6 | = P5 | Adds `EnterScope_TA / LeaveScope_TA` (8-byte absolute timestamp) and `EnterScope_TB / LeaveScope_TB` (7-byte base-relative timestamp, read as `*(uint64*)(p-1) >> 8`). |

### `FNewEventEvent` (type declaration) — Protocol4 vs Protocol6

Emitted on the Events thread as an *important* event. Layout (Protocol4, `Protocol0.h:71`/`Protocol4.h:45`):
```
uint16 EventUid;
uint8  FieldCount;
uint8  Flags;          // Protocol4::EEventFlags
uint8  LoggerNameSize;
uint8  EventNameSize;
struct { uint16 Offset; uint16 Size; uint8 TypeInfo; uint8 NameSize; } Fields[FieldCount];
uint8  NameData[LoggerNameSize + EventNameSize + sum(Fields[].NameSize)];
```
Protocol6 (`Protocol6.h:45`) replaces `Fields[]` with a discriminated union keyed on `uint8 FieldType` (`EFieldFamily`): `Regular` (same as P4), `Reference` (`{Offset, RefUid, TypeInfo, NameSize}` — points at a sibling event's instance), `DefinitionId` (`{Offset, Unused1, Unused2, TypeInfo}` — refers to the declaring event itself).

`TypeInfo` (Protocol0) packs the field type in one byte:
- bits 0..1 `Field_Pow2SizeMask`: log2 size (0→1, 1→2, 2→4, 3→8).
- bits 4..6 `Field_SpecialMask`: `Field_Pod`, `Field_String`, `Field_Signed`.
- bits 6..7 `Field_CategoryMask`: `Field_Integer (0x00)`, `Field_Float (0x40)`, `Field_Array (0x80)`.
- `EFieldType` enumerates the concrete combinations (Bool, Int8..Int64, Uint8..Uint64, Pointer, Float32/64, AnsiString, WideString, Array).

The analysis `FTypeRegistry::AddVersion4` / `AddVersion6` (`Engine.cpp:1254` / `1312`) parses this into an `FTypeInfo` keyed by `EventUid`. `SizeAndType` is stored as a signed int8: positive = integer with that byte size, negative = float with `abs(size)`, `Field_String` fields carry element size 1 (ansi) or 2 (wide). This is exactly what `IAnalyzer::FEventFieldInfo::GetType()` (`Engine.cpp:675`) reverses to pick `EType::Integer / Float / AnsiString / WideString / Reference8..64`.

### Aux data

Events flagged `MaybeHasAux` are followed (on the same thread stream) by a sequence of `FAuxHeader` blobs and a terminating zero byte (`AuxDataTerminal`). Protocol1/2 `FAuxHeader` (`Protocol1.h:51`):
```
uint8 FieldIndex_Or_Size;  // MSB (0x80) = "this is aux"; low 7 bits = field index
uint8 Data[...];           // size encoded as (Header.Size >> 8) + sizeof(FAuxHeader)
```
Protocol5+ `FAuxHeader` (`Protocol5.h:84`) is 4 bytes packed: `{uint8 Uid, uint8 FieldIndex_Size, uint16 Size}`, with `Uid = EKnownUids::AuxData` and the payload size at `Pack >> FieldShift (13)`.

Aux payloads are how variable-length data (strings, arrays) attach to otherwise fixed-size events. `FEventData::GetString` / `GetArray` (`Engine.cpp:879` / `844`) look up the aux blob by `FieldIndex` and return a view over `Data`/`DataSize`. Fragmented aux (where a single logical value is split across multiple `FAuxHeader` entries) is reassembled by `FAuxDataCollector::Defragment`.

### Important events and the serial order

"Important" events (typed with `EEventFlags::Important`, e.g. the `$Trace.NewTrace` prologue and the `ThreadSpec.Name` thread-name events) are cached on the runtime and replayed on every connection. They are carried on tid 0 (Events) with an explicit `Size` and *no* serial. Non-important sync'd events carry a 24-bit monotonic serial. The Protocol5 dispatcher (`OnDataNormal`, `Engine.cpp:3854`) builds a min-heap of per-thread `FEventDesc` runs keyed by serial and dispatches them in serial order, using `Sync` packets as commit points to distinguish genuine serial gaps (dropped tail packets) from temporary ones (data not yet arrived). See `DetectSerialGaps` (`Engine.cpp:4689`) — it needs 3 syncs to settle.

## `$Trace.NewTrace` prologue

The very first important event written by `Writer_SessionPrologue` (`Writer.cpp:952`):
```
uint64 StartCycle;
uint64 CycleFrequency;
uint16 Endian;          // written as 0x524d ('RM')
uint8  PointerSize;     // sizeof(void*) of the producing platform
double StartDateTime;   // seconds since epoch as a double
```
This is the source of the cycle→time conversion and the platform pointer width that an analyzer needs to interpret the rest of the stream. There is no separate "trace header" — the prologue is just the first Important `NewEvent`-typed event in the Events stream.

## Dashboard-critical event families

The source-code deep dive confirms a small useful subset for performance
dashboard extraction. This subset is enough to summarize CPU timing by
frame/thread and still preserve a path back to the original trace for deep
dives.

| Data needed | Events | UE source |
|-------------|--------|-----------|
| Trace clock/base date | `$Trace.NewTrace` | `Runtime/TraceLog/Private/Trace/Writer.cpp:77` and `:963` |
| Thread labels | `$Trace.ThreadInfo`, `$Trace.ThreadGroupBegin/End` | `Runtime/TraceLog/Private/Trace/Trace.cpp:261` |
| Frame intervals | `Misc.BeginFrame`, `Misc.EndFrame` | `Runtime/Core/Private/ProfilingDebugging/MiscTrace.cpp:50`; analyzer in `MiscTraceAnalysis.cpp` |
| Regions | `Misc.RegionBegin[WithId]`, `Misc.RegionEnd[WithId]` | `MiscTrace.cpp:28`; analyzer in `MiscTraceAnalysis.cpp` |
| Bookmarks | `Misc.BookmarkSpec`, `Misc.Bookmark` | `MiscTrace.cpp:14`; analyzer in `BookmarksTraceAnalysis.cpp` |
| CPU timer names | `CpuProfiler.EventSpec` | `Runtime/Core/Private/ProfilingDebugging/CpuProfilerTrace.cpp:26`; analyzer `OnEventSpec` |
| CPU scope intervals | `CpuProfiler.EventBatchV3` | `CpuProfilerTrace.cpp:48`/`:154`; analyzer `ProcessBufferV2` |

`EventSpec` carries `Id`, `Name`, and, when enabled, `File` and `Line`. The
dashboard parser should keep those fields as provenance for any scope summary.
`EventBatchV3` uses the thread carrying the event as the timing thread; Unreal's
analyzer reads it via `Context.ThreadInfo.GetId()`.

### `CpuProfiler.EventBatchV3` encoding

`EventBatchV3` has one `uint8[] Data` field. The writer appends compact records
to a small per-thread buffer and flushes it as an event. The normal CPU scope
records are unsigned 7-bit varints, not zigzag values:

```
first = Decode7bit()
cycle_delta = first >> 2
opcode = first & 0b11
```

Opcodes used by UE 5.7:

- `0b01`: begin regular/metadata scope. Followed by another 7-bit varint. In
  V3, payload bit 0 means metadata id; otherwise `payload >> 1` is the
  `EventSpec.Id`.
- `0b00`: end scope. No following id.
- `0b11`: coroutine resume. Followed by coroutine id and timer-scope depth.
- `0b10`: coroutine suspend. Followed by timer-scope depth.

Cycle reconstruction mirrors `FCpuProfilerAnalyzer::ProcessBufferV2`: the
decoded cycle is a delta from the previous event on that thread, with a
late-connect correction against the enclosing event's base cycle. Convert cycles
to seconds using `$Trace.NewTrace.CycleFrequency` and start cycle/base event
time. Metadata records can be recognized and preserved later, but the dashboard
MVP can start with plain scope ids plus a clear "metadata scope unresolved"
fallback.

## Designing the `utrace` parser CLI

The analysis engine is deliberately decoupled from the rest of Unreal:
- `TraceAnalysis.Build.cs` only depends on `TraceLog` (for `Protocol*` headers + `Decode`), `Core`, and `JsonUtilities` (for `EventToCbor.cpp`). The `Decode` function is standalone LZ4.
- `TraceLog` ships `create_standalone.py` which inlines the writer into a single header pair (`standalone_prologue.h.src` / `standalone_epilogue.h.src`) for embedding in non-UE programs — proof that the wire format is self-contained.

A Rust `utrace` parser following this repo's conventions would need:

1. **Header parse** — read 4-byte magic; branch on raw bytes `2CRT`/`ECRT`/`0x00000001`. For `2CRT`, skip the metadata block (`u16 size` + bytes); reject swapped `TRC2`/`TRCE`.
2. **Transport/protocol byte pair** — `u8 transport, u8 protocol`. Reject `transport != 4` (TidPacketSync is the only one written today; Raw/Packet/TidPacket are present for historical reasons) and `protocol > 7`.
3. **Packet loop** — `FTidPacketBase { u16 packet_size, u16 thread_id }`; apply the three marker bits; on `EncodedMarker`, read `u16 decoded_size` and `LZ4_decompress_safe` the next `packet_size - 6` bytes. Demultiplex by `thread_id`: tid 0 → important/new-events stream, tid 1 → internal (alias), tid `0x3fff` → sync marker (bump sync counter, no payload), `2..0x3ffe` → per-thread event streams.
4. **Per-thread event framing** — selected by protocol. For P5/6/7: read uid byte (extend to u16 if `Flag_TwoByteUid`), shift by 1; if uid is a well-known (`EnterScope`, `LeaveScope`, `AuxData`, `AuxDataTerminal`, `EnterScope_TA/TB`, `LeaveScope_TA/TB`) use the fixed size from `SetSizeIfKnownEvent`; otherwise look up `EventUid` in the registry built from `NewEvent` declarations and read `TypeInfo->EventSize` bytes. Sync'd events additionally carry a 24-bit serial in the 3 bytes after the uid.
5. **`NewEvent` decoding** — parse `FNewEventEvent` per protocol 4 or 6; build a `TypeRegistry` (`EventUid → {logger, event, fields[]}`). Field `TypeInfo` byte decodes via the Protocol0 masks. This must complete before any user event with that uid can be interpreted.
6. **Field value decode** — for each field, `EventDataPtr + Field.Offset` gives the raw bytes; sign/float/string/array is decided by `SizeAndType` and `Class` exactly as `IAnalyzer::FEventData::GetValue/GetString/GetArray` does. Strings/arrays pull from the following aux blobs by `FieldIndex`.
7. **Dashboard subset decode** — decode `$Trace.NewTrace`, `$Trace.ThreadInfo`, `Misc` frame/region/bookmark events, `CpuProfiler.EventSpec`, and `CpuProfiler.EventBatchV3`. Emit stable dashboard JSON with frame/thread/scope summaries and provenance, while retaining `inspect` output for parser debugging and raw unknown events.

The repo already has the right primitives: `src/archive.rs` (bounded LE reader, checked cursor), `src/codec.rs` (soft-object-path byte handling — analogous to trace string decode), and a `bin/` layout used by `uasset`. The roadmap currently favors one CLI entrypoint with `utrace inspect` and `utrace dashboard` subcommands, not a separate public binary. The only new dependency required is an LZ4 block crate (e.g. `lz4_flex` or `lz4` with block-mode).

Recommended phasing that mirrors how this repo rolled out `uasset`:
1. Header + magic + transport/protocol + packet walking with raw hex dump per thread (no decode).
2. LZ4 inflation and sync-packet counting; emit packet/thread statistics.
3. `NewEvent` parsing → type registry dump (logger, event, fields with decoded types).
4. Important-event decode (prologue `$Trace.NewTrace`, thread specs).
5. Normal-event decode + serial-ordered dispatch with sync-point gap detection.
6. Dashboard MVP: schema-versioned `utrace dashboard --format json` for CPU timing scopes, thread names, frame markers, regions/bookmarks, and provenance. Keep `utrace inspect` as the broader parser/debug surface.

## Legacy `.ue4stats` files — still present in 5.7

The old Stats system (`stat startfile` / `stat startfileraw`) is **not removed**.

- Code lives in `Runtime/Core/{Public,Private}/Stats/StatsFile.{h,cpp}` and `StatsCommand.cpp`. It is compiled when `STATS` is defined.
- `STATS` is on by default in Debug and Development (`Misc/Build.h:258`/`286`): `STATS = (WITH_UNREAL_DEVELOPER_TOOLS || !WITH_EDITORONLY_DATA || USE_STATS_WITHOUT_ENGINE || FORCE_USE_STATS) && !ENABLE_STATNAMEDEVENTS`. In Test/Shipping it's off unless `FORCE_USE_STATS=1`.
- `FCommandStatsFile::Start` / `StartRaw` (`StatsFile.cpp:1073`/`1082`) still allocate `FStatsWriteFile` / `FRawStatsWriteFile` and write the file. The console commands `stat startfile` / `stat startfileraw` / `stat stopfile` are still wired in `StatsCommand.cpp:2052`/`2058`/`2065`.
- File magic (`StatsFile.h:43`/`54`):
  - `MAGIC_NO_HEADER = 0x7E1B83C1` (v1, no header) and swapped `0xC1831B7E`.
  - `MAGIC = 0x10293847` (v2+, with `FStatsStreamHeader`) and swapped `0x47382910`. Latest is `VERSION_6 = 6` (added realloc messages, not backward compatible with v5).
- The reader code (`FStatsReadFile`, `ReadHeader`, `ReadStatPacket`) compiles and is self-consistent.

**But the reader is orphaned.** `FStatsReadFile` / `FStatsWriteFile` / `FRawStatsWriteFile` are referenced *only* inside the Stats module itself (`StatsFile.{h,cpp}`, `StatsData.h`). No consumer in `Developer/` (TraceInsights, TraceServices) or `Editor/` loads `.ue4stats` files anymore. The classic Profiler UI that consumed them is gone; Unreal Insights (`TraceInsights`/`TraceAnalysis`) only reads UTrace streams.

Net: UE 5.7 can still *write* `.ue4stats` files for backward compatibility with external/legacy tooling, and the reader code is still there and would compile into a standalone parser, but the engine itself no longer ships a UI that consumes them. A Rust parser targeting 5.7 traces should prioritize `.utrace`; a `.ue4stats` reader is feasible (the format is fully documented in `StatsFile.h`'s `FStatsStreamHeader` / `FStatPacket` / `FStatMessageArray` structs and `ReadHeader` at line 630) but would only matter for legacy archives.

## File-to-module map (trace side)

| Parser area (proposed) | UE source |
|------------------------|-----------|
| magic / metadata / transport handshake | `Developer/TraceAnalysis/Private/Analysis/Engine.cpp` (`FMagicStage`, `FMetadataStage`, `FEstablishTransportStage`); `Runtime/TraceLog/Private/Trace/Writer.cpp` (`FHandshake`, line 769) |
| packet framing, LZ4 decode | `Runtime/TraceLog/Public/Trace/Detail/Transport.h`; `Developer/TraceAnalysis/Private/Analysis/Transport/TidPacketTransport.cpp`; `Runtime/TraceLog/Private/Trace/Codec.cpp` |
| per-protocol event header layouts | `Runtime/TraceLog/Public/Trace/Detail/Protocols/Protocol0..7.h` |
| `NewEvent` type declarations | `Engine.cpp` `FTypeRegistry::AddVersion4` (1254) / `AddVersion6` (1312) |
| well-known scope/timestamp events | `Engine.cpp` `FProtocol4Stage::OnDataKnown` (3393), `FProtocol7Stage::SetSizeIfKnownEvent` (5045) / `DispatchKnownEvent` (5062) |
| field value decode (int/float/string/array/reference) | `Engine.cpp` `IAnalyzer::FEventFieldInfo::GetType` (675), `FEventData::GetValueImpl` (862), `GetString` (879/907), `GetArrayImpl` (844) |
| serial ordering + sync gaps | `Engine.cpp` `FProtocol5Stage::OnDataNormal` (3854), `DispatchNormalEvents` (4092), `DetectSerialGaps` (4689) |
| `$Trace.NewTrace` prologue | `Writer.cpp:77` (event decl), `Writer.cpp:952` (prologue emit) |
| `$Trace.ThreadInfo` and thread groups | `Trace.cpp:261` (`ThreadInfo` declaration), `Trace.cpp:283` (emit) |
| frame, region, bookmark events | `MiscTrace.cpp:14` (`BookmarkSpec`), `:28` (`RegionBegin`), `:50` (`BeginFrame`); `MiscTraceAnalysis.cpp`; `BookmarksTraceAnalysis.cpp` |
| CPU profiler event specs and batches | `CpuProfilerTrace.cpp:26` (`EventSpec`), `:48` (`EventBatchV3`), `:154` (batch emit); `CpuProfilerTraceAnalysis.cpp:391` (`ProcessBufferV2`) |
| legacy `.ue4stats` | `Runtime/Core/Public/Stats/StatsFile.h`, `Runtime/Core/Private/Stats/StatsFile.cpp`, `StatsCommand.cpp:2052` |
