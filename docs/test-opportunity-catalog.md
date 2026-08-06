# Historical test opportunity catalog

> Historical mixed-scope catalog. Asset-parser entries are retained as
> provenance from before this repository narrowed to UTrace. For current work,
> use the UTrace-focused tests and coverage notes instead.

Evidence-based index of where more tests would help. Companion policy:
[`tdd-prioritization.md`](tdd-prioritization.md). Class/event scoreboards stay in
[`asset-coverage.md`](asset-coverage.md) and `memory/utrace-coverage-matrix.md`.

**Audit basis:** module-by-module comparison of implementation vs every in-module
`#[test]` (plus `tests/*` and fuzz). Not a vibe pass — gaps cite code. Re-audit
when large surfaces land (new asset class, GpuProfiler unit suite, etc.).

**Baseline:** ~18.5k prod / ~9.2k test LOC; ~162 `src/` unit tests; ~38
integration tests (many `#[ignore]`); 3 panic-only fuzz targets; no property-test
crate yet.

**Oracle map:** wire format → UE tree (`memory/ue-source-reference.md`); domain
values → `tests/fixtures/electroswag-v15.json`.

**Correctness marker:** **BUG** means comparison with the UE 5.7.2 source found
an implementation mismatch, not merely missing regression coverage. The test
slice for that entry must land with the fix and should encode both sides of any
version boundary.

| Pri | Meaning |
|-----|---------|
| **P0** | Wrong here corrupts many assets/events, or table/docs claim more than tests prove |
| **P1** | Shared translation surface; cheap units unlock safe feature work |
| **P2** | Narrower; grow when that decoder/provider is touched |
| **P3** | Depth / generative / fuzz enrichment |

---

## Attention map (evidence summary)

```text
Over-claimed vs tests
  GpuProfiler: 12 Partial events, heavy impl, zero crafted-packet decode tests
  CLI asset JSON / text render: zero unit tests on asset_output_from_decoded

Thin relative to impl
  property.rs: optional tag fields/root extensions covered; deeper combinations remain
  codec.rs: scalar matrix covered; container shape/error combinations remain
  package maps/paths/version gates: summary happy-path only in default CI
  Memory provider: root Alloc/Free only; Init/TagSpec/system/video/realloc untested
  CurveTable Simple/Empty modes: no unit tests

UE-source mismatches found during independent audit
  BUG: modern inline FSoftObjectPath is decoded as obsolete FString + FString
  BUG: legacy CurveTables omit the mode byte, but the parser always consumes it
  Package saved-hash and import/export layout gates lack boundary matrices
  Current UE 5.7 Memory events are missing while fictional video-realloc names
    were previously called out

Already relatively strong (default CI)
  archive Reader core paths
  codec SoftObject happy paths + ByteProperty enum/plain + Map INDEX_NONE
  asset DataTable/Enum/Struct/UObject/Skeleton synthetic decodes
  utrace CPU EventBatchV3 stack/coroutine/rebase cluster
  utrace_dispatch serial merge/wrap/gap
  counters / CSV catalog / LoadTime crafted packets / logging / Session2
```

---

## 1. `src/archive.rs` — 18 unit tests

**Strong today:** LE primitive matrix, bounded/take_bounded and construction failures,
seek/skip failure cursor preservation, FString empty/ANSI/wide
happy + missing terminator + bad UTF-16, soft subpath null-trim, TArray count/alloc
limits, Guid/IoHash/NameRef happy, negative NameIndex.

| Gap | Evidence | Pri |
|-----|----------|:---:|
| `max_string_code_units` / `validate_string_length` | Impl `815–824` | P1 |
| `validate_remaining` | Impl `687–697` | P2 |
| Retire or version the obsolete inline `read_soft_object_path` helper | Impl `473–484`; its early `remaining()==0` branch is secondary to the modern-layout BUG in codec section | P1 |

**Property-test candidates:** count/alloc/string-length limit matrix; seek/skip
failure leaves `tell()` unchanged.

---

## 2. `src/version.rs` — 2 unit tests

**Strong today:** package flag bits/contains including `FILTER_EDITOR_ONLY`; UE4/UE5
inclusive predicate boundaries; unversioned licensee behavior.

