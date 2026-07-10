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

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct ThreadEvent {
    uid: u16,
    data: Vec<u8>,
    scope_cycle: Option<u64>,
    /// `None` means well-known or `no_sync` (UE Ignored).
    serial: Option<TraceSerial>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct HeapEntry {
    distance: u32,
    thread_index: usize,
    event_index: usize,
    serial: TraceSerial,
}

impl Ord for HeapEntry {
    fn cmp(&self, other: &Self) -> Ordering {
        // Min-heap by modular distance from origin (UE FSerialDistancePredicate).
        other
            .distance
            .cmp(&self.distance)
            .then_with(|| other.thread_index.cmp(&self.thread_index))
            .then_with(|| other.event_index.cmp(&self.event_index))
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

/// Parse one thread's normal stream into owned events with retained serials.
fn parse_thread_normal_events(
    stream: &[u8],
    registry: &BTreeMap<u16, &EventTypeInfo>,
) -> Result<Vec<ThreadEvent>, TraceError> {
    let mut reader = Reader::new(stream);
    let mut events = Vec::new();
    let mut scope_cycles = Vec::<u64>::new();
    while reader.remaining() > 0 {
        let parsed = parse_protocol5_normal_event(&mut reader, registry)?;
        if parsed.uid == 3 {
            continue;
        }
        let mut data = stream[parsed.data_start..parsed.data_end].to_vec();
        match parsed.uid {
            6 | 8 => {
                if let Some(cycle) = decode_known_scope_cycle(parsed.uid, &data) {
                    scope_cycles.push(cycle);
                }
            }
            7 | 9 => {
                scope_cycles.pop();
            }
            _ => {}
        }
        let scope_cycle = (parsed.uid >= 16)
            .then(|| scope_cycles.last().copied())
            .flatten();
        if parsed.has_aux {
            let mut aux_chain = 0_u32;
            loop {
                aux_chain = aux_chain.saturating_add(1);
                if aux_chain > 64_000 {
                    return Err(TraceError::new(
                        TraceErrorKind::ResourceLimit,
                        reader.tell(),
                        "Events.Aux",
                        "aux event chain exceeded 64000 events",
                    ));
                }
                let aux = parse_protocol5_normal_event(&mut reader, registry)?;
                match aux.uid {
                    1 => {
                        let mut aux_bytes = stream[aux.offset..aux.total_end].to_vec();
                        aux_bytes[0] = 1;
                        data.extend_from_slice(&aux_bytes);
                    }
                    3 => {
                        data.push(3);
                        break;
                    }
                    _ => {}
                }
            }
        }
        events.push(ThreadEvent {
            uid: parsed.uid,
            data,
            scope_cycle,
            serial: parsed.serial,
        });
    }
    Ok(events)
}

/// Dispatch normal events from all threads in global serial order.
pub fn dispatch_normal_events(
    streams: &BTreeMap<u16, Vec<u8>>,
    registry: &BTreeMap<u16, &EventTypeInfo>,
    sync_count: u64,
) -> Result<(Vec<DispatchedNormalEvent>, SerialDispatchSummary), TraceError> {
    let mut thread_ids = Vec::new();
    let mut thread_events = Vec::new();
    for (thread_id, stream) in streams {
        if *thread_id <= 1 {
            continue;
        }
        thread_ids.push(*thread_id);
        thread_events.push(parse_thread_normal_events(stream, registry)?);
    }

    let synced_events = thread_events
        .iter()
        .flat_map(|events| events.iter())
        .filter(|event| event.serial.is_some())
        .count();
    ensure_single_serial_epoch(synced_events)?;

    let mut cursors = vec![0_usize; thread_events.len()];
    let mut out = Vec::new();
    let mut summary = SerialDispatchSummary {
        serial_ordered: true,
        sync_count,
        ..SerialDispatchSummary::default()
    };

    // Peel leading unsynchronized events from every thread.
    for (thread_index, events) in thread_events.iter_mut().enumerate() {
        while cursors[thread_index] < events.len() && events[cursors[thread_index]].serial.is_none()
        {
            let event = take_event(events, cursors[thread_index]);
            push_taken(&mut out, &mut summary, thread_ids[thread_index], event);
            cursors[thread_index] += 1;
        }
    }

    let mut next_serial = wrap_aware_origin(&thread_events, &cursors);
    let mut observed_serials = Vec::<u32>::new();

    let mut heap = BinaryHeap::new();
    if let Some(origin) = next_serial {
        for (thread_index, events) in thread_events.iter().enumerate() {
            if let Some(entry) = heap_entry_for(thread_index, cursors[thread_index], events, origin)
            {
                heap.push(entry);
            }
        }
    }

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
        let mut event_index = top.event_index;

        while event_index < thread_events[thread_index].len() {
            let serial = thread_events[thread_index][event_index].serial;
            match serial {
                Some(serial) if serial == expected => {
                    observed_serials.push(serial.raw());
                    let event = take_event(&mut thread_events[thread_index], event_index);
                    push_taken(&mut out, &mut summary, thread_ids[thread_index], event);
                    event_index += 1;
                    expected = expected.wrapping_add(1);
                    next_serial = Some(expected);
                }
                None => {
                    let event = take_event(&mut thread_events[thread_index], event_index);
                    push_taken(&mut out, &mut summary, thread_ids[thread_index], event);
                    event_index += 1;
                }
                Some(_) => break,
            }
        }

        cursors[thread_index] = event_index;
        if let Some(origin) = next_serial {
            if let Some(entry) = heap_entry_for(
                thread_index,
                cursors[thread_index],
                &thread_events[thread_index],
                origin,
            ) {
                heap.push(entry);
            }
        }
    }

    let gap_kind = if sync_count >= 3 {
        SerialGapKind::Genuine
    } else {
        SerialGapKind::Provisional
    };
    summary.gaps = serial_gaps_from_observed(&observed_serials, gap_kind);
    summary.gap_count = u64::try_from(summary.gaps.len()).unwrap_or(u64::MAX);
    summary.missing_serial_count = summary
        .gaps
        .iter()
        .map(|gap| u64::from(gap.missing_count))
        .sum();
    summary.dispatched_event_count = u64::try_from(out.len()).unwrap_or(u64::MAX);
    if summary.gaps.len() > 64 {
        summary.gaps.truncate(64);
    }
    Ok((out, summary))
}

fn take_event(events: &mut [ThreadEvent], index: usize) -> ThreadEvent {
    std::mem::take(&mut events[index])
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

fn push_taken(
    out: &mut Vec<DispatchedNormalEvent>,
    summary: &mut SerialDispatchSummary,
    thread_id: u16,
    event: ThreadEvent,
) {
    if event.serial.is_some() {
        summary.synced_event_count += 1;
    } else {
        summary.unsynced_event_count += 1;
    }
    out.push(DispatchedNormalEvent {
        thread_id,
        uid: event.uid,
        data: event.data,
        scope_cycle: event.scope_cycle,
        serial: event.serial,
    });
}

/// Pick the serial origin among all buffered serials using the largest circular gap.
///
/// The element immediately after the largest forward gap is the start of the
/// captured serial run. Considering every buffered serial, rather than only
/// thread heads, avoids mistaking a late-created thread for a wrap when a long
/// non-wrapping capture spans more than half the 24-bit range.
fn wrap_aware_origin(thread_events: &[Vec<ThreadEvent>], cursors: &[usize]) -> Option<TraceSerial> {
    let mut serials = Vec::new();
    for (thread_index, events) in thread_events.iter().enumerate() {
        serials.extend(
            events[cursors[thread_index]..]
                .iter()
                .filter_map(|event| event.serial.map(TraceSerial::raw)),
        );
    }
    circular_run_start(&serials).map(TraceSerial)
}

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

fn push_gap_if_small(gaps: &mut Vec<SerialGap>, left: u32, right: u32, kind: SerialGapKind) {
    let forward = right.wrapping_sub(left) & SERIAL_MASK;
    if forward == 0 || forward >= SERIAL_HALF {
        return;
    }
    let missing = forward.saturating_sub(1);
    if missing == 0 {
        return;
    }
    gaps.push(SerialGap {
        after_serial: left.wrapping_add(1) & SERIAL_MASK,
        missing_count: missing,
        kind,
    });
}

fn heap_entry_for(
    thread_index: usize,
    event_index: usize,
    events: &[ThreadEvent],
    origin: TraceSerial,
) -> Option<HeapEntry> {
    let mut index = event_index;
    while index < events.len() && events[index].serial.is_none() {
        index += 1;
    }
    let event = events.get(index)?;
    let serial = event.serial?;
    Some(HeapEntry {
        distance: serial.distance_from(origin),
        thread_index,
        event_index: index,
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
    registry: &BTreeMap<u16, &EventTypeInfo>,
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
        let Some(event) = registry.get(&uid).copied() else {
            return Err(TraceError::new(
                TraceErrorKind::MalformedData,
                u64::try_from(offset).unwrap(),
                "Events.Uid",
                format!("unknown event uid {uid} in normal stream"),
            ));
        };
        let serial = if event.flags.no_sync {
            None
        } else {
            Some(read_serial_24(reader)?)
        };
        (event_data_size(event), event.flags.maybe_has_aux, serial)
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
