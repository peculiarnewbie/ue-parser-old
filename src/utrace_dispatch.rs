//! Offline serial-ordered dispatch for Protocol 5+ normal events.
//!
//! Mirrors Unreal's `FProtocol5Stage::OnDataNormal` / `DispatchNormalEvents` /
//! `DetectSerialGaps` for complete files: parse per-thread, peel leading
//! `no_sync` events, then min-heap merge by 24-bit serial using modular
//! distance from an origin (UE `FSerialDistancePredicate`).
//!
//! Gap classification:
//! - `sync_count >= 3`: gaps are reported as `genuine` (offline analogue of
//!   Unreal's three-sync settle).
//! - `sync_count < 3`: gaps are reported as `provisional`.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BinaryHeap};

use serde::Serialize;

use crate::Reader;
use crate::utrace::{EventTypeInfo, TraceError, TraceErrorKind};

const SERIAL_BITS: u32 = 24;
const SERIAL_MASK: u32 = (1 << SERIAL_BITS) - 1;
const SERIAL_RANGE: u32 = 1 << SERIAL_BITS;
const SERIAL_HALF: u32 = SERIAL_RANGE / 2;
const SERIAL_BITMAP_WORDS: usize = (SERIAL_RANGE / 64) as usize;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize)]
pub struct TraceSerial(u32);

impl TraceSerial {
    #[must_use]
    pub fn raw(self) -> u32 {
        self.0 & SERIAL_MASK
    }

    fn wrapping_add(self, count: u32) -> Self {
        Self(self.0.wrapping_add(count) & SERIAL_MASK)
    }

