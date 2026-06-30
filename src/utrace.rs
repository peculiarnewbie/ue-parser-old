//! Read-only UTrace (`.utrace`) container inspection.

use std::collections::BTreeMap;
use std::fmt;

use serde::Serialize;

use crate::{ArchiveError, ArchiveErrorKind, Reader};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceMagic {
    Trc2,
    Trce,
    LegacyProtocol0Transport1,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TraceHeader {
    pub magic: TraceMagic,
    pub metadata_size: Option<u16>,
    pub transport: u8,
    pub protocol: u8,
    pub packet_stream_offset: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TraceInspect {
    pub header: TraceHeader,
    pub packets: PacketSummary,
    pub events: Vec<EventTypeInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prologue: Option<TracePrologue>,
    pub thread_info: Vec<TraceThreadInfo>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TraceDashboard {
    pub header: TraceHeader,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prologue: Option<TracePrologue>,
    pub thread_info: Vec<TraceThreadInfo>,
    pub cpu: CpuDashboard,
    pub frames: Vec<FrameMarker>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct CpuDashboard {
    pub specs: Vec<CpuScopeSpec>,
    pub batches: CpuBatchSummary,
    pub scopes: Vec<CpuScopeSummary>,
    pub threads: Vec<CpuThreadSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CpuScopeSpec {
    pub id: u32,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct CpuBatchSummary {
    pub count: u64,
    pub decoded_records: u64,
    pub intervals: u64,
    pub unresolved_specs: u64,
    pub metadata_scopes: u64,
    pub coroutine_records: u64,
    pub unmatched_ends: u64,
    pub unterminated_scopes: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CpuScopeSummary {
    pub spec_id: u32,
    pub name: String,
    pub count: u64,
    pub total_cycles: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_seconds: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CpuThreadSummary {
    pub thread_id: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub count: u64,
    pub total_cycles: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_seconds: Option<f64>,
    pub scopes: Vec<CpuScopeSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FrameMarker {
    pub kind: FrameMarkerKind,
    pub cycle: u64,
    pub frame_type: u8,
    pub thread_id: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FrameMarkerKind {
    Begin,
    End,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct PacketSummary {
    pub count: u64,
    pub sync_count: u64,
    pub raw_bytes: u64,
    pub decoded_bytes: u64,
    pub compressed_payload_bytes: u64,
    pub compressed_decoded_bytes: u64,
    pub thread_count: usize,
    pub threads: Vec<ThreadPacketSummary>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct ThreadPacketSummary {
    pub thread_id: u16,
    pub packet_count: u64,
    pub raw_bytes: u64,
    pub decoded_bytes: u64,
    pub compressed_payload_bytes: u64,
    pub compressed_decoded_bytes: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TracePrologue {
    pub start_cycle: u64,
    pub cycle_frequency: u64,
    pub endian: u16,
    pub pointer_size: u8,
    pub start_date_time: f64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TraceThreadInfo {
    pub thread_id: u32,
    pub system_id: u32,
    pub sort_hint: i32,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct EventTypeInfo {
    pub uid: u16,
    pub logger: String,
    pub event: String,
    pub flags: EventFlags,
    pub fields: Vec<FieldInfo>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct EventFlags {
    pub important: bool,
    pub maybe_has_aux: bool,
    pub no_sync: bool,
    pub definition: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct FieldInfo {
    pub name: String,
    pub offset: u16,
    pub size: u16,
    pub family: FieldFamily,
    #[serde(rename = "type")]
    pub type_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ref_uid: Option<u16>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldFamily {
    Regular,
    Reference,
    DefinitionId,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct DecodedStreams {
    summary: PacketSummary,
    streams: BTreeMap<u16, Vec<u8>>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TraceError {
    kind: TraceErrorKind,
    offset: u64,
    path: String,
    detail: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TraceErrorKind {
    MalformedData,
    UnsupportedFormat,
}

impl TraceError {
    fn new(
        kind: TraceErrorKind,
        offset: u64,
        path: impl Into<String>,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            offset,
            path: path.into(),
            detail: detail.into(),
        }
    }

    #[must_use]
    pub const fn kind(&self) -> TraceErrorKind {
        self.kind
    }

    #[must_use]
    pub const fn offset(&self) -> u64 {
        self.offset
    }

    #[must_use]
    pub fn path(&self) -> &str {
        &self.path
    }

    #[must_use]
    pub fn detail(&self) -> &str {
        &self.detail
    }
}

impl fmt::Display for TraceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{:?} at byte {} while reading {}: {}",
            self.kind, self.offset, self.path, self.detail
        )
    }
}

impl std::error::Error for TraceError {}

impl From<ArchiveError> for TraceError {
    fn from(error: ArchiveError) -> Self {
        let kind = match error.kind() {
            ArchiveErrorKind::OutOfBounds
            | ArchiveErrorKind::InvalidSeek
            | ArchiveErrorKind::InvalidCount
            | ArchiveErrorKind::IntegerOverflow => TraceErrorKind::MalformedData,
            ArchiveErrorKind::AllocationLimit
            | ArchiveErrorKind::MissingNullTerminator
            | ArchiveErrorKind::InvalidString
            | ArchiveErrorKind::InvalidNameReference => TraceErrorKind::MalformedData,
        };
        Self::new(kind, error.offset(), error.path(), error.detail())
    }
}

pub fn inspect(source: &[u8]) -> Result<TraceInspect, TraceError> {
    let mut reader = Reader::new(source);
    let header = read_header(&mut reader)?;
    let decoded = read_packets(&mut reader)?;
    let events = read_event_registry(&header, &decoded.streams)?;
    let decoded_importants = read_known_important_events(&header, &decoded.streams, &events)?;
    Ok(TraceInspect {
        header,
        packets: decoded.summary,
        events,
        prologue: decoded_importants.prologue,
        thread_info: decoded_importants.thread_info,
    })
}

pub fn dashboard(source: &[u8]) -> Result<TraceDashboard, TraceError> {
    let mut reader = Reader::new(source);
    let header = read_header(&mut reader)?;
    let decoded = read_packets(&mut reader)?;
    let events = read_event_registry(&header, &decoded.streams)?;
    let decoded_importants = read_known_important_events(&header, &decoded.streams, &events)?;
    let dashboard = read_dashboard_events(&header, &decoded.streams, &events, &decoded_importants)?;
    Ok(TraceDashboard {
        header,
        prologue: decoded_importants.prologue,
        thread_info: decoded_importants.thread_info,
        cpu: dashboard.cpu,
        frames: dashboard.frames,
    })
}

fn read_header(reader: &mut Reader<'_>) -> Result<TraceHeader, TraceError> {
    let magic_offset = reader.tell();
    let magic_bytes = reader.read_bytes(4, "Header.Magic")?;
    let (magic, metadata_size) = match magic_bytes {
        // Runtime/TraceLog/Private/Trace/Writer.cpp writes this byte order.
        b"2CRT" => {
            let size = reader.read_u16("Header.MetadataSize")?;
            reader.skip(u64::from(size), "Header.Metadata")?;
            (TraceMagic::Trc2, Some(size))
        }
        b"ECRT" => (TraceMagic::Trce, None),
        // Swapped-endian spellings are rejected by TraceAnalysis.
        b"TRC2" | b"TRCE" => {
            return Err(TraceError::new(
                TraceErrorKind::UnsupportedFormat,
                magic_offset,
                "Header.Magic",
                "big-endian trace streams are not supported",
            ));
        }
        [1, 0, 0, 0] => {
            reader.seek(magic_offset, "Header.Legacy")?;
            (TraceMagic::LegacyProtocol0Transport1, None)
        }
        _ => {
            return Err(TraceError::new(
                TraceErrorKind::MalformedData,
                magic_offset,
                "Header.Magic",
                format!("unrecognized trace magic bytes {magic_bytes:02x?}"),
            ));
        }
    };

    let transport = reader.read_u8("Header.TransportVersion")?;
    let protocol = reader.read_u8("Header.ProtocolVersion")?;
    if transport != 4 {
        return Err(TraceError::new(
            TraceErrorKind::UnsupportedFormat,
            reader.tell() - 2,
            "Header.TransportVersion",
            format!("transport {transport} is not TidPacketSync (4)"),
        ));
    }
    if protocol > 7 {
        return Err(TraceError::new(
            TraceErrorKind::UnsupportedFormat,
            reader.tell() - 1,
            "Header.ProtocolVersion",
            format!("protocol {protocol} is newer than supported protocol 7"),
        ));
    }

    Ok(TraceHeader {
        magic,
        metadata_size,
        transport,
        protocol,
        packet_stream_offset: reader.tell(),
    })
}

fn read_packets(reader: &mut Reader<'_>) -> Result<DecodedStreams, TraceError> {
    let mut summary = PacketSummary::default();
    let mut threads = BTreeMap::<u16, ThreadPacketSummary>::new();
    let mut streams = BTreeMap::<u16, Vec<u8>>::new();

    while reader.remaining() > 0 {
        let packet_offset = reader.tell();
        let packet_size = reader.read_u16("Packet.PacketSize")?;
        let thread_word = reader.read_u16("Packet.ThreadId")?;
        if packet_size < 4 {
            return Err(TraceError::new(
                TraceErrorKind::MalformedData,
                packet_offset,
                "Packet.PacketSize",
                format!("packet size {packet_size} is smaller than FTidPacketBase"),
            ));
        }

        let thread_id = thread_word & 0x3fff;
        let encoded = (thread_word & 0x8000) != 0;
        summary.count += 1;
        summary.raw_bytes += u64::from(packet_size);

        if thread_id == 0x3fff {
            if packet_size != 4 {
                return Err(TraceError::new(
                    TraceErrorKind::MalformedData,
                    packet_offset,
                    "Packet.Sync",
                    format!("sync packet size must be 4, got {packet_size}"),
                ));
            }
            summary.sync_count += 1;
            continue;
        }

        let thread = threads.entry(thread_id).or_insert(ThreadPacketSummary {
            thread_id,
            ..ThreadPacketSummary::default()
        });
        let stream = streams.entry(thread_id).or_default();
        thread.packet_count += 1;
        thread.raw_bytes += u64::from(packet_size);

        if encoded {
            if packet_size < 6 {
                return Err(TraceError::new(
                    TraceErrorKind::MalformedData,
                    packet_offset,
                    "Packet.DecodedSize",
                    format!("encoded packet size {packet_size} is smaller than FTidPacketEncoded"),
                ));
            }
            let decoded_size = reader.read_u16("Packet.DecodedSize")?;
            let compressed_size = usize::from(packet_size - 6);
            let compressed = reader.read_bytes(compressed_size, "Packet.CompressedData")?;
            if decoded_size > 0 {
                let mut decoded = vec![0_u8; usize::from(decoded_size)];
                let actual = lz4_flex::block::decompress_into(compressed, &mut decoded).map_err(
                    |error| {
                        TraceError::new(
                            TraceErrorKind::MalformedData,
                            packet_offset,
                            "Packet.CompressedData",
                            format!("LZ4 block decompression failed: {error}"),
                        )
                    },
                )?;
                if actual != usize::from(decoded_size) {
                    return Err(TraceError::new(
                        TraceErrorKind::MalformedData,
                        packet_offset,
                        "Packet.DecodedSize",
                        format!("expected {decoded_size} decoded bytes, got {actual}"),
                    ));
                }
                stream.extend_from_slice(&decoded);
            }
            summary.compressed_payload_bytes += u64::try_from(compressed_size).unwrap();
            summary.compressed_decoded_bytes += u64::from(decoded_size);
            summary.decoded_bytes += u64::from(decoded_size);
            thread.compressed_payload_bytes += u64::try_from(compressed_size).unwrap();
            thread.compressed_decoded_bytes += u64::from(decoded_size);
            thread.decoded_bytes += u64::from(decoded_size);
        } else {
            let payload_size = usize::from(packet_size - 4);
            let payload = reader.read_bytes(payload_size, "Packet.Data")?;
            stream.extend_from_slice(payload);
            summary.decoded_bytes += u64::try_from(payload_size).unwrap();
            thread.decoded_bytes += u64::try_from(payload_size).unwrap();
        }
    }

    summary.threads = threads.into_values().collect();
    summary.thread_count = summary.threads.len();
    Ok(DecodedStreams { summary, streams })
}

fn read_event_registry(
    header: &TraceHeader,
    streams: &BTreeMap<u16, Vec<u8>>,
) -> Result<Vec<EventTypeInfo>, TraceError> {
    if header.protocol < 5 {
        return Ok(Vec::new());
    }

    let mut events = Vec::new();
    for thread_id in [0_u16, 1_u16] {
        let Some(stream) = streams.get(&thread_id) else {
            continue;
        };
        let mut reader = Reader::new(stream);
        while reader.remaining() > 0 {
            let event_offset = reader.tell();
            if reader.remaining() < 4 {
                return Err(TraceError::new(
                    TraceErrorKind::MalformedData,
                    event_offset,
                    "Events.ImportantHeader",
                    "truncated important event header",
                ));
            }
            let uid = reader.read_u16("Events.Uid")?;
            let size = reader.read_u16("Events.Size")?;
            let data = reader.read_bytes(usize::from(size), "Events.Data")?;
            if uid == 0 {
                events.push(decode_new_event(data, header.protocol, event_offset + 4)?);
            }
        }
    }
    events.sort_by_key(|event| event.uid);
    Ok(events)
}

fn decode_new_event(
    data: &[u8],
    protocol: u8,
    base_offset: u64,
) -> Result<EventTypeInfo, TraceError> {
    let mut reader = Reader::new(data);
    let event_uid = reader.read_u16("NewEvent.EventUid")?;
    let field_count = reader.read_u8("NewEvent.FieldCount")?;
    let raw_flags = reader.read_u8("NewEvent.Flags")?;
    let logger_name_size = reader.read_u8("NewEvent.LoggerNameSize")?;
    let event_name_size = reader.read_u8("NewEvent.EventNameSize")?;

    let mut raw_fields = Vec::with_capacity(usize::from(field_count));
    for index in 0..field_count {
        raw_fields.push(if protocol >= 6 {
            read_protocol6_field(&mut reader, index)?
        } else {
            read_protocol4_field(&mut reader, index)?
        });
    }

    let logger = read_ansi_name(&mut reader, logger_name_size, "NewEvent.LoggerName")?;
    let event = read_ansi_name(&mut reader, event_name_size, "NewEvent.EventName")?;
    let mut fields = Vec::with_capacity(raw_fields.len());
    for (index, field) in raw_fields.into_iter().enumerate() {
        let name = if field.family == FieldFamily::DefinitionId {
            "DefinitionId".to_owned()
        } else {
            read_ansi_name(
                &mut reader,
                field.name_size,
                &format!("NewEvent.Fields[{index}].Name"),
            )?
        };
        fields.push(FieldInfo {
            name,
            offset: field.offset,
            size: field.size,
            family: field.family,
            type_name: type_info_name(field.type_info),
            ref_uid: field.ref_uid,
        });
    }

    if reader.remaining() > 1 {
        return Err(TraceError::new(
            TraceErrorKind::MalformedData,
            base_offset + reader.tell(),
            "NewEvent",
            format!(
                "{} trailing bytes after NewEvent declaration",
                reader.remaining()
            ),
        ));
    }

    Ok(EventTypeInfo {
        uid: event_uid,
        logger,
        event,
        flags: EventFlags {
            important: (raw_flags & 0x01) != 0,
            maybe_has_aux: (raw_flags & 0x02) != 0,
            no_sync: (raw_flags & 0x04) != 0,
            definition: (raw_flags & 0x08) != 0,
        },
        fields,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RawFieldInfo {
    family: FieldFamily,
    offset: u16,
    size: u16,
    type_info: u8,
    name_size: u8,
    ref_uid: Option<u16>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct DecodedImportantEvents {
    prologue: Option<TracePrologue>,
    thread_info: Vec<TraceThreadInfo>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct DecodedDashboardEvents {
    cpu: CpuDashboard,
    frames: Vec<FrameMarker>,
}

fn read_known_important_events(
    header: &TraceHeader,
    streams: &BTreeMap<u16, Vec<u8>>,
    events: &[EventTypeInfo],
) -> Result<DecodedImportantEvents, TraceError> {
    if header.protocol < 5 {
        return Ok(DecodedImportantEvents::default());
    }

    let registry = events
        .iter()
        .map(|event| (event.uid, event))
        .collect::<BTreeMap<_, _>>();
    let mut decoded = DecodedImportantEvents::default();

    for thread_id in [0_u16, 1_u16] {
        let Some(stream) = streams.get(&thread_id) else {
            continue;
        };
        let mut reader = Reader::new(stream);
        while reader.remaining() > 0 {
            let event_offset = reader.tell();
            if reader.remaining() < 4 {
                return Err(TraceError::new(
                    TraceErrorKind::MalformedData,
                    event_offset,
                    "Events.ImportantHeader",
                    "truncated important event header",
                ));
            }
            let uid = reader.read_u16("Events.Uid")?;
            let size = reader.read_u16("Events.Size")?;
            let data = reader.read_bytes(usize::from(size), "Events.Data")?;
            let Some(event) = registry.get(&uid).copied() else {
                continue;
            };
            match (event.logger.as_str(), event.event.as_str()) {
                ("$Trace", "NewTrace") => {
                    decoded.prologue = Some(decode_new_trace(event, data, event_offset + 4)?);
                }
                ("$Trace", "ThreadInfo") => {
                    decoded
                        .thread_info
                        .push(decode_thread_info(event, data, event_offset + 4)?);
                }
                _ => {}
            }
        }
    }

    decoded.thread_info.sort_by_key(|thread| thread.thread_id);
    Ok(decoded)
}

fn read_dashboard_events(
    header: &TraceHeader,
    streams: &BTreeMap<u16, Vec<u8>>,
    events: &[EventTypeInfo],
    importants: &DecodedImportantEvents,
) -> Result<DecodedDashboardEvents, TraceError> {
    if header.protocol < 5 {
        return Ok(DecodedDashboardEvents::default());
    }

    let registry = events
        .iter()
        .map(|event| (event.uid, event))
        .collect::<BTreeMap<_, _>>();
    let mut decoded = DecodedDashboardEvents::default();
    let mut spec_by_id = BTreeMap::<u32, CpuScopeSpec>::new();
    let mut scope_totals = BTreeMap::<u32, (u64, u64)>::new();
    let mut thread_scope_totals = BTreeMap::<u16, BTreeMap<u32, (u64, u64)>>::new();
    let cycle_frequency = importants
        .prologue
        .as_ref()
        .map(|prologue| prologue.cycle_frequency)
        .filter(|frequency| *frequency > 0);

    for (thread_id, stream) in streams {
        if *thread_id > 1 {
            for raw_event in read_protocol5_normal_events(stream, &registry)? {
                let Some(event) = registry.get(&raw_event.uid).copied() else {
                    continue;
                };
                if (event.logger.as_str(), event.event.as_str()) == ("CpuProfiler", "EventBatchV3")
                {
                    let Some(data) = read_aux_bytes(event, &raw_event.data, "Data", 0)? else {
                        continue;
                    };
                    decode_cpu_batch(
                        &data,
                        &spec_by_id,
                        &mut decoded.cpu.batches,
                        &mut scope_totals,
                        thread_scope_totals.entry(*thread_id).or_default(),
                    )?;
                } else if (event.logger.as_str(), event.event.as_str()) == ("Misc", "BeginFrame") {
                    decoded.frames.push(decode_frame_marker(
                        event,
                        &raw_event.data,
                        0,
                        *thread_id,
                        FrameMarkerKind::Begin,
                    )?);
                } else if (event.logger.as_str(), event.event.as_str()) == ("Misc", "EndFrame") {
                    decoded.frames.push(decode_frame_marker(
                        event,
                        &raw_event.data,
                        0,
                        *thread_id,
                        FrameMarkerKind::End,
                    )?);
                }
            }
            continue;
        }

        for raw_event in read_protocol5_important_events(stream)? {
            let Some(event) = registry.get(&raw_event.uid).copied() else {
                continue;
            };
            match (event.logger.as_str(), event.event.as_str()) {
                ("CpuProfiler", "EventSpec") => {
                    let spec = decode_cpu_event_spec(event, raw_event.data, raw_event.offset + 4)?;
                    spec_by_id.insert(spec.id, spec);
                }
                ("Misc", "BeginFrame") => {
                    decoded.frames.push(decode_frame_marker(
                        event,
                        raw_event.data,
                        raw_event.offset + 4,
                        *thread_id,
                        FrameMarkerKind::Begin,
                    )?);
                }
                ("Misc", "EndFrame") => {
                    decoded.frames.push(decode_frame_marker(
                        event,
                        raw_event.data,
                        raw_event.offset + 4,
                        *thread_id,
                        FrameMarkerKind::End,
                    )?);
                }
                _ => {}
            }
        }
    }

    decoded.cpu.specs = spec_by_id.values().cloned().collect();
    decoded.cpu.scopes = scope_summaries(scope_totals, &spec_by_id, cycle_frequency);
    decoded.cpu.threads = thread_scope_totals
        .into_iter()
        .map(|(thread_id, totals)| {
            let scopes = scope_summaries(totals, &spec_by_id, cycle_frequency);
            let count = scopes.iter().map(|scope| scope.count).sum();
            let total_cycles = scopes.iter().map(|scope| scope.total_cycles).sum();
            let info = importants
                .thread_info
                .iter()
                .find(|thread| thread.thread_id == u32::from(thread_id));
            CpuThreadSummary {
                thread_id,
                system_id: info.map(|thread| thread.system_id),
                name: info.map(|thread| thread.name.clone()),
                count,
                total_cycles,
                total_seconds: cycle_frequency
                    .map(|frequency| total_cycles as f64 / frequency as f64),
                scopes,
            }
        })
        .collect();
    decoded
        .cpu
        .threads
        .sort_by(|left, right| right.total_cycles.cmp(&left.total_cycles));
    decoded.frames.sort_by_key(|frame| frame.cycle);
    Ok(decoded)
}

fn scope_summaries(
    totals: BTreeMap<u32, (u64, u64)>,
    spec_by_id: &BTreeMap<u32, CpuScopeSpec>,
    cycle_frequency: Option<u64>,
) -> Vec<CpuScopeSummary> {
    let mut scopes = totals
        .into_iter()
        .map(|(spec_id, (count, total_cycles))| {
            let name = spec_by_id
                .get(&spec_id)
                .map(|spec| spec.name.clone())
                .unwrap_or_else(|| format!("#{spec_id}"));
            CpuScopeSummary {
                spec_id,
                name,
                count,
                total_cycles,
                total_seconds: cycle_frequency
                    .map(|frequency| total_cycles as f64 / frequency as f64),
            }
        })
        .collect::<Vec<_>>();
    scopes.sort_by(|left, right| right.total_cycles.cmp(&left.total_cycles));
    scopes
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RawEvent<'a> {
    uid: u16,
    offset: u64,
    data: &'a [u8],
}

fn read_protocol5_important_events(stream: &[u8]) -> Result<Vec<RawEvent<'_>>, TraceError> {
    let mut reader = Reader::new(stream);
    let mut events = Vec::new();
    while reader.remaining() > 0 {
        let event_offset = reader.tell();
        if reader.remaining() < 4 {
            return Err(TraceError::new(
                TraceErrorKind::MalformedData,
                event_offset,
                "Events.Header",
                "truncated event header",
            ));
        }
        let uid = reader.read_u16("Events.Uid")?;
        let size = reader.read_u16("Events.Size")?;
        let data = reader.read_bytes(usize::from(size), "Events.Data")?;
        events.push(RawEvent {
            uid,
            offset: event_offset,
            data,
        });
    }
    Ok(events)
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct OwnedRawEvent {
    uid: u16,
    data: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ParsedNormalEvent {
    uid: u16,
    offset: usize,
    total_end: usize,
    data_start: usize,
    data_end: usize,
    has_aux: bool,
}

fn read_protocol5_normal_events(
    stream: &[u8],
    registry: &BTreeMap<u16, &EventTypeInfo>,
) -> Result<Vec<OwnedRawEvent>, TraceError> {
    let mut reader = Reader::new(stream);
    let mut events = Vec::new();
    while reader.remaining() > 0 {
        let parsed = parse_protocol5_normal_event(&mut reader, registry)?;
        if parsed.uid == 3 {
            continue;
        }
        let mut data = stream[parsed.data_start..parsed.data_end].to_vec();
        if parsed.has_aux {
            loop {
                let aux = parse_protocol5_normal_event(&mut reader, registry)?;
                match aux.uid {
                    1 => {
                        let mut aux_bytes = stream[aux.offset..aux.total_end].to_vec();
                        // Normal streams store known UIDs shifted; our aux parser consumes the
                        // unshifted important-event spelling.
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
        events.push(OwnedRawEvent {
            uid: parsed.uid,
            data,
        });
    }
    Ok(events)
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

    let (event_size, has_aux) = if uid < USER_UID {
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
        (size, false)
    } else {
        let Some(event) = registry.get(&uid).copied() else {
            return Err(TraceError::new(
                TraceErrorKind::MalformedData,
                u64::try_from(offset).unwrap(),
                "Events.Uid",
                format!("unknown event uid {uid} in normal stream"),
            ));
        };
        if !event.flags.no_sync {
            reader.skip(3, "Events.Serial")?;
        }
        (event_data_size(event), event.flags.maybe_has_aux)
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
    })
}

fn decode_cpu_event_spec(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
) -> Result<CpuScopeSpec, TraceError> {
    let aux = parse_protocol5_aux(data, event_data_size(event), base_offset)?;
    let name = read_aux_string(event, &aux, "Name")?;
    let file = optional_aux_string(event, &aux, "File")?.filter(|file| !file.is_empty());
    let line = read_optional_u32_field(event, data, "Line", base_offset)?;
    Ok(CpuScopeSpec {
        id: read_u32_field(event, data, "Id", base_offset)?,
        name,
        file,
        line: line.filter(|line| *line != 0),
    })
}

fn decode_frame_marker(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
    thread_id: u16,
    kind: FrameMarkerKind,
) -> Result<FrameMarker, TraceError> {
    Ok(FrameMarker {
        kind,
        cycle: read_u64_field(event, data, "Cycle", base_offset)?,
        frame_type: read_u8_field(event, data, "FrameType", base_offset)?,
        thread_id,
    })
}

fn decode_cpu_batch(
    data: &[u8],
    specs: &BTreeMap<u32, CpuScopeSpec>,
    batches: &mut CpuBatchSummary,
    scope_totals: &mut BTreeMap<u32, (u64, u64)>,
    thread_scope_totals: &mut BTreeMap<u32, (u64, u64)>,
) -> Result<(), TraceError> {
    batches.count += 1;
    let mut reader = VarintReader::new(data);
    let mut last_cycle = 0_u64;
    let mut stack = Vec::<(u32, u64)>::new();

    while !reader.is_empty() {
        let first = reader.read_u64()?;
        batches.decoded_records += 1;
        let mut cycle = first >> 2;
        if cycle < last_cycle {
            cycle = cycle.saturating_add(last_cycle);
        }
        match first & 0b11 {
            0b00 => {
                if let Some((spec_id, start_cycle)) = stack.pop() {
                    let duration = cycle.saturating_sub(start_cycle);
                    let entry = scope_totals.entry(spec_id).or_insert((0, 0));
                    entry.0 += 1;
                    entry.1 = entry.1.saturating_add(duration);
                    let thread_entry = thread_scope_totals.entry(spec_id).or_insert((0, 0));
                    thread_entry.0 += 1;
                    thread_entry.1 = thread_entry.1.saturating_add(duration);
                    batches.intervals += 1;
                } else {
                    batches.unmatched_ends += 1;
                }
            }
            0b01 => {
                let payload = reader.read_u64()?;
                if (payload & 1) != 0 {
                    batches.metadata_scopes += 1;
                    continue;
                }
                let spec_id = u32::try_from(payload >> 1).map_err(|_| {
                    TraceError::new(
                        TraceErrorKind::MalformedData,
                        0,
                        "CpuProfiler.EventBatchV3.Data",
                        "scope spec id does not fit in u32",
                    )
                })?;
                if !specs.contains_key(&spec_id) {
                    batches.unresolved_specs += 1;
                }
                stack.push((spec_id, cycle));
            }
            0b10 => {
                let _depth = reader.read_u64()?;
                batches.coroutine_records += 1;
            }
            0b11 => {
                let _coroutine_id = reader.read_u64()?;
                let _depth = reader.read_u64()?;
                batches.coroutine_records += 1;
            }
            _ => unreachable!("opcode mask is two bits"),
        }
        last_cycle = cycle;
    }

    batches.unterminated_scopes += u64::try_from(stack.len()).unwrap();
    Ok(())
}

fn decode_new_trace(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
) -> Result<TracePrologue, TraceError> {
    Ok(TracePrologue {
        start_cycle: read_u64_field(event, data, "StartCycle", base_offset)?,
        cycle_frequency: read_u64_field(event, data, "CycleFrequency", base_offset)?,
        endian: read_u16_field(event, data, "Endian", base_offset)?,
        pointer_size: read_u8_field(event, data, "PointerSize", base_offset)?,
        start_date_time: read_f64_field(event, data, "StartDateTime", base_offset)?,
    })
}

fn decode_thread_info(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
) -> Result<TraceThreadInfo, TraceError> {
    let aux = parse_protocol5_aux(data, event_data_size(event), base_offset)?;
    Ok(TraceThreadInfo {
        thread_id: read_u32_field(event, data, "ThreadId", base_offset)?,
        system_id: read_u32_field(event, data, "SystemId", base_offset)?,
        sort_hint: read_i32_field(event, data, "SortHint", base_offset)?,
        name: read_aux_string(event, &aux, "Name")?,
    })
}

fn read_u8_field(
    event: &EventTypeInfo,
    data: &[u8],
    name: &str,
    base_offset: u64,
) -> Result<u8, TraceError> {
    let bytes = fixed_field_bytes(event, data, name, 1, base_offset)?;
    Ok(bytes[0])
}

fn read_u16_field(
    event: &EventTypeInfo,
    data: &[u8],
    name: &str,
    base_offset: u64,
) -> Result<u16, TraceError> {
    Ok(u16::from_le_bytes(
        fixed_field_bytes(event, data, name, 2, base_offset)?
            .try_into()
            .expect("fixed field length was checked"),
    ))
}

fn read_u32_field(
    event: &EventTypeInfo,
    data: &[u8],
    name: &str,
    base_offset: u64,
) -> Result<u32, TraceError> {
    Ok(u32::from_le_bytes(
        fixed_field_bytes(event, data, name, 4, base_offset)?
            .try_into()
            .expect("fixed field length was checked"),
    ))
}

fn read_optional_u32_field(
    event: &EventTypeInfo,
    data: &[u8],
    name: &str,
    base_offset: u64,
) -> Result<Option<u32>, TraceError> {
    if event.fields.iter().any(|field| field.name == name) {
        Ok(Some(read_u32_field(event, data, name, base_offset)?))
    } else {
        Ok(None)
    }
}

fn read_i32_field(
    event: &EventTypeInfo,
    data: &[u8],
    name: &str,
    base_offset: u64,
) -> Result<i32, TraceError> {
    Ok(i32::from_le_bytes(
        fixed_field_bytes(event, data, name, 4, base_offset)?
            .try_into()
            .expect("fixed field length was checked"),
    ))
}

fn read_u64_field(
    event: &EventTypeInfo,
    data: &[u8],
    name: &str,
    base_offset: u64,
) -> Result<u64, TraceError> {
    Ok(u64::from_le_bytes(
        fixed_field_bytes(event, data, name, 8, base_offset)?
            .try_into()
            .expect("fixed field length was checked"),
    ))
}

fn read_f64_field(
    event: &EventTypeInfo,
    data: &[u8],
    name: &str,
    base_offset: u64,
) -> Result<f64, TraceError> {
    Ok(f64::from_le_bytes(
        fixed_field_bytes(event, data, name, 8, base_offset)?
            .try_into()
            .expect("fixed field length was checked"),
    ))
}

fn fixed_field_bytes<'a>(
    event: &EventTypeInfo,
    data: &'a [u8],
    name: &str,
    expected_size: usize,
    base_offset: u64,
) -> Result<&'a [u8], TraceError> {
    let field = find_field(event, name)?;
    if usize::from(field.size) != expected_size {
        return Err(TraceError::new(
            TraceErrorKind::MalformedData,
            base_offset + u64::from(field.offset),
            format!("{}.{}", event.event, name),
            format!("expected {expected_size} byte field, got {}", field.size),
        ));
    }
    let start = usize::from(field.offset);
    let end = start.checked_add(expected_size).ok_or_else(|| {
        TraceError::new(
            TraceErrorKind::MalformedData,
            base_offset + u64::from(field.offset),
            format!("{}.{}", event.event, name),
            "field range overflows",
        )
    })?;
    if end > data.len() {
        return Err(TraceError::new(
            TraceErrorKind::MalformedData,
            base_offset + u64::from(field.offset),
            format!("{}.{}", event.event, name),
            format!("field extends past event payload of {} bytes", data.len()),
        ));
    }
    Ok(&data[start..end])
}

fn find_field<'a>(event: &'a EventTypeInfo, name: &str) -> Result<&'a FieldInfo, TraceError> {
    event
        .fields
        .iter()
        .find(|field| field.name == name)
        .ok_or_else(|| {
            TraceError::new(
                TraceErrorKind::MalformedData,
                0,
                format!("{}.{}", event.event, name),
                "declared event is missing required field",
            )
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

fn parse_protocol5_aux(
    data: &[u8],
    event_data_size: usize,
    base_offset: u64,
) -> Result<BTreeMap<u8, Vec<u8>>, TraceError> {
    let mut aux = BTreeMap::<u8, Vec<u8>>::new();
    if event_data_size >= data.len() {
        return Ok(aux);
    }

    let mut cursor = event_data_size;
    while cursor < data.len() {
        let uid = data[cursor];
        if uid == 3 {
            return Ok(aux);
        }
        if uid != 1 {
            return Err(TraceError::new(
                TraceErrorKind::MalformedData,
                base_offset + u64::try_from(cursor).unwrap(),
                "Aux.Uid",
                format!("expected AuxData/AuxDataTerminal, got uid {uid}"),
            ));
        }
        if data.len() - cursor < 4 {
            return Err(TraceError::new(
                TraceErrorKind::MalformedData,
                base_offset + u64::try_from(cursor).unwrap(),
                "Aux.Header",
                "truncated aux header",
            ));
        }
        let pack = u32::from_le_bytes(
            data[cursor..cursor + 4]
                .try_into()
                .expect("aux header length was checked"),
        );
        let field_index = ((pack >> 8) & 0x1f) as u8;
        let size = usize::try_from(pack >> 13).expect("u32 fits in usize");
        let payload_start = cursor + 4;
        let payload_end = payload_start.checked_add(size).ok_or_else(|| {
            TraceError::new(
                TraceErrorKind::MalformedData,
                base_offset + u64::try_from(cursor).unwrap(),
                "Aux.Size",
                "aux payload range overflows",
            )
        })?;
        if payload_end > data.len() {
            return Err(TraceError::new(
                TraceErrorKind::MalformedData,
                base_offset + u64::try_from(payload_start).unwrap(),
                "Aux.Data",
                format!(
                    "aux payload extends past event payload of {} bytes",
                    data.len()
                ),
            ));
        }
        aux.entry(field_index)
            .or_default()
            .extend_from_slice(&data[payload_start..payload_end]);
        cursor = payload_end;
    }
    Ok(aux)
}

fn read_aux_string(
    event: &EventTypeInfo,
    aux: &BTreeMap<u8, Vec<u8>>,
    name: &str,
) -> Result<String, TraceError> {
    let index = event
        .fields
        .iter()
        .position(|field| field.name == name)
        .ok_or_else(|| {
            TraceError::new(
                TraceErrorKind::MalformedData,
                0,
                format!("{}.{}", event.event, name),
                "declared event is missing required aux string field",
            )
        })?;
    let bytes = aux.get(&(index as u8)).ok_or_else(|| {
        TraceError::new(
            TraceErrorKind::MalformedData,
            0,
            format!("{}.{}", event.event, name),
            "event payload is missing required aux string",
        )
    })?;
    Ok(decode_ansi_bytes(bytes))
}

fn optional_aux_string(
    event: &EventTypeInfo,
    aux: &BTreeMap<u8, Vec<u8>>,
    name: &str,
) -> Result<Option<String>, TraceError> {
    let Some(index) = event.fields.iter().position(|field| field.name == name) else {
        return Ok(None);
    };
    Ok(aux
        .get(&(index as u8))
        .map(|bytes| decode_ansi_bytes(bytes)))
}

fn read_aux_bytes(
    event: &EventTypeInfo,
    data: &[u8],
    name: &str,
    base_offset: u64,
) -> Result<Option<Vec<u8>>, TraceError> {
    let Some(index) = event.fields.iter().position(|field| field.name == name) else {
        return Ok(None);
    };
    let aux = parse_protocol5_aux(data, event_data_size(event), base_offset)?;
    Ok(aux.get(&(index as u8)).cloned())
}

fn decode_ansi_bytes(bytes: &[u8]) -> String {
    let bytes = bytes.strip_suffix(&[0]).unwrap_or(bytes);
    bytes.iter().map(|byte| char::from(*byte)).collect()
}

struct VarintReader<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> VarintReader<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, cursor: 0 }
    }

    fn is_empty(&self) -> bool {
        self.cursor >= self.bytes.len()
    }

    fn read_u64(&mut self) -> Result<u64, TraceError> {
        let start = self.cursor;
        let mut value = 0_u64;
        for shift in (0..=63).step_by(7) {
            if self.cursor >= self.bytes.len() {
                return Err(TraceError::new(
                    TraceErrorKind::MalformedData,
                    u64::try_from(start).unwrap(),
                    "CpuProfiler.EventBatchV3.Data",
                    "truncated 7-bit varint",
                ));
            }
            let byte = self.bytes[self.cursor];
            self.cursor += 1;
            value |= u64::from(byte & 0x7f) << shift;
            if (byte & 0x80) == 0 {
                return Ok(value);
            }
        }
        Err(TraceError::new(
            TraceErrorKind::MalformedData,
            u64::try_from(start).unwrap(),
            "CpuProfiler.EventBatchV3.Data",
            "7-bit varint is too large",
        ))
    }
}

fn read_protocol4_field(reader: &mut Reader<'_>, index: u8) -> Result<RawFieldInfo, TraceError> {
    let offset = reader.read_u16(&format!("NewEvent.Fields[{index}].Offset"))?;
    let size = reader.read_u16(&format!("NewEvent.Fields[{index}].Size"))?;
    let type_info = reader.read_u8(&format!("NewEvent.Fields[{index}].TypeInfo"))?;
    let name_size = reader.read_u8(&format!("NewEvent.Fields[{index}].NameSize"))?;
    Ok(RawFieldInfo {
        family: FieldFamily::Regular,
        offset,
        size,
        type_info,
        name_size,
        ref_uid: None,
    })
}

fn read_protocol6_field(reader: &mut Reader<'_>, index: u8) -> Result<RawFieldInfo, TraceError> {
    let field_type = reader.read_u8(&format!("NewEvent.Fields[{index}].FieldType"))?;
    reader.skip(1, &format!("NewEvent.Fields[{index}].Padding"))?;
    match field_type {
        0 => read_protocol4_field(reader, index),
        1 => {
            let offset = reader.read_u16(&format!("NewEvent.Fields[{index}].Offset"))?;
            let ref_uid = reader.read_u16(&format!("NewEvent.Fields[{index}].RefUid"))?;
            let type_info = reader.read_u8(&format!("NewEvent.Fields[{index}].TypeInfo"))?;
            let name_size = reader.read_u8(&format!("NewEvent.Fields[{index}].NameSize"))?;
            Ok(RawFieldInfo {
                family: FieldFamily::Reference,
                offset,
                size: 1_u16 << (type_info & 0x03),
                type_info,
                name_size,
                ref_uid: Some(ref_uid),
            })
        }
        2 => {
            let offset = reader.read_u16(&format!("NewEvent.Fields[{index}].Offset"))?;
            reader.skip(2, &format!("NewEvent.Fields[{index}].Unused1"))?;
            reader.skip(1, &format!("NewEvent.Fields[{index}].Unused2"))?;
            let type_info = reader.read_u8(&format!("NewEvent.Fields[{index}].TypeInfo"))?;
            Ok(RawFieldInfo {
                family: FieldFamily::DefinitionId,
                offset,
                size: 1_u16 << (type_info & 0x03),
                type_info,
                name_size: 0,
                ref_uid: None,
            })
        }
        _ => Err(TraceError::new(
            TraceErrorKind::MalformedData,
            reader.tell() - 1,
            format!("NewEvent.Fields[{index}].FieldType"),
            format!("unknown field family {field_type}"),
        )),
    }
}

fn read_ansi_name(reader: &mut Reader<'_>, size: u8, path: &str) -> Result<String, TraceError> {
    let bytes = reader.read_bytes(usize::from(size), path)?;
    Ok(bytes.iter().map(|byte| char::from(*byte)).collect())
}

fn type_info_name(type_info: u8) -> String {
    let size = 1_u8 << (type_info & 0x03);
    let category = type_info & 0xc0;
    let special = type_info & 0x18;
    match (category, special, size) {
        (0x80, 0x08, 1) => "ansi_string".to_owned(),
        (0x80, 0x08, 2) => "wide_string".to_owned(),
        (0x80, _, _) => "array".to_owned(),
        (0x40, _, 4) => "float32".to_owned(),
        (0x40, _, 8) => "float64".to_owned(),
        (0x40, _, _) => format!("float{bits}", bits = u16::from(size) * 8),
        (0x00, 0x10, _) => format!("int{bits}", bits = u16::from(size) * 8),
        (0x00, _, _) => format!("uint{bits}", bits = u16::from(size) * 8),
        _ => format!("unknown_0x{type_info:02x}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const UINT8: u8 = 0x00;
    const UINT16: u8 = 0x01;
    const UINT32: u8 = 0x02;
    const UINT64: u8 = 0x03;
    const INT32: u8 = 0x12;
    const FLOAT64: u8 = 0x43;
    const ANSI_STRING: u8 = 0x88;

    #[test]
    fn parses_trc2_header_and_sync_packet() {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"2CRT");
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.push(4);
        bytes.push(7);
        bytes.extend_from_slice(&4_u16.to_le_bytes());
        bytes.extend_from_slice(&0x3fff_u16.to_le_bytes());

        let trace = inspect(&bytes).unwrap();
        assert_eq!(trace.header.magic, TraceMagic::Trc2);
        assert_eq!(trace.header.metadata_size, Some(0));
        assert_eq!(trace.header.transport, 4);
        assert_eq!(trace.header.protocol, 7);
        assert_eq!(trace.packets.count, 1);
        assert_eq!(trace.packets.sync_count, 1);
    }

    #[test]
    fn rejects_swapped_magic() {
        let error = inspect(b"TRC2").unwrap_err();
        assert_eq!(error.kind(), TraceErrorKind::UnsupportedFormat);
    }

    #[test]
    fn decodes_protocol7_new_event_declaration() {
        let declaration = new_event(
            42,
            0x07,
            "Cpu",
            "EventSpec",
            &[regular_field(0, 4, UINT32, "Id")],
        );
        let bytes = trace_with_events(&[important_event(0, &declaration)]);

        let trace = inspect(&bytes).unwrap();
        assert_eq!(trace.events.len(), 1);
        let event = &trace.events[0];
        assert_eq!(event.uid, 42);
        assert_eq!(event.logger, "Cpu");
        assert_eq!(event.event, "EventSpec");
        assert!(event.flags.important);
        assert!(event.flags.maybe_has_aux);
        assert!(event.flags.no_sync);
        assert_eq!(event.fields[0].name, "Id");
        assert_eq!(event.fields[0].type_name, "uint32");
    }

    #[test]
    fn decodes_new_trace_and_thread_info() {
        let new_trace_uid = 10;
        let thread_info_uid = 11;
        let new_trace_decl = new_event(
            new_trace_uid,
            0x05,
            "$Trace",
            "NewTrace",
            &[
                regular_field(0, 8, UINT64, "StartCycle"),
                regular_field(8, 8, UINT64, "CycleFrequency"),
                regular_field(16, 2, UINT16, "Endian"),
                regular_field(18, 1, UINT8, "PointerSize"),
                regular_field(19, 8, FLOAT64, "StartDateTime"),
            ],
        );
        let thread_info_decl = new_event(
            thread_info_uid,
            0x07,
            "$Trace",
            "ThreadInfo",
            &[
                regular_field(0, 4, UINT32, "ThreadId"),
                regular_field(4, 4, UINT32, "SystemId"),
                regular_field(8, 4, INT32, "SortHint"),
                regular_field(12, 0, ANSI_STRING, "Name"),
            ],
        );

        let mut new_trace_data = Vec::new();
        new_trace_data.extend_from_slice(&100_u64.to_le_bytes());
        new_trace_data.extend_from_slice(&1_000_000_u64.to_le_bytes());
        new_trace_data.extend_from_slice(&0x524d_u16.to_le_bytes());
        new_trace_data.push(8);
        new_trace_data.extend_from_slice(&1234.5_f64.to_le_bytes());

        let mut thread_data = Vec::new();
        thread_data.extend_from_slice(&2_u32.to_le_bytes());
        thread_data.extend_from_slice(&99_u32.to_le_bytes());
        thread_data.extend_from_slice(&(-7_i32).to_le_bytes());
        thread_data.extend_from_slice(&aux(3, b"GameThread"));
        thread_data.push(3);

        let bytes = trace_with_events(&[
            important_event(0, &new_trace_decl),
            important_event(0, &thread_info_decl),
            important_event(new_trace_uid, &new_trace_data),
            important_event(thread_info_uid, &thread_data),
        ]);

        let trace = inspect(&bytes).unwrap();
        assert_eq!(
            trace.prologue,
            Some(TracePrologue {
                start_cycle: 100,
                cycle_frequency: 1_000_000,
                endian: 0x524d,
                pointer_size: 8,
                start_date_time: 1234.5,
            })
        );
        assert_eq!(
            trace.thread_info,
            vec![TraceThreadInfo {
                thread_id: 2,
                system_id: 99,
                sort_hint: -7,
                name: "GameThread".to_owned(),
            }]
        );
    }

    #[derive(Clone)]
    struct TestField {
        offset: u16,
        size: u16,
        type_info: u8,
        name: &'static str,
    }

    fn regular_field(offset: u16, size: u16, type_info: u8, name: &'static str) -> TestField {
        TestField {
            offset,
            size,
            type_info,
            name,
        }
    }

    fn new_event(uid: u16, flags: u8, logger: &str, event: &str, fields: &[TestField]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&uid.to_le_bytes());
        bytes.push(u8::try_from(fields.len()).unwrap());
        bytes.push(flags);
        bytes.push(u8::try_from(logger.len()).unwrap());
        bytes.push(u8::try_from(event.len()).unwrap());
        for field in fields {
            bytes.push(0);
            bytes.push(0);
            bytes.extend_from_slice(&field.offset.to_le_bytes());
            bytes.extend_from_slice(&field.size.to_le_bytes());
            bytes.push(field.type_info);
            bytes.push(u8::try_from(field.name.len()).unwrap());
        }
        bytes.extend_from_slice(logger.as_bytes());
        bytes.extend_from_slice(event.as_bytes());
        for field in fields {
            bytes.extend_from_slice(field.name.as_bytes());
        }
        bytes
    }

    fn important_event(uid: u16, data: &[u8]) -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&uid.to_le_bytes());
        bytes.extend_from_slice(&u16::try_from(data.len()).unwrap().to_le_bytes());
        bytes.extend_from_slice(data);
        bytes
    }

    fn aux(field_index: u8, payload: &[u8]) -> Vec<u8> {
        let pack =
            1_u32 | (u32::from(field_index) << 8) | (u32::try_from(payload.len()).unwrap() << 13);
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&pack.to_le_bytes());
        bytes.extend_from_slice(payload);
        bytes
    }

    fn trace_with_events(events: &[Vec<u8>]) -> Vec<u8> {
        let packet_payload = events.concat();
        let packet_size = u16::try_from(packet_payload.len() + 4).unwrap();
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"2CRT");
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.push(4);
        bytes.push(7);
        bytes.extend_from_slice(&packet_size.to_le_bytes());
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&packet_payload);
        bytes
    }
}