| Gap | Evidence | Pri |
|-----|----------|:---:|
| Bound constants (too-old UE4, too-high UE5) | `38–42`; package has future-legacy `-10` only (`1962–1971`); prove through layout/version rejection tests rather than isolated constant asserts | P1 |

---

## 3. `src/package.rs` — 14 active + 2 ignored unit tests

**Strong today:** both sides of `PACKAGE_SAVED_HASH`, current UE5 summary contract,
cooked/unversioned flags, swapped/zero tag, future legacy, truncated field path,
absurd name count, table-at-EOF and export-span overflow/past-EOF rejection,
soft-path sanitize/plausible helpers, import outer **cycle**, archive→package limit mapping.

**Ignored only:** StarterContent summary (`1863`); SoftObjectPaths from
`DT_AssetRefs` (`1929`).

| Gap | Evidence | Pri |
|-----|----------|:---:|
| **BUG:** version-aware `read_soft_object_path_list` subpaths (wide/workaround vs UTF-8; trailing-NUL boundary) | UE `SoftObjectPath.cpp:569–599`; parser `archive.rs:512+`, `package.rs:886–924` ignores retained Fortnite custom versions | P0 |
| Synthetic path-list values (None / Package.Asset / :SubPath) | Impl `886–924`, `931–956`; only ignored fixture; include both custom-version layouts | P0 |
| `validate_package_versions` unversioned / UE4 too old / too high | Impl `766–797`; cover around the structural summary matrices | P1 |
| `validate_legacy_version` UE3 / too-old legacy | Impl `738–761` | P1 |
| `resolve_name` with `number != 0` | Impl `377–379`, `1256–1267` | P1 |
| Import/export path resolution depth limit (non-cycle) | Impl `1320–1326` | P0 |
| Import/export record layout matrix across implemented gates | UE `ObjectResource.cpp:149–209`, `353–371`; parser `1044–1173`; one wrong field gate shifts the rest of the table | P0 |
| `Package::parse` with non-empty synthetic names/imports/exports | `tiny_corpus` is empty tables only; use it to integrate the fixed-size layout matrices | P1 |
| `FILTER_EDITOR_ONLY` summary/import branches | Impl `492+`, `1053+`; include in import/export layout matrix | P0 |
| Custom version layouts / duplicate GUID | Impl `821–877` | P2 |
| Compressed chunks / legacy texture alloc rejection | Impl `593–617` | P2 |
| Bad `total_header_size` | Impl `700–709`; table location boundaries now covered directly | P1 |
| 32-bit vs 64-bit export serial sizes | Impl `1203–1219`; fold into the P0 import/export layout matrix | P0 |
| `read_archive_bool` invalid | Impl `1222–1234` | P2 |

---

## 4. `src/property.rs` — 11 unit tests

**Strong today:** multi-record complete-type-name streams; Array inner type params;
type-name depth/negative-inner limits; negative tag size; pre-version rejection;
optional array-index/GUID/property extensions; root class extensions and reserved
group rejection; ArchiveError→PropertyError for AllocationLimit.

| Gap | Evidence | Pri |
|-----|----------|:---:|
| Type-tree allocation limit independent of depth | Impl `410–415`; negative/depth cases covered | P1 |
| `resolve_name_ref` with `number != 0` | Impl `503–507` | P1 |
| `PropertyTagFlags` helpers (`bool_value`, `is_skipped`, …) | Impl `35–54` | P2 |
| `PropertyError::from` non-AllocationLimit kinds | Impl `216–225` | P2 |

**Property-test candidates:** nested type trees under `MAX_PROPERTY_TYPE_DEPTH`;
random tag flag combinations under size caps.

---

## 5. `src/codec.rs` — 27 unit tests

**Strong today:** strict table-backed SoftObject payload/index validation;
Bool/signed/unsigned/float/double/Name/Str/Object/Class scalar
matrix; EnumProperty; Weak/Lazy object; Text empty/keyed; Name array
(len only); absurd array count; nested struct + Vector f64; struct depth;
Enum trailing→Raw; ByteProperty enum vs plain vs byte array; SoftObject inline /
subpath / indexed / indexed array; Name set (len); Int→Str map; Map `INDEX_NONE`;
unsupported→Raw; Vector f32 layout.

### Other codec gaps