    fn distance_from(self, origin: Self) -> u32 {
        self.raw().wrapping_sub(origin.raw()) & SERIAL_MASK
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct SerialDispatchHint {
    origin: Option<TraceSerial>,
    synced_events: usize,
}

/// Incrementally prepares the only capture-wide fact required before serial
/// dispatch. Progressive sessions already frame every normal event, so keeping
/// this fixed 2 MiB bitmap avoids framing the complete capture again at finish.
pub(crate) struct SerialDispatchPreparation {
    serial_bits: Vec<u64>,
    synced_events: usize,
}

impl SerialDispatchPreparation {
    pub(crate) fn new() -> Self {
        Self {
            serial_bits: vec![0_u64; SERIAL_BITMAP_WORDS],
            synced_events: 0,
        }
    }

    pub(crate) fn note(&mut self, raw_serial: Option<u32>) {
        let Some(raw_serial) = raw_serial else {
            return;
        };
        let raw = usize::try_from(raw_serial & SERIAL_MASK).unwrap();
        self.serial_bits[raw / 64] |= 1_u64 << (raw % 64);
        self.synced_events = self.synced_events.saturating_add(1);
    }

    pub(crate) fn finish(self) -> Result<SerialDispatchHint, TraceError> {
        ensure_single_serial_epoch(self.synced_events)?;
        Ok(SerialDispatchHint {
            origin: circular_run_start_bitmap(&self.serial_bits).map(TraceSerial),
            synced_events: self.synced_events,
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum SerialGapKind {
    Genuine,
    Provisional,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SerialGap {
    pub after_serial: u32,
    pub missing_count: u32,
    pub kind: SerialGapKind,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct SerialDispatchSummary {
    pub serial_ordered: bool,
    pub synced_event_count: u64,
    pub unsynced_event_count: u64,
    pub dispatched_event_count: u64,
    pub gap_count: u64,
    pub missing_serial_count: u64,
    pub sync_count: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub gaps: Vec<SerialGap>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DispatchedNormalEvent {
    pub thread_id: u16,
    pub uid: u16,
    pub data: Vec<u8>,
    pub scope_cycle: Option<u64>,
    pub serial: Option<TraceSerial>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct ThreadEvent {
    uid: u16,
    data_start: u32,
    data_end: u32,
    scope_cycle: Option<u64>,
    /// `None` means well-known or `no_sync` (UE Ignored).
    serial: Option<TraceSerial>,
}

struct ThreadCursor<'a> {
    reader: Reader<'a>,
    stream: &'a [u8],
    scope_cycles: Vec<u64>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct PreparedNormalEvents {
    threads: BTreeMap<u16, PreparedThreadEvents>,
}

/// Columnar event boundaries captured by the progressive parser.
///
/// The retained thread stream remains the source of truth for UIDs and serials.
/// A sequential cursor only needs each complete event's wire length to skip its
/// payload and auxiliary chain without framing those bytes a second time.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct PreparedThreadEvents {
    wire_lengths: Vec<u16>,
    overflow_event_indices: Vec<u32>,
    overflow_wire_lengths: Vec<u32>,
}

impl PreparedNormalEvents {
    pub(crate) fn record(&mut self, thread_id: u16, wire_length: usize) -> Result<(), TraceError> {
        let wire_length = u32::try_from(wire_length).map_err(|_| {
            TraceError::new(
                TraceErrorKind::ResourceLimit,
                u64::try_from(wire_length).unwrap_or(u64::MAX),
                "Events.Data",
                "normal event wire length exceeds u32",
            )
        })?;
        if wire_length == 0 {
            return Err(TraceError::new(
                TraceErrorKind::MalformedData,
                0,
                "Events.Data",
                "normal event wire length is zero",
            ));
        }
        let thread = self.threads.entry(thread_id).or_default();
        let event_index = u32::try_from(thread.wire_lengths.len()).map_err(|_| {
            TraceError::new(
                TraceErrorKind::ResourceLimit,
                u64::try_from(thread.wire_lengths.len()).unwrap_or(u64::MAX),
                "Events.Data",
                "normal event count exceeds u32",
            )
        })?;
        if let Ok(wire_length) = u16::try_from(wire_length) {
            thread.wire_lengths.push(wire_length);
        } else {
            thread.wire_lengths.push(0);
            thread.overflow_event_indices.push(event_index);
            thread.overflow_wire_lengths.push(wire_length);
        }
        Ok(())
    }
}

enum DispatchThreadCursor<'a> {
    Parsed(ThreadCursor<'a>),
    Prepared {
        stream: &'a [u8],
        events: &'a PreparedThreadEvents,
        index: usize,
        stream_offset: u32,
        overflow_index: usize,
        scope_cycles: Vec<u64>,
    },
}

impl<'a> DispatchThreadCursor<'a> {
    fn stream(&self) -> &'a [u8] {
        match self {
            Self::Parsed(cursor) => cursor.stream,
            Self::Prepared { stream, .. } => stream,
        }
    }

    fn next_event(
        &mut self,
        layouts: &[Option<NormalEventLayout>],
    ) -> Result<Option<ThreadEvent>, TraceError> {
        match self {
            Self::Parsed(cursor) => cursor.next_event(layouts),
            Self::Prepared {
                stream,
                events,
                index,
                stream_offset,
                overflow_index,
                scope_cycles,
            } => loop {
                let Some(&encoded_wire_length) = events.wire_lengths.get(*index) else {
                    if usize::try_from(*stream_offset).unwrap() != stream.len() {
                        return Err(TraceError::new(
                            TraceErrorKind::MalformedData,
                            u64::from(*stream_offset),
                            "Events.Data",
                            "prepared normal events do not cover the thread stream",
                        ));
                    }
                    return Ok(None);
                };
                let wire_length = match encoded_wire_length {
                    0 => {
                        let Some(&overflow_event_index) =
                            events.overflow_event_indices.get(*overflow_index)
                        else {
                            return Err(invalid_prepared_events(*stream_offset));
                        };
                        if usize::try_from(overflow_event_index).unwrap() != *index {
                            return Err(invalid_prepared_events(*stream_offset));
                        }
                        let Some(&wire_length) = events.overflow_wire_lengths.get(*overflow_index)
                        else {
                            return Err(invalid_prepared_events(*stream_offset));
                        };
                        *overflow_index += 1;
                        wire_length
                    }
                    wire_length => u32::from(wire_length),
                };
                let event_start = *stream_offset;
                let data_end = event_start.checked_add(wire_length).ok_or_else(|| {
                    TraceError::new(
                        TraceErrorKind::ResourceLimit,
                        u64::from(event_start),
                        "Events.Data",
                        "prepared normal event end overflow",
                    )
                })?;
                let event_start_usize = usize::try_from(event_start).unwrap();
                let Some(&first) = stream.get(event_start_usize) else {
                    return Err(invalid_prepared_events(event_start));
                };
                let (uid, uid_size) = if first & 1 != 0 {
                    let Some(&second) = stream.get(event_start_usize + 1) else {
                        return Err(invalid_prepared_events(event_start));
                    };
                    ((u16::from(first) | (u16::from(second) << 8)) >> 1, 2)
                } else {
                    (u16::from(first) >> 1, 1)
                };
                let synced = if uid < 16 {
                    false
                } else {
                    let Some(layout) = layouts.get(usize::from(uid)).copied().flatten() else {
                        return Err(TraceError::new(
                            TraceErrorKind::MalformedData,
                            u64::from(event_start),
                            "Events.Uid",
                            format!("unknown event uid {uid} in prepared normal stream"),
                        ));
                    };
                    !layout.no_sync
                };
                let serial = if synced {
                    let serial_start = event_start_usize + uid_size;
                    let Some(bytes) = stream.get(serial_start..serial_start + 3) else {
                        return Err(invalid_prepared_events(event_start));
                    };
                    Some(TraceSerial(
                        u32::from(bytes[0])
                            | (u32::from(bytes[1]) << 8)
                            | (u32::from(bytes[2]) << 16),
                    ))
                } else {
                    None
                };
                let header_size = if uid == 1 {
                    4
                } else {
                    uid_size + usize::from(synced) * 3
                };
                let data_start = event_start
                    .checked_add(u32::try_from(header_size).unwrap())
                    .ok_or_else(|| invalid_prepared_events(event_start))?;
                if data_start > data_end || usize::try_from(data_end).unwrap() > stream.len() {
                    return Err(invalid_prepared_events(event_start));
                }
                let data = &stream
                    [usize::try_from(data_start).unwrap()..usize::try_from(data_end).unwrap()];
                match uid {
                    6 | 8 => {
                        if let Some(cycle) = decode_known_scope_cycle(uid, data) {
                            scope_cycles.push(cycle);
                        }
                    }
                    7 | 9 => {
                        scope_cycles.pop();
                    }
                    _ => {}
                }
                let scope_cycle = (uid >= 16).then(|| scope_cycles.last().copied()).flatten();
                *stream_offset = data_end;
                *index += 1;
                if uid == 3 {
                    continue;
                }
                return Ok(Some(ThreadEvent {
                    uid,
                    data_start,
                    data_end,
                    scope_cycle,
                    serial,
                }));
            },
        }
    }
}

fn invalid_prepared_events(offset: u32) -> TraceError {
    TraceError::new(
        TraceErrorKind::MalformedData,
        u64::from(offset),
        "Events.Data",
        "prepared normal event columns are inconsistent",
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DispatchedNormalEventView<'a> {
    pub thread_id: u16,
    pub uid: u16,
    pub data: &'a [u8],
    pub scope_cycle: Option<u64>,
    pub serial: Option<TraceSerial>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct HeapEntry {
    distance: u32,
    thread_index: usize,
    serial: TraceSerial,
}

impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        // Min-heap by modular distance from origin (UE FSerialDistancePredicate).
        other
            .distance
            .cmp(&self.distance)
            .then_with(|| other.thread_index.cmp(&self.thread_index))
    }
}

impl PartialOrd for HeapEntry {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ParsedNormalEvent {
    uid: u16,
    offset: usize,
    total_end: usize,
    data_start: usize,
    data_end: usize,
    has_aux: bool,
    serial: Option<TraceSerial>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct NormalEventLayout {
    data_size: usize,
    maybe_has_aux: bool,
    no_sync: bool,
}

fn normal_event_layouts(
    registry: &BTreeMap<u16, &EventTypeInfo>,
) -> Vec<Option<NormalEventLayout>> {
    let max_uid = registry.keys().next_back().copied().unwrap_or(0);
    let mut layouts = vec![None; usize::from(max_uid) + 1];
    for (&uid, event) in registry {
        layouts[usize::from(uid)] = Some(NormalEventLayout {
            data_size: event_data_size(event),
            maybe_has_aux: event.flags.maybe_has_aux,
            no_sync: event.flags.no_sync,
        });
    }
    layouts
}

impl<'a> ThreadCursor<'a> {
    fn new(stream: &'a [u8]) -> Self {
        Self {
            reader: Reader::new(stream),
            stream,
            scope_cycles: Vec::new(),
        }
    }

    fn next_event(
        &mut self,
        layouts: &[Option<NormalEventLayout>],
    ) -> Result<Option<ThreadEvent>, TraceError> {
        while self.reader.remaining() > 0 {
            let parsed = parse_protocol5_normal_event(&mut self.reader, layouts)?;
            if parsed.uid == 3 {
                continue;
            }
            let data = &self.stream[parsed.data_start..parsed.data_end];
            match parsed.uid {
                6 | 8 => {
                    if let Some(cycle) = decode_known_scope_cycle(parsed.uid, data) {
                        self.scope_cycles.push(cycle);
                    }
                }
                7 | 9 => {
                    self.scope_cycles.pop();
                }
                _ => {}
            }
            let scope_cycle = (parsed.uid >= 16)
                .then(|| self.scope_cycles.last().copied())
                .flatten();
            let mut data_end = parsed.data_end;
            if parsed.has_aux {
                let mut aux_chain = 0_u32;
                loop {
                    aux_chain = aux_chain.saturating_add(1);
                    if aux_chain > 64_000 {
                        return Err(TraceError::new(
                            TraceErrorKind::ResourceLimit,
                            self.reader.tell(),
                            "Events.Aux",
                            "aux event chain exceeded 64000 events",
                        ));
                    }
                    let aux = parse_protocol5_normal_event(&mut self.reader, layouts)?;
                    data_end = aux.total_end;
                    match aux.uid {
                        1 => {}
                        3 => break,
                        uid => {
                            return Err(TraceError::new(
                                TraceErrorKind::MalformedData,
                                u64::try_from(aux.offset).unwrap_or(u64::MAX),
                                "Events.Aux",
                                format!("expected AuxData/AuxDataTerminal, got uid {uid}"),
                            ));
                        }
                    }
                }
            }
            return Ok(Some(ThreadEvent {
                uid: parsed.uid,
                data_start: u32::try_from(parsed.data_start).map_err(|_| {
                    TraceError::new(
                        TraceErrorKind::ResourceLimit,
                        u64::try_from(parsed.data_start).unwrap_or(u64::MAX),
                        "Events.Data",
                        "normal event data offset exceeds u32",
                    )
                })?,
                data_end: u32::try_from(data_end).map_err(|_| {
                    TraceError::new(
                        TraceErrorKind::ResourceLimit,
                        u64::try_from(data_end).unwrap_or(u64::MAX),
                        "Events.Data",
                        "normal event data end exceeds u32",
                    )
                })?,
                scope_cycle,
                serial: parsed.serial,
            }));
        }
        Ok(None)
    }
}

/// Dispatch normal events from all threads in global serial order.
pub fn dispatch_normal_events(
    streams: &BTreeMap<u16, Vec<u8>>,
    registry: &BTreeMap<u16, &EventTypeInfo>,
    sync_count: u64,
) -> Result<(Vec<DispatchedNormalEvent>, SerialDispatchSummary), TraceError> {
    let mut out = Vec::new();
    let summary = dispatch_normal_events_with(streams, registry, sync_count, |event| {
        let event_info = registry.get(&event.uid).copied();
        out.push(DispatchedNormalEvent {
            thread_id: event.thread_id,
            uid: event.uid,
            data: owned_event_data(event.data, event_info),
            scope_cycle: event.scope_cycle,
            serial: event.serial,
        });
        Ok(())
    })?;
    Ok((out, summary))
}

pub(crate) fn dispatch_normal_events_with<'a>(
    streams: &'a BTreeMap<u16, Vec<u8>>,
    registry: &BTreeMap<u16, &EventTypeInfo>,
    sync_count: u64,
    visit: impl FnMut(DispatchedNormalEventView<'a>) -> Result<(), TraceError>,
) -> Result<SerialDispatchSummary, TraceError> {
    dispatch_normal_events_with_hint(streams, registry, sync_count, None, None, visit)
}

pub(crate) fn dispatch_normal_events_with_hint<'a>(
    streams: &'a BTreeMap<u16, Vec<u8>>,
    registry: &BTreeMap<u16, &EventTypeInfo>,
    sync_count: u64,
    preparation: Option<SerialDispatchHint>,
    prepared: Option<&'a PreparedNormalEvents>,
    mut visit: impl FnMut(DispatchedNormalEventView<'a>) -> Result<(), TraceError>,
) -> Result<SerialDispatchSummary, TraceError> {
    let layouts = normal_event_layouts(registry);
    let next_serial = match preparation {
        Some(preparation) => {
            ensure_single_serial_epoch(preparation.synced_events)?;
            preparation.origin
        }
        None => serial_origin_from_streams(streams, &layouts)?,
    };
    let mut thread_ids = Vec::new();
    let mut cursors = Vec::new();
    for (thread_id, stream) in streams {
        if *thread_id <= 1 {
            continue;
        }
        thread_ids.push(*thread_id);
        cursors.push(
            if let Some(events) = prepared.and_then(|prepared| prepared.threads.get(thread_id)) {
                DispatchThreadCursor::Prepared {
                    stream,
                    events,
                    index: 0,
                    stream_offset: 0,
                    overflow_index: 0,
                    scope_cycles: Vec::new(),
                }
            } else {
                DispatchThreadCursor::Parsed(ThreadCursor::new(stream))
            },
        );
    }
    let mut pending = cursors
        .iter_mut()
        .map(|cursor| cursor.next_event(&layouts))
        .collect::<Result<Vec<_>, _>>()?;
    let mut summary = SerialDispatchSummary {
        serial_ordered: true,
        sync_count,
        ..SerialDispatchSummary::default()
    };

    // Peel leading unsynchronized events from every thread.
    for thread_index in 0..cursors.len() {
        while pending[thread_index].is_some_and(|event| event.serial.is_none()) {
            let event = pending[thread_index]
                .take()
                .expect("pending event was checked");
            visit_taken(
                cursors[thread_index].stream(),
                &mut visit,
                &mut summary,
                thread_ids[thread_index],
                event,
            )?;
            pending[thread_index] = cursors[thread_index].next_event(&layouts)?;
        }
    }

    let mut next_serial = next_serial;
    let mut heap = BinaryHeap::new();
    if let Some(origin) = next_serial {
        for (thread_index, event) in pending.iter().enumerate() {
            if let Some(entry) = heap_entry_for_pending(thread_index, *event, origin) {
                heap.push(entry);
            }
        }
    }

    let gap_kind = if sync_count >= 3 {
        SerialGapKind::Genuine
    } else {
        SerialGapKind::Provisional
    };
    let mut last_observed_serial = None;

    while let Some(top) = heap.pop() {
        let Some(mut expected) = next_serial else {
            break;
        };
        // Offline complete files: skip forward to the next available serial.
        // Holes are derived from the observed serial set after dispatch.
        if top.serial != expected {
            expected = top.serial;
            next_serial = Some(expected);
        }

        let thread_index = top.thread_index;
        loop {
            let Some(event) = pending[thread_index] else {
                break;
            };
            let serial = event.serial;
            match serial {
                Some(serial) if serial == expected => {
                    record_gap(&mut summary, last_observed_serial, serial.raw(), gap_kind);
                    last_observed_serial = Some(serial.raw());
                    pending[thread_index] = None;
                    visit_taken(
                        cursors[thread_index].stream(),
                        &mut visit,
                        &mut summary,
                        thread_ids[thread_index],
                        event,
                    )?;
                    pending[thread_index] = cursors[thread_index].next_event(&layouts)?;
                    expected = expected.wrapping_add(1);
                    next_serial = Some(expected);
                }
                None => {
                    pending[thread_index] = None;
                    visit_taken(
                        cursors[thread_index].stream(),
                        &mut visit,
                        &mut summary,
                        thread_ids[thread_index],
                        event,
                    )?;
                    pending[thread_index] = cursors[thread_index].next_event(&layouts)?;
                }
                Some(_) => break,
            }
        }

        if let Some(origin) = next_serial {
            if let Some(entry) = heap_entry_for_pending(thread_index, pending[thread_index], origin)
            {
                heap.push(entry);
            }
        }
    }
    Ok(summary)
}

fn ensure_single_serial_epoch(synced_events: usize) -> Result<(), TraceError> {
    if synced_events >= usize::try_from(SERIAL_RANGE).unwrap() {
        return Err(TraceError::new(
            TraceErrorKind::UnsupportedFormat,
            0,
            "Events.Serial",
            "offline dispatch cannot disambiguate a complete 24-bit serial epoch",
        ));
    }
    Ok(())
}

fn visit_taken<'a>(
    stream: &'a [u8],
    visit: &mut impl FnMut(DispatchedNormalEventView<'a>) -> Result<(), TraceError>,
    summary: &mut SerialDispatchSummary,
    thread_id: u16,
    event: ThreadEvent,
) -> Result<(), TraceError> {
    if event.serial.is_some() {
        summary.synced_event_count += 1;
    } else {
        summary.unsynced_event_count += 1;
    }
    summary.dispatched_event_count = summary.dispatched_event_count.saturating_add(1);
    let data_start = usize::try_from(event.data_start).unwrap();
    let data_end = usize::try_from(event.data_end).unwrap();
    let data = stream.get(data_start..data_end).ok_or_else(|| {
        TraceError::new(
            TraceErrorKind::MalformedData,
            u64::from(event.data_start),
            "Events.Data",
            "normal event descriptor points outside its thread stream",
        )
    })?;
    visit(DispatchedNormalEventView {
        thread_id,
        uid: event.uid,
        data,
        scope_cycle: event.scope_cycle,
        serial: event.serial,
    })
}

fn owned_event_data(data: &[u8], event: Option<&EventTypeInfo>) -> Vec<u8> {
    let mut owned = data.to_vec();
    let Some(event) = event else {
        return owned;
    };
    let mut cursor = event_data_size(event);
    while cursor < owned.len() {
        match owned[cursor] {
            2 => owned[cursor] = 1,
            6 => {
                owned[cursor] = 3;
                break;
            }
            1 => {}
            3 => break,
            _ => break,
        }
        if owned.len().saturating_sub(cursor) < 4 {
            break;
        }
        let pack = u32::from_le_bytes(owned[cursor..cursor + 4].try_into().unwrap());
        let size = usize::try_from(pack >> 13).unwrap();
        let Some(next) = cursor
            .checked_add(4)
            .and_then(|start| start.checked_add(size))
        else {
            break;
        };
        cursor = next;
    }
    owned
}

/// Find the serial origin without retaining one descriptor per event.
///
/// Protocol 5 serials occupy a fixed 24-bit domain, so a 2 MiB bitmap is both
/// bounded and substantially smaller than buffering millions of serials. The
/// dispatch pass then reparses each thread while holding only one pending event
/// per thread.
fn serial_origin_from_streams(
    streams: &BTreeMap<u16, Vec<u8>>,
    layouts: &[Option<NormalEventLayout>],
) -> Result<Option<TraceSerial>, TraceError> {
    let word_count = usize::try_from(SERIAL_RANGE / 64).unwrap();
    let mut serial_bits = vec![0_u64; word_count];
    let mut synced_events = 0_usize;

    for (thread_id, stream) in streams {
        if *thread_id <= 1 {
            continue;
        }
        let mut cursor = ThreadCursor::new(stream);
        while let Some(event) = cursor.next_event(layouts)? {
            let Some(serial) = event.serial else {
                continue;
            };
            synced_events = synced_events.checked_add(1).ok_or_else(|| {
                TraceError::new(
                    TraceErrorKind::ResourceLimit,
                    0,
                    "Events.Serial",
                    "normal event count exceeds addressable memory",
                )
            })?;
            let raw = usize::try_from(serial.raw()).unwrap();
            serial_bits[raw / 64] |= 1_u64 << (raw % 64);
        }
    }

    ensure_single_serial_epoch(synced_events)?;
    Ok(circular_run_start_bitmap(&serial_bits).map(TraceSerial))
}

fn circular_run_start_bitmap(words: &[u64]) -> Option<u32> {
    let mut first = None;
    let mut previous = None;
    let mut best_gap = 0_u32;
    let mut best_next = None;

    for (word_index, encoded_word) in words.iter().copied().enumerate() {
        let mut word = encoded_word;
        while word != 0 {
            let bit_index = word.trailing_zeros();
            let value = u32::try_from(word_index)
                .unwrap()
                .saturating_mul(64)
                .saturating_add(bit_index);
            first.get_or_insert(value);
            best_next.get_or_insert(value);
            if let Some(left) = previous {
                let gap = value.wrapping_sub(left) & SERIAL_MASK;
                if gap > best_gap {
                    best_gap = gap;
                    best_next = Some(value);
                }
            }
            previous = Some(value);
            word &= word - 1;
        }
    }

    let first = first?;
    let last = previous.unwrap_or(first);
    let wrap_gap = first.wrapping_add(SERIAL_RANGE).wrapping_sub(last) & SERIAL_MASK;
    if wrap_gap > best_gap {
        Some(first)
    } else {
        best_next
    }
}

#[cfg(test)]
fn circular_run_start(values: &[u32]) -> Option<u32> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    if sorted.len() == 1 {
        return Some(sorted[0]);
    }

    let mut best_gap = 0_u32;
    let mut best_next = sorted[0];
    for window in sorted.windows(2) {
        let gap = window[1].wrapping_sub(window[0]) & SERIAL_MASK;
        if gap > best_gap {
            best_gap = gap;
            best_next = window[1];
        }
    }
    // Wrap edge: last -> first.
    let wrap_gap = sorted[0]
        .wrapping_add(SERIAL_RANGE)
        .wrapping_sub(sorted[sorted.len() - 1])
        & SERIAL_MASK;
    if wrap_gap > best_gap {
        best_next = sorted[0];
    }
    Some(best_next)
}

#[cfg(test)]
fn serial_gaps_from_observed(observed: &[u32], kind: SerialGapKind) -> Vec<SerialGap> {
    if observed.len() < 2 {
        return Vec::new();
    }
    let mut gaps = Vec::new();
    for window in observed.windows(2) {
        push_gap_if_small(&mut gaps, window[0], window[1], kind);
    }
    gaps
}

#[cfg(test)]
fn push_gap_if_small(gaps: &mut Vec<SerialGap>, left: u32, right: u32, kind: SerialGapKind) {
    if let Some(gap) = serial_gap(left, right, kind) {
        gaps.push(gap);
    }
}

fn serial_gap(left: u32, right: u32, kind: SerialGapKind) -> Option<SerialGap> {
    let forward = right.wrapping_sub(left) & SERIAL_MASK;
    if forward == 0 || forward >= SERIAL_HALF {
        return None;
    }
    let missing_count = forward.saturating_sub(1);
    (missing_count != 0).then_some(SerialGap {
        after_serial: left.wrapping_add(1) & SERIAL_MASK,
        missing_count,
        kind,
    })
}

fn record_gap(
    summary: &mut SerialDispatchSummary,
    left: Option<u32>,
    right: u32,
    kind: SerialGapKind,
) {
    let Some(gap) = left.and_then(|left| serial_gap(left, right, kind)) else {
        return;
    };
    summary.gap_count = summary.gap_count.saturating_add(1);
    summary.missing_serial_count = summary
        .missing_serial_count
        .saturating_add(u64::from(gap.missing_count));
    if summary.gaps.len() < 64 {
        summary.gaps.push(gap);
    }
}

fn heap_entry_for_pending(
    thread_index: usize,
    event: Option<ThreadEvent>,
    origin: TraceSerial,
) -> Option<HeapEntry> {
    let event = event?;
    let serial = event.serial?;
    Some(HeapEntry {
        distance: serial.distance_from(origin),
        thread_index,
        serial,
    })
}

fn decode_known_scope_cycle(uid: u16, data: &[u8]) -> Option<u64> {
    match uid {
        6 if data.len() == 8 => Some(u64::from_le_bytes(data.try_into().ok()?)),
        8 if data.len() == 7 => {
            let mut bytes = [0_u8; 8];
            bytes[..7].copy_from_slice(data);
            Some(u64::from_le_bytes(bytes))
        }
        _ => None,
    }
}

fn read_serial_24(reader: &mut Reader<'_>) -> Result<TraceSerial, TraceError> {
    let b0 = u32::from(reader.read_u8("Events.Serial[0]")?);
    let b1 = u32::from(reader.read_u8("Events.Serial[1]")?);
    let b2 = u32::from(reader.read_u8("Events.Serial[2]")?);
    Ok(TraceSerial((b0 | (b1 << 8) | (b2 << 16)) & SERIAL_MASK))
}

fn parse_protocol5_normal_event(
    reader: &mut Reader<'_>,
    layouts: &[Option<NormalEventLayout>],
) -> Result<ParsedNormalEvent, TraceError> {
    const USER_UID: u16 = 16;
    let offset = usize::try_from(reader.tell()).unwrap();
    let first = reader.read_u8("Events.Uid")?;
    let raw_uid = if (first & 1) != 0 {
        let second = reader.read_u8("Events.Uid")?;
        u16::from(first) | (u16::from(second) << 8)
    } else {
        u16::from(first)
    };
    let uid = raw_uid >> 1;

    let (event_size, has_aux, serial) = if uid < USER_UID {
        let size = match uid {
            1 => {
                if reader.remaining() < 3 {
                    return Err(TraceError::new(
                        TraceErrorKind::MalformedData,
                        reader.tell(),
                        "Aux.Header",
                        "truncated aux header",
                    ));
                }
                let rest = reader.read_bytes(3, "Aux.Header")?;
                let pack = u32::from(first)
                    | (u32::from(rest[0]) << 8)
                    | (u32::from(rest[1]) << 16)
                    | (u32::from(rest[2]) << 24);
                usize::try_from(pack >> 13).unwrap()
            }
            6 | 7 => 8,
            8 | 9 => 7,
            _ => 0,
        };
        (size, false, None)
    } else {
        let Some(layout) = layouts.get(usize::from(uid)).copied().flatten() else {
            return Err(TraceError::new(
                TraceErrorKind::MalformedData,
                u64::try_from(offset).unwrap(),
                "Events.Uid",
                format!("unknown event uid {uid} in normal stream"),
            ));
        };
        let serial = if layout.no_sync {
            None
        } else {
            Some(read_serial_24(reader)?)
        };
        (layout.data_size, layout.maybe_has_aux, serial)
    };

    let data_start = usize::try_from(reader.tell()).unwrap();
    reader.skip(u64::try_from(event_size).unwrap(), "Events.Data")?;
    let data_end = usize::try_from(reader.tell()).unwrap();
    Ok(ParsedNormalEvent {
        uid,
        offset,
        total_end: data_end,
        data_start,
        data_end,
        has_aux,
        serial,
    })
}

fn event_data_size(event: &EventTypeInfo) -> usize {
    event
        .fields
        .iter()
        .map(|field| usize::from(field.offset) + usize::from(field.size))
        .max()
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utrace::EventFlags;

    fn synced_event(uid: u16, name: &str) -> EventTypeInfo {
        EventTypeInfo {
            uid,
            logger: "Test".to_owned(),
            event: name.to_owned(),
            flags: EventFlags {
                important: false,
                maybe_has_aux: false,
                no_sync: false,
                definition: false,
            },
            fields: vec![crate::utrace::FieldInfo {
                name: "Value".to_owned(),
                offset: 0,
                size: 1,
                family: crate::utrace::FieldFamily::Regular,
                type_name: "uint8".to_owned(),
                ref_uid: None,
            }],
        }
    }

    fn no_sync_event(uid: u16, name: &str) -> EventTypeInfo {
        let mut event = synced_event(uid, name);
        event.flags.no_sync = true;
        event
    }

    fn synced_event_with_aux(uid: u16, name: &str) -> EventTypeInfo {
        let mut event = synced_event(uid, name);
        event.flags.maybe_has_aux = true;
        event
    }

    fn encode_uid(uid: u16) -> Vec<u8> {
        let shifted = uid << 1;
        if shifted < 128 {
            vec![shifted as u8]
        } else {
            vec![(shifted as u8) | 1, (shifted >> 8) as u8]
        }
    }

    fn push_synced(stream: &mut Vec<u8>, uid: u16, serial: u32, payload: u8) {
        stream.extend(encode_uid(uid));
        stream.push((serial & 0xff) as u8);
        stream.push(((serial >> 8) & 0xff) as u8);
        stream.push(((serial >> 16) & 0xff) as u8);
        stream.push(payload);
    }

    fn push_unsynced(stream: &mut Vec<u8>, uid: u16, payload: u8) {
        stream.extend(encode_uid(uid));
        stream.push(payload);
    }

    fn push_synced_with_aux(
        stream: &mut Vec<u8>,
        uid: u16,
        serial: u32,
        payload: u8,
        aux_payload: &[u8],
    ) {
        push_synced(stream, uid, serial, payload);
        let pack = (u32::try_from(aux_payload.len()).unwrap() << 13) | 2;
        stream.extend_from_slice(&pack.to_le_bytes());
        stream.extend_from_slice(aux_payload);
        stream.extend(encode_uid(3));
    }

    #[test]
    fn dispatches_cross_thread_events_by_serial_not_thread_id() {
        let events = [
            synced_event(16, "A"),
            synced_event(17, "B"),
            synced_event(18, "C"),
        ];
        let registry = events
            .iter()
            .map(|event| (event.uid, event))
            .collect::<BTreeMap<_, _>>();

        let mut stream_5 = Vec::new();
        push_synced(&mut stream_5, 17, 2, 0x22);
        let mut stream_10 = Vec::new();
        push_synced(&mut stream_10, 16, 1, 0x11);
        push_synced(&mut stream_10, 18, 3, 0x33);

        let streams = [(5_u16, stream_5), (10_u16, stream_10)]
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        let (dispatched, summary) = dispatch_normal_events(&streams, &registry, 3).unwrap();
        assert_eq!(
            dispatched
                .iter()
                .map(|event| (
                    event.serial.map(TraceSerial::raw),
                    event.thread_id,
                    event.data[0]
                ))
                .collect::<Vec<_>>(),
            vec![(Some(1), 10, 0x11), (Some(2), 5, 0x22), (Some(3), 10, 0x33),]
        );
        assert!(summary.serial_ordered);
        assert_eq!(summary.gap_count, 0);
        assert_eq!(summary.synced_event_count, 3);
    }

    #[test]
    fn prepared_serial_origin_matches_offline_origin_scan() {
        let events = [
            synced_event(16, "A"),
            synced_event_with_aux(17, "B"),
            no_sync_event(18, "C"),
        ];
        let registry = events
            .iter()
            .map(|event| (event.uid, event))
            .collect::<BTreeMap<_, _>>();
        let mut stream_5 = Vec::new();
        push_synced_with_aux(&mut stream_5, 17, SERIAL_MASK, 0x22, &[0xaa, 0xbb]);
        push_synced(&mut stream_5, 16, 2, 0x44);
        let mut stream_10 = Vec::new();
        push_synced(&mut stream_10, 16, SERIAL_MASK - 1, 0x11);
        push_unsynced(&mut stream_10, 18, 0x33);
        push_synced(&mut stream_10, 16, 0, 0x55);
        let streams = [(5_u16, stream_5), (10_u16, stream_10)]
            .into_iter()
            .collect::<BTreeMap<_, _>>();

        let mut preparation = SerialDispatchPreparation::new();
        preparation.note(Some(SERIAL_MASK));
        preparation.note(Some(2));
        preparation.note(Some(SERIAL_MASK - 1));
        preparation.note(None);
        preparation.note(Some(0));
        let hint = preparation.finish().unwrap();
        let mut prepared = Vec::new();
        let prepared_summary =
            dispatch_normal_events_with_hint(&streams, &registry, 3, Some(hint), None, |event| {
                prepared.push((
                    event.thread_id,
                    event.uid,
                    event.serial.map(TraceSerial::raw),
                    event.data.to_vec(),
                ));
                Ok(())
            })
            .unwrap();
        let mut offline = Vec::new();
        let offline_summary = dispatch_normal_events_with(&streams, &registry, 3, |event| {
            offline.push((
                event.thread_id,
                event.uid,
                event.serial.map(TraceSerial::raw),
                event.data.to_vec(),
            ));
            Ok(())
        })
        .unwrap();

        assert_eq!(prepared, offline);
        assert_eq!(prepared_summary, offline_summary);
    }

    #[test]
    fn prepared_columns_match_parsed_dispatch_with_scopes_aux_and_skips() {
        let events = [
            synced_event_with_aux(70, "SyncedAux"),
            no_sync_event(18, "ScopedNoSync"),
        ];
        let registry = events
            .iter()
            .map(|event| (event.uid, event))
            .collect::<BTreeMap<_, _>>();

        let mut stream_5 = Vec::new();
        let scope_start = stream_5.len();
        stream_5.extend(encode_uid(6));
        stream_5.extend_from_slice(&1234_u64.to_le_bytes());
        let scope_length = stream_5.len() - scope_start;
        let unsynced_start = stream_5.len();
        push_unsynced(&mut stream_5, 18, 0x33);
        let unsynced_length = stream_5.len() - unsynced_start;
        let skipped_start = stream_5.len();
        stream_5.extend(encode_uid(3));
        let skipped_length = stream_5.len() - skipped_start;
        let pop_start = stream_5.len();
        stream_5.extend(encode_uid(7));
        stream_5.extend_from_slice(&[0; 8]);
        let pop_length = stream_5.len() - pop_start;
        let synced_start = stream_5.len();
        push_synced_with_aux(&mut stream_5, 70, 9, 0x44, &[0xaa, 0xbb]);
        let synced_length = stream_5.len() - synced_start;

        let streams = [(5_u16, stream_5)].into_iter().collect::<BTreeMap<_, _>>();
        let mut prepared_events = PreparedNormalEvents::default();
        prepared_events.record(5, scope_length).unwrap();
        prepared_events.record(5, unsynced_length).unwrap();
        prepared_events.record(5, skipped_length).unwrap();
        prepared_events.record(5, pop_length).unwrap();
        prepared_events.record(5, synced_length).unwrap();
        let mut preparation = SerialDispatchPreparation::new();
        preparation.note(Some(9));
        let hint = preparation.finish().unwrap();

        let collect = |prepared: Option<&PreparedNormalEvents>| {
            let mut dispatched = Vec::new();
            let summary = dispatch_normal_events_with_hint(
                &streams,
                &registry,
                3,
                Some(hint),
                prepared,
                |event| {
                    dispatched.push((
                        event.thread_id,
                        event.uid,
                        event.data.to_vec(),
                        event.scope_cycle,
                        event.serial.map(TraceSerial::raw),
                    ));
                    Ok(())
                },
            )
            .unwrap();
            (dispatched, summary)
        };
        let parsed = collect(None);
        let prepared = collect(Some(&prepared_events));

        assert_eq!(prepared, parsed);
        assert_eq!(prepared.0[1].1, 18);
        assert_eq!(prepared.0[1].3, Some(1234));
        assert_eq!(prepared.0.last().unwrap().1, 70);
        assert_eq!(prepared.0.last().unwrap().2[1], 2);
    }

    #[test]
    fn prepared_columns_support_overflow_wire_lengths() {
        let mut event = synced_event(16, "Large");
        event.fields[0].size = u16::MAX;
        let registry = [(event.uid, &event)]
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        let mut stream = Vec::new();
        stream.extend(encode_uid(16));
        stream.extend_from_slice(&1_u32.to_le_bytes()[..3]);
        stream.resize(stream.len() + usize::from(u16::MAX), 0xaa);
        let streams = [(5_u16, stream)].into_iter().collect::<BTreeMap<_, _>>();
        let mut prepared = PreparedNormalEvents::default();
        prepared.record(5, streams[&5].len()).unwrap();
        let mut preparation = SerialDispatchPreparation::new();
        preparation.note(Some(1));

        let mut lengths = Vec::new();
        dispatch_normal_events_with_hint(
            &streams,
            &registry,
            3,
            Some(preparation.finish().unwrap()),
            Some(&prepared),
            |event| {
                lengths.push(event.data.len());
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(lengths, vec![usize::from(u16::MAX)]);
    }

    #[test]
    fn borrowed_dispatch_exposes_normal_aux_without_copying() {
        let events = [synced_event_with_aux(16, "WithAux")];
        let registry = events
            .iter()
            .map(|event| (event.uid, event))
            .collect::<BTreeMap<_, _>>();
        let mut stream = Vec::new();
        push_synced_with_aux(&mut stream, 16, 1, 0xaa, &[0x11, 0x22, 0x33]);
        let streams = [(5_u16, stream)].into_iter().collect::<BTreeMap<_, _>>();

        let mut borrowed = Vec::new();
        let mut borrowed_range = None;
        dispatch_normal_events_with(&streams, &registry, 3, |event| {
            borrowed_range = Some((event.data.as_ptr() as usize, event.data.len()));
            borrowed.push(event.data.to_vec());
            Ok(())
        })
        .unwrap();
        let stream = &streams[&5];
        let stream_start = stream.as_ptr() as usize;
        let stream_end = stream_start + stream.len();
        let (borrowed_start, borrowed_len) = borrowed_range.unwrap();
        assert!(borrowed_start >= stream_start);
        assert!(borrowed_start + borrowed_len <= stream_end);
        assert_eq!(borrowed[0][0], 0xaa);
        assert_eq!(borrowed[0][1], 2, "normal-stream AuxData uid stays encoded");
        assert_eq!(*borrowed[0].last().unwrap(), 6);
        let aux = crate::utrace::parse_protocol5_aux(&borrowed[0], 1, 0).unwrap();
        assert_eq!(aux.get(&0).unwrap(), &[0x11, 0x22, 0x33]);

        let (owned, _) = dispatch_normal_events(&streams, &registry, 3).unwrap();
        assert_eq!(owned[0].data[1], 1, "public owned output stays canonical");
        assert_eq!(*owned[0].data.last().unwrap(), 3);
    }

    #[test]
    fn dispatches_across_24bit_serial_wrap() {
        let events = [
            synced_event(16, "A"),
            synced_event(17, "B"),
            synced_event(18, "C"),
        ];
        let registry = events
            .iter()
            .map(|event| (event.uid, event))
            .collect::<BTreeMap<_, _>>();

        // Heads 0xfffffe and 0: wrap-aware origin must be 0xfffffe, not 0.
        let mut stream_a = Vec::new();
        push_synced(&mut stream_a, 16, 0xfffffe, 0xfe);
        push_synced(&mut stream_a, 17, 0xffffff, 0xff);
        let mut stream_b = Vec::new();
        push_synced(&mut stream_b, 18, 0, 0x00);

        let streams = [(5_u16, stream_a), (9_u16, stream_b)]
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        let (dispatched, summary) = dispatch_normal_events(&streams, &registry, 3).unwrap();
        assert_eq!(
            dispatched
                .iter()
                .map(|event| event.serial.map(TraceSerial::raw))
                .collect::<Vec<_>>(),
            vec![Some(0xfffffe), Some(0xffffff), Some(0)]
        );
        assert_eq!(summary.gap_count, 0);
    }

    #[test]
    fn reports_wrap_edge_gap_and_provisional_without_three_syncs() {
        let events = [synced_event(16, "A"), synced_event(17, "B")];
        let registry = events
            .iter()
            .map(|event| (event.uid, event))
            .collect::<BTreeMap<_, _>>();
        let mut stream = Vec::new();
        push_synced(&mut stream, 16, 0xffffff, 1);
        push_synced(&mut stream, 17, 1, 2); // missing serial 0 across wrap
        let streams = [(5_u16, stream)].into_iter().collect::<BTreeMap<_, _>>();

        let (_, provisional) = dispatch_normal_events(&streams, &registry, 1).unwrap();
        assert_eq!(provisional.gap_count, 1);
        assert_eq!(provisional.gaps[0].after_serial, 0);
        assert_eq!(provisional.gaps[0].missing_count, 1);
        assert_eq!(provisional.gaps[0].kind, SerialGapKind::Provisional);

        let (_, genuine) = dispatch_normal_events(&streams, &registry, 3).unwrap();
        assert_eq!(genuine.gaps[0].kind, SerialGapKind::Genuine);
    }

    #[test]
    fn reports_genuine_serial_gap_when_serial_missing() {
        let events = [synced_event(16, "A"), synced_event(17, "B")];
        let registry = events
            .iter()
            .map(|event| (event.uid, event))
            .collect::<BTreeMap<_, _>>();
        let mut stream = Vec::new();
        push_synced(&mut stream, 16, 10, 1);
        push_synced(&mut stream, 17, 12, 2);
        let streams = [(5_u16, stream)].into_iter().collect::<BTreeMap<_, _>>();
        let (dispatched, summary) = dispatch_normal_events(&streams, &registry, 3).unwrap();
        assert_eq!(dispatched.len(), 2);
        assert_eq!(summary.gap_count, 1);
        assert_eq!(summary.gaps[0].after_serial, 11);
        assert_eq!(summary.gaps[0].missing_count, 1);
        assert_eq!(summary.missing_serial_count, 1);
        assert_eq!(summary.gaps[0].kind, SerialGapKind::Genuine);
    }

    #[test]
    fn peels_leading_no_sync_events_before_serial_merge() {
        let events = [no_sync_event(16, "Meta"), synced_event(17, "Synced")];
        let registry = events
            .iter()
            .map(|event| (event.uid, event))
            .collect::<BTreeMap<_, _>>();
        let mut stream_a = Vec::new();
        push_unsynced(&mut stream_a, 16, 0xaa);
        push_synced(&mut stream_a, 17, 1, 0x01);
        let mut stream_b = Vec::new();
        push_synced(&mut stream_b, 17, 0, 0x00);
        let streams = [(8_u16, stream_a), (9_u16, stream_b)]
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        let (dispatched, summary) = dispatch_normal_events(&streams, &registry, 1).unwrap();
        assert_eq!(dispatched[0].serial, None);
        assert_eq!(dispatched[0].data[0], 0xaa);
        assert_eq!(
            dispatched[1..]
                .iter()
                .map(|event| event.serial.map(TraceSerial::raw))
                .collect::<Vec<_>>(),
            vec![Some(0), Some(1)]
        );
        assert_eq!(summary.unsynced_event_count, 1);
        assert_eq!(summary.synced_event_count, 2);
    }

    #[test]
    fn circular_run_start_prefers_post_wrap_cluster() {
        assert_eq!(circular_run_start(&[0, 0xfffffe]), Some(0xfffffe));
        assert_eq!(circular_run_start(&[1, 2, 3]), Some(1));
        assert_eq!(circular_run_start(&[0xffffff, 0, 1]), Some(0xffffff));
        assert_eq!(
            circular_run_start(&[0, 0x200000, 0x400000, 0x600000, 0x800000, 0x900000]),
            Some(0)
        );
    }

    #[test]
    fn bitmap_origin_matches_sorted_reference() {
        let values = [0, 1, 17, 63, 64, 0x200000, 0x900000, 0xfffffe, 0xffffff];
        let mut words = vec![0_u64; usize::try_from(SERIAL_RANGE / 64).unwrap()];
        for value in values {
            let index = usize::try_from(value).unwrap();
            words[index / 64] |= 1_u64 << (index % 64);
        }
        assert_eq!(
            circular_run_start_bitmap(&words),
            circular_run_start(&values)
        );
    }

    #[test]
    fn streaming_gap_summary_counts_beyond_retained_samples() {
        let events = [synced_event(16, "A")];
        let registry = events
            .iter()
            .map(|event| (event.uid, event))
            .collect::<BTreeMap<_, _>>();
        let mut stream = Vec::new();
        for serial in (0..140).step_by(2) {
            push_synced(&mut stream, 16, serial, 1);
        }
        let streams = [(5_u16, stream)].into_iter().collect::<BTreeMap<_, _>>();

        let (_, summary) = dispatch_normal_events(&streams, &registry, 3).unwrap();
        assert_eq!(summary.gap_count, 69);
        assert_eq!(summary.missing_serial_count, 69);
        assert_eq!(summary.gaps.len(), 64);
    }

    #[test]
    fn gap_detection_does_not_close_a_non_wrapping_capture_into_a_ring() {
        let gaps = serial_gaps_from_observed(&[0, 1, 2, 0x900000], SerialGapKind::Genuine);
        assert!(gaps.is_empty());

        let wrap_gap = serial_gaps_from_observed(&[0xffffff, 1], SerialGapKind::Genuine);
        assert_eq!(wrap_gap.len(), 1);
        assert_eq!(wrap_gap[0].after_serial, 0);
        assert_eq!(wrap_gap[0].missing_count, 1);
    }

    #[test]
    fn rejects_an_ambiguous_complete_serial_epoch() {
        ensure_single_serial_epoch(usize::try_from(SERIAL_RANGE - 1).unwrap()).unwrap();
        let error = ensure_single_serial_epoch(usize::try_from(SERIAL_RANGE).unwrap())
            .expect_err("a second serial epoch is ambiguous in an offline merge");
        assert_eq!(error.kind(), TraceErrorKind::UnsupportedFormat);
        assert_eq!(error.path(), "Events.Serial");
    }
}