| Gap | Evidence | Pri |
|-----|----------|:---:|
| **BUG:** modern inline SoftObject layout must be version/name-map aware | UE `SoftObjectPath.cpp:543–590` uses legacy FString, intermediate FName, or modern FTopLevelAssetPath; `archive.rs:473–484` always reads two FStrings and current tests encode that obsolete form | P0 |
| SoftObject as **Map key/value** (not just array) | Map uses shared `decode_container_element`; add one regression case to the corrected SoftObject slice, not a separate algorithm | P1 |
| Map removal behavior | Positive keys and invalid negative counts now covered; add allocation-limit stress only | P2 |
| `flags.is_skipped()` / unresolved type name | Impl `63–75` | P1 |
| Fixed-size container elements beyond plain Byte + Name | `fixed_serialized_size` `553–569`; enum-backed Byte as array element untested | P1 |
| Vector unsupported size error | Impl `703–709` | P2 |
| Text unsupported `history_type` → None/Raw | Impl `688` | P2 |
| Missing/unresolved map/array inner type params | Impl `578–641` | P1 |

---

## 6. `src/asset.rs` — 27 unit tests

**Strong today:** DataTable (minimal, row_struct, composite parents, bad marker,
trailing, absurd count); CurveTable **Rich** only; StringTable entries + metadata
reject; Enum entries/display/bad CppForm; Struct Int/Bool + unsupported + depth;
Skeleton bones (editor ExportName on); UObject scalar/tail/zero-footer/ImportData
JSON; class-path helpers; `decode_export` prefers DataTable; DataAsset minimal +
trailing.

| Gap | Evidence | Pri |
|-----|----------|:---:|
| **BUG:** CurveTable mode-byte custom-version boundary | UE `CurveTable.cpp:108–123` omits the mode before `ShrinkCurveTableSize`; parser `541–552` always consumes it | P1 |
| CurveTable `SimpleCurves` + `decode_simple_curve_keys` | Impl `543–571`, `1472+`; unit only Rich (`2225`); UE uses a distinct tagged FSimpleCurve representation | P1 |
| CurveTable modern `Empty` mode | Zero-row discriminator; cover in the custom-version matrix | P2 |
| `decode_export` branches for Curve/String/DataAsset/Enum/Struct | Direct `.decode()` only; dispatch untested except DT/Skeleton/UObject | P1 |
| `decode_export` → `Ok(None)` (zero size / missing class / no decoder) | Impl `1284–1315` | P2 |
| Skeleton `FILTER_EDITOR_ONLY` skips ExportName | Impl `1171–1176` | P1 |
| Skeleton non-zero object-guid footer | Impl `1222–1227` | P2 |
| Enum/StringTable/Struct trailing-byte / negative-count edges | Various | P2 |
| **AnimSequence / Montage / BlendSpace** (backlog) | No tests; docs prioritize — **TDD when picked up**, not speculative | P0* |
| Enum/Skeleton in fixture contract | No `enums`/`skeletons` JSON sections; no `tests/` references | P2 |

\*P0 for the work item when started; not a standing “write Anim tests now” item.

**Fixture note:** Electroswag covers DT/DA/CT/ST/UObject well when ignored tests
run; Struct is hardcoded `S_E2EFixture` only; Enum/Skeleton are unit-only.

---

## 7. CLI / JSON (`src/bin/uasset.rs`, `tests/cli.rs`, `tests/tiny_corpus.rs`)

**Strong today:** arg-parse contracts (12 units); every `DecodedAsset` mapper kind,
representative StringTable JSON shape, and inspect text rendering; CLI error JSON; synthetic
utrace inspect/coverage/html; tiny empty-package inspect.

| Gap | Evidence | Pri |
|-----|----------|:---:|
| Portable `status: partial` + `decode_errors` + exit 6 | Only ignored `cli.rs:103–127` (`UASSET_PARTIAL_SAMPLE`) | P0 |
| SCHEMA / exhaustive variant mapping when adding `DecodedAsset` | Pain already noted in memory docs | P1 |
| utrace dashboard **text** render | Large `1146–1704`; HTML has smoke contains-checks only | P2 |

`tiny_corpus` inspect has **zero exports** → never hits asset output mapping.

---

## 8. UTrace — `EVENT_COVERAGE` vs tests

Table: **73 rows** in `utrace.rs:1598–2037` — **5 Decoded**, **68 Partial**, **0 Raw
in table** (absent → Raw). `Cpu.*` is Partial via `decode_status_for`, not table rows.

### Decoded (all five have portable payload tests)

| Event | Unit payload test? | Evidence |
|-------|:------------------:|----------|
| `$Trace.NewTrace` | Yes | `decodes_new_trace_and_thread_info` |
| `$Trace.ThreadInfo` | Yes | same |
| `CpuProfiler.EventSpec` | Yes | `decodes_cpu_event_spec_payload` covers Id/Name/File/Line |
| `Misc.BeginFrame` | Yes | `decodes_begin_and_end_frame_payloads` |
| `Misc.EndFrame` | Yes | same |

### Partial families — unit depth

| Family | Unit depth | Gap |
|--------|------------|-----|
| CPU `EventBatchV3` | **Strong** (10+ stack/coroutine/rebase tests) | Full `dashboard()` pipeline still mostly fixture |
| CPU MetadataSpec/Metadata | **Strong** | — |
| Counters Spec+Int+Float | **Strong** | — |
| CSV catalog | **Strong** | — |
| LoadTime (7 events) | **Strong** crafted | — |
| Logging + Session2 | **Strong** | Fixture adds rendered `sample_message` |
| Bookmarks + all region variants | **Strong** | — |
| Trace channels | **Strong** | — |
| IoStore | Partial | Create/Start/Complete only; **Failed/Unresolved untested** but share the simple Cycle+RequestHandle layout — one P2 lifecycle test |
| Thread groups | Thin | Begin + `state.end()`; **no `ThreadGroupEnd` event decode** |
| Lightweight (ThreadTiming, EndThread, MemoryScope, MetadataStack, Slate) | Thin one-shot | — |
| **GpuProfiler current protocol (12 events)** | **None** for decode | Only latency math + frame/timeline **cap helpers**; all pairing is ignored fixture |
| **GpuProfiler legacy protocol** | None / absent from table | UE 5.7 still declares deprecated `EventSpec`, `Frame`, `Frame2`; decide explicitly whether older/current legacy captures are supported |
| **Memory (13 table events; more current UE events absent)** | Thin | `utrace_memory`: root Alloc/Free + outstanding bound; **no Init v1/v2, TagSpec, system/video/realloc** tests; test invalid Init versions/size reconstruction before allocations |
| Stats Spec | Spec only | No sample events (table admits this) |

### Transport / protocol edges untested

Transport ≠ 4; protocol &gt; 7; **protocol 4–6 field readers** (all synthetic traces use
protocol 7); sync/non-sync size mismatches; header metadata_size ≠ 0; multi-packet
framing beyond single synthetic packet.

### UE-source/table mismatches

- `Memory.ReallocAllocVideo` / `ReallocFreeVideo` are accepted aliases in the
  parser but are **not declared by UE 5.7.2**. Do not add coverage rows or tests
  unless an older supported UE source version proves these event names existed.
- Current UE 5.7.2 declares `Memory.Marker`, `UpdateAlloc`, `MemorySwapOp`,
  `HeapSpec`, `HeapMarkAlloc`, and `HeapUnmarkAlloc`; all are absent from
  `EVENT_COVERAGE` and therefore Raw. Inventory these before adding provider
  behavior; `MemoryTrace.cpp:73`, `141–165`.
- UE 5.7 still declares the deprecated GPU `EventSpec`, `Frame`, and `Frame2`
  alongside the current protocol. The table covers only the latter.
- `event_coverage_table_is_consistent` checks table↔status mapping, **not**
  “Decoded ⇒ has behavioral test.”

### Ignored fixtures carry (not default CI)

Full CLI inspect/dashboard/coverage/inventory on real traces; scaled CPU+GPU
dashboard; Begin/EndFrame; EventSpec at scale; IoStore fail paths; Memory Init/tags;
targeted provider corpus. See `tests/utrace_fixture.rs` (9× `#[ignore]`).

---

## 9. `src/utrace_dispatch.rs` — 8 unit tests

**Strong today:** cross-thread serial order, 24-bit wrap, provisional vs genuine
gaps, no-sync peel, circular run-start, non-wrap false ring, ambiguous epoch reject.

| Gap | Pri |
|-----|:---:|
| Integration: dispatch → provider pipeline inside unit `dashboard()` | P2 |
| Real multi-thread packet reader (not synthetic event lists) | P2 |

---

## 10. Fixtures, fuzz, helpers

| Item | Evidence | Pri |
|------|----------|:---:|
| Grow committed tiny fixtures so SoftObjectPaths / partial decode / BeginFrame don’t need Perforce | SoftObjectPaths + partial + frames are ignored-only today; enabling technique after focused crafted units | P1 |
| `UASSET_REQUIRE_FIXTURE` / `UTRACE_REQUIRE_FIXTURE` documentation mismatch | README documents enforcement, but neither variable appears in the harness; UTrace fixture tests are also `#[ignore]`, so the documented command without `--ignored` runs none of them | P1 |
| Fuzz semantic asserts (reject vs Raw), not panic-only | `package_parse`, `property_stream`, `utrace_packets` | P2 |
| `test_support`: wide `push_fstring`, `name_ref` number≠0 | Helpers ANSI-only / number=0 heavy | P2 |

### Lower priority (don’t pad ratio here)

Bulk mesh/texture/audio buffers; full Insights UI parity; huge dashboard JSON
goldens; `schema.rs` trait seam; `web/` UI.

---

## Effort × impact ranking

Ordered by expected impact per unit effort. Effort includes implementation and
portable regression coverage. Re-rank when a prerequisite lands or UE-source
comparison changes the understood contract.

| Rank | Work | Effort | Impact | Leverage | Status |
|-----:|------|:------:|:------:|:--------:|--------|
| 1 | Strict table-backed SoftObject index validation | S | Very high | Excellent | **Done** |
| 2 | CLI asset-output + text-render contracts | S–M | High | Excellent | **Done** |
| 3 | Modern inline SoftObject layouts | M | Very high | Excellent | Open |
| 4 | Portable partial-decode fixture + exit 6 | M | High | High | Open |
| 5 | `PACKAGE_SAVED_HASH` boundary | S–M | High | High | **Done** |
| 6 | Legacy CurveTable mode boundary | M | High | High | Open |
| 7 | Export-span and table-location failures | S–M | High | High | **Done** |
| 8 | Import/export record layout matrices | M | Very high | High | Open |
| 9 | SoftObject path-list custom-version matrix | M | High | Medium–high | Open |
| 10 | `FILTER_EDITOR_ONLY` Skeleton behavior | S | Medium | Medium–high | Open |
| 11 | Codec missing-inner-type/error cases | S | Medium | Medium–high | Open |
| 12 | CurveTable `SimpleCurves` | S–M | Medium | Medium | Open; follows mode fix |
| 13 | Memory.Init v1/v2 + invalid versions | M | High | Medium | Open |
| 14 | IoStore Failed/Unresolved lifecycle | S | Low–medium | Medium | Open |
| 15 | Current GPU protocol crafted packets | M–L | High | Medium | Open |
| 16 | Current UE 5.7 Memory inventory/decoding | L | High | Medium–low | Open |
| 17 | Nonempty import/export integration fixture | L | High | Medium–low | Open |
| 18 | Legacy GPU protocol support | L–XL | Medium | Low | Product decision |
| 19 | Property/generative fuzz enrichment | M–L | Medium | Low initially | Open |
| 20 | AnimSequence/Montage/BlendSpace | L–XL | Deferred | Deferred | When implementation starts |

---

## Suggested next slices (ordered by evidence leverage)

1. **Fix and prove remaining SoftObject wire contracts:** modern inline layouts,
   versioned subpaths, then one Map regression case. Strict table indices are done.
2. **Property tag optional fields + root class extensions** (alignment envelope).
3. **Package structural boundary matrices:** import/export gates,
   `FILTER_EDITOR_ONLY`, 32/64-bit serial locations. `PACKAGE_SAVED_HASH` is done.
4. **CLI portable partial-decode fixture.** Asset variant dispatch/text is done.
5. **GpuProfiler current-protocol crafted-packet suite**; separately decide
   whether legacy `EventSpec`/`Frame`/`Frame2` support is in scope.
6. **Memory Init version matrix + current UE 5.7 event inventory/provider work**;
   keep IoStore Failed/Unresolved as a small P2 lifecycle addition.
7. **Fix the legacy CurveTable mode boundary, then test SimpleCurves**; fold
   modern Empty into that matrix.
8. When Anim* work starts: red tests from UE Serialize first.

When you add a decoder or promote coverage status, update this file in the same
change — or the catalog drifts again.
