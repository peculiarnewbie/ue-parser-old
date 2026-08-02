//! Incremental UTrace packet transport and dashboard session lifecycle.
//!
//! Provider/event projection still occurs at `finish`; packet framing and LZ4
//! decoding are incremental and never retain the complete capture byte stream.

use std::collections::{BTreeMap, VecDeque};

use crate::Reader;
use crate::utrace::{
    CpuTimelineMemoryIndex, DashboardOptions, DecodedStreams, EventTypeInfo, FrameMarkerKind,
    PacketSummary, SourceFingerprint, ThreadPacketSummary, TimelineIndexBuild,
    TimelineIndexRequest, TraceDashboard, TraceError, TraceErrorKind, TraceHeader, TraceInventory,
    TracePrologue, TraceThreadInfo, dashboard_from_decoded,
    dashboard_from_decoded_with_memory_timeline_index, dashboard_from_decoded_with_timeline_index,
    decode_frame_marker, decode_new_event, decode_new_trace, decode_thread_info,
    decompress_lz4_into_stream, inventory_from_decoded, parse_protocol5_normal_event,
    read_u32_field, read_u64_field,
};
use crate::utrace_progress::{
    DashboardBootstrap, DashboardPatch, DecodePhase, DecodeProgress, FrameTimingDashboard,
    ProgressiveFrameTiming,
};

pub(crate) const MAX_PUSH_CHUNK_BYTES: usize = 1024 * 1024;
const MAX_DECODED_STREAM_BYTES: usize = 1024 * 1024 * 1024;
const MAX_BOOTSTRAP_THREADS: usize = 4096;
const MAX_PROGRESSIVE_GPU_QUEUES: usize = 64;
const MAX_PROGRESSIVE_OPEN_GPU_WORK_PER_QUEUE: usize = 256;
const MAX_PENDING_PROGRESSIVE_GPU_WORK: usize = 1024;

#[derive(Clone, Copy, Debug)]
struct ProgressiveGpuOpenWork {
    gpu_timestamp_top: u64,
    cpu_timestamp: u64,
}

#[derive(Clone, Copy, Debug)]
struct ProgressiveGpuCompletedWork {
    cpu_timestamp: u64,
    duration_cycles: u64,
}

#[derive(Clone, Debug)]
struct ProgressiveFrameSlot {
    begin_cycle: u64,
    end_cycle: Option<u64>,
    /// Index into `progressive_frames` once this slot has been ended at least once.
    published_index: Option<usize>,
}

pub struct ProgressiveDashboardSession {
    options: DashboardOptions,
    header: Option<TraceHeader>,
    pending: Vec<u8>,
    pending_offset: u64,
    summary: PacketSummary,
    threads: BTreeMap<u16, ThreadPacketSummary>,
    streams: BTreeMap<u16, Vec<u8>>,
    decoded_stream_bytes: usize,
    finished: bool,
    important_cursors: BTreeMap<u16, usize>,
    normal_cursors: BTreeMap<u16, usize>,
    bootstrap_registry: BTreeMap<u16, EventTypeInfo>,
    bootstrap_prologue: Option<TracePrologue>,
    bootstrap_threads: Vec<TraceThreadInfo>,
    bootstrap_threads_truncated: bool,
    /// Insights `FFrameProvider` parity: each `BeginFrame` pushes a slot; each
    /// `EndFrame` updates the latest slot for that `FrameType` (and may extend an
    /// already-closed frame). See TraceServices `Frames.cpp`.
    frames_by_type: BTreeMap<u8, Vec<ProgressiveFrameSlot>>,
    progressive_frames: Vec<ProgressiveFrameTiming>,
    progressive_frame_count: u64,
    progressive_frame_revision: u64,
    progressive_gpu_open_work: BTreeMap<u32, VecDeque<ProgressiveGpuOpenWork>>,
    pending_progressive_gpu_work: VecDeque<ProgressiveGpuCompletedWork>,
    source_fingerprint: SourceFingerprint,
}

impl ProgressiveDashboardSession {
    #[must_use]
    pub fn new(options: DashboardOptions) -> Self {
        Self {
            options,
            header: None,
            pending: Vec::new(),
            pending_offset: 0,
            summary: PacketSummary::default(),
            threads: BTreeMap::new(),
            streams: BTreeMap::new(),
            decoded_stream_bytes: 0,
            finished: false,
            important_cursors: BTreeMap::new(),
            normal_cursors: BTreeMap::new(),
            bootstrap_registry: BTreeMap::new(),
            bootstrap_prologue: None,
            bootstrap_threads: Vec::new(),
            bootstrap_threads_truncated: false,
            frames_by_type: BTreeMap::new(),
            progressive_frames: Vec::new(),
            progressive_frame_count: 0,
            progressive_frame_revision: 0,
            progressive_gpu_open_work: BTreeMap::new(),
            pending_progressive_gpu_work: VecDeque::new(),
            source_fingerprint: SourceFingerprint::new(),
        }
    }

    pub fn push_chunk(&mut self, chunk: &[u8]) -> Result<(), TraceError> {
        if self.finished {
            return Err(TraceError::new(
                TraceErrorKind::MalformedData,
                self.pending_offset,
                "Session",
                "cannot push input after finish",
            ));
        }
        if chunk.len() > MAX_PUSH_CHUNK_BYTES {
            return Err(TraceError::new(
                TraceErrorKind::ResourceLimit,
                self.pending_offset,
                "Session.Chunk",
                format!(
                    "chunk is {} bytes; maximum is {MAX_PUSH_CHUNK_BYTES}",
                    chunk.len()
                ),
            ));
        }
        self.source_fingerprint.update(chunk);
        self.pending.extend_from_slice(chunk);
        self.decode_available()
    }

    pub fn finish(self) -> Result<TraceDashboard, TraceError> {
        let options = self.options;
        let frame_timing = self.frame_timing_dashboard();
        let (header, decoded) = self.finish_decoding()?;
        let mut dashboard = dashboard_from_decoded(header, decoded, options)?;
        dashboard.frame_timing = Some(frame_timing);
        Ok(dashboard)
    }

    pub fn finish_with_inventory(self) -> Result<(TraceDashboard, TraceInventory), TraceError> {
        let options = self.options;
        let frame_timing = self.frame_timing_dashboard();
        let (header, decoded) = self.finish_decoding()?;
        let inventory = inventory_from_decoded(header.clone(), &decoded)?;
        let mut dashboard = dashboard_from_decoded(header, decoded, options)?;
        dashboard.frame_timing = Some(frame_timing);
        Ok((dashboard, inventory))
    }

    pub fn finish_with_inventory_and_timeline_index(
        self,
        request: Option<TimelineIndexRequest>,
    ) -> Result<(TraceDashboard, TraceInventory, Option<TimelineIndexBuild>), TraceError> {
        let options = self.options;
        let source_identity = self.source_fingerprint.finish();
        let frame_timing = self.frame_timing_dashboard();
        let (header, decoded) = self.finish_decoding()?;
        let inventory = inventory_from_decoded(header.clone(), &decoded)?;
        let (mut dashboard, timeline_index) = dashboard_from_decoded_with_timeline_index(
            header,
            decoded,
            options,
            request.map(|request| (request, source_identity)),
        )?;
        dashboard.frame_timing = Some(frame_timing);
        Ok((dashboard, inventory, timeline_index))
    }

    pub fn finish_with_inventory_and_memory_timeline_index(
        self,
    ) -> Result<(TraceDashboard, TraceInventory, CpuTimelineMemoryIndex), TraceError> {
        let options = self.options;
        let source_identity = self.source_fingerprint.finish();
        let frame_timing = self.frame_timing_dashboard();
        let (header, decoded) = self.finish_decoding()?;
        let inventory = inventory_from_decoded(header.clone(), &decoded)?;
        let (mut dashboard, timeline_index) = dashboard_from_decoded_with_memory_timeline_index(
            header,
            decoded,
            options,
            source_identity,
        )?;
        dashboard.frame_timing = Some(frame_timing);
        Ok((dashboard, inventory, timeline_index))
    }

    #[must_use]
    pub fn bootstrap(
        &self,
        total_bytes: Option<u64>,
    ) -> Option<(DecodeProgress, DashboardBootstrap)> {
        let header = self.header.clone()?;
        Some((
            self.progress(total_bytes, DecodePhase::Reading),
            DashboardBootstrap {
                header,
                prologue: self.bootstrap_prologue.clone(),
                thread_info: self.bootstrap_threads.clone(),
                declared_event_types: u64::try_from(self.bootstrap_registry.len()).unwrap(),
                packets: self.packet_snapshot(),
                thread_info_truncated: self.bootstrap_threads_truncated,
            },
        ))
    }

    #[must_use]
    pub fn transport_patch(&self, total_bytes: Option<u64>) -> (DecodeProgress, DashboardPatch) {
        self.transport_patch_for_phase(total_bytes, DecodePhase::Reading)
    }

    #[must_use]
    pub fn analyzing_patch(&self, total_bytes: Option<u64>) -> (DecodeProgress, DashboardPatch) {
        self.transport_patch_for_phase(total_bytes, DecodePhase::Analyzing)
    }

    fn transport_patch_for_phase(
        &self,
        total_bytes: Option<u64>,
        phase: DecodePhase,
    ) -> (DecodeProgress, DashboardPatch) {
        (
            self.progress(total_bytes, phase),
            DashboardPatch::Transport {
                packets: self.packet_snapshot(),
            },
        )
    }

    #[must_use]
    pub fn complete_progress(&self, total_bytes: Option<u64>) -> DecodeProgress {
        self.progress(total_bytes, DecodePhase::Complete)
    }

    #[must_use]
    pub fn frame_patch(&self, total_bytes: Option<u64>) -> (DecodeProgress, DashboardPatch) {
        let total_frame_count = self.progressive_frame_count;
        let truncated = total_frame_count > u64::try_from(self.progressive_frames.len()).unwrap();
        let frames = self.progressive_frames.clone();
        (
            self.progress(total_bytes, DecodePhase::Reading),
            DashboardPatch::Frames {
                total_frame_count,
                truncated,
                frames,
            },
        )
    }

    #[must_use]
    pub fn frame_revision(&self) -> u64 {
        self.progressive_frame_revision
    }

    fn frame_timing_dashboard(&self) -> FrameTimingDashboard {
        FrameTimingDashboard {
            total_frame_count: self.progressive_frame_count,
            frames: self.progressive_frames.clone(),
        }
    }

    fn finish_decoding(mut self) -> Result<(TraceHeader, DecodedStreams), TraceError> {
        self.finished = true;
        self.decode_available()?;
        let header = self.header.ok_or_else(|| {
            TraceError::new(
                TraceErrorKind::MalformedData,
                self.pending_offset,
                "Header.Magic",
                "input ended before the trace header was complete",
            )
        })?;
        if !self.pending.is_empty() {
            return Err(TraceError::new(
                TraceErrorKind::MalformedData,
                self.pending_offset,
                if self.pending.len() < 2 {
                    "Packet.PacketSize"
                } else if self.pending.len() < 4 {
                    "Packet.ThreadId"
                } else {
                    "Packet.Data"
                },
                "input ended in a partial packet",
            ));
        }
        self.summary.threads = self.threads.into_values().collect();
        self.summary.thread_count = self.summary.threads.len();
        Ok((
            header,
            DecodedStreams {
                summary: self.summary,
                streams: self.streams,
            },
        ))
    }

    fn decode_available(&mut self) -> Result<(), TraceError> {
        if self.header.is_none() {
            let Some(header_len) = complete_header_len(&self.pending)? else {
                return Ok(());
            };
            let mut reader = Reader::new(&self.pending[..header_len]);
            let header = super::utrace::read_header(&mut reader)?;
            self.header = Some(header);
            self.consume(header_len)?;
        }

        loop {
            if self.pending.len() < 4 {
                return Ok(());
            }
            let packet_size = usize::from(u16::from_le_bytes([self.pending[0], self.pending[1]]));
            if packet_size < 4 {
                return Err(TraceError::new(
                    TraceErrorKind::MalformedData,
                    self.pending_offset,
                    "Packet.PacketSize",
                    format!("packet size {packet_size} is smaller than FTidPacketBase"),
                ));
            }
            if self.pending.len() < packet_size {
                return Ok(());
            }
            self.decode_packet(packet_size)?;
            self.consume(packet_size)?;
        }
    }

    fn decode_packet(&mut self, packet_size: usize) -> Result<(), TraceError> {
        let packet_offset = self.pending_offset;
        let thread_word = u16::from_le_bytes([self.pending[2], self.pending[3]]);
        let thread_id = thread_word & 0x3fff;
        let encoded = (thread_word & 0x8000) != 0;
        self.summary.count += 1;
        self.summary.raw_bytes += u64::try_from(packet_size).expect("u16 packet size fits u64");

        if thread_id == 0x3fff {
            if packet_size != 4 {
                return Err(TraceError::new(
                    TraceErrorKind::MalformedData,
                    packet_offset,
                    "Packet.Sync",
                    format!("sync packet size must be 4, got {packet_size}"),
                ));
            }
            self.summary.sync_count += 1;
            return Ok(());
        }

        let decoded_len = if encoded {
            if packet_size < 6 {
                return Err(TraceError::new(
                    TraceErrorKind::MalformedData,
                    packet_offset,
                    "Packet.DecodedSize",
                    format!("encoded packet size {packet_size} is smaller than FTidPacketEncoded"),
                ));
            }
            let decoded_size = usize::from(u16::from_le_bytes([self.pending[4], self.pending[5]]));
            let compressed_size = packet_size - 6;
            self.summary.compressed_payload_bytes += u64::try_from(compressed_size).unwrap();
            self.summary.compressed_decoded_bytes += u64::try_from(decoded_size).unwrap();
            let thread = self
                .threads
                .entry(thread_id)
                .or_insert_with(|| ThreadPacketSummary {
                    thread_id,
                    ..Default::default()
                });
            thread.compressed_payload_bytes += u64::try_from(compressed_size).unwrap();
            thread.compressed_decoded_bytes += u64::try_from(decoded_size).unwrap();
            decoded_size
        } else {
            packet_size - 4
        };

        let next_total = self
            .decoded_stream_bytes
            .checked_add(decoded_len)
            .ok_or_else(|| {
                TraceError::new(
                    TraceErrorKind::ResourceLimit,
                    packet_offset,
                    "Packet.Data",
                    "decoded stream byte count overflow",
                )
            })?;
        if next_total > MAX_DECODED_STREAM_BYTES {
            return Err(TraceError::new(
                TraceErrorKind::ResourceLimit,
                packet_offset,
                "Packet.Data",
                format!("decoded streams exceed {MAX_DECODED_STREAM_BYTES} bytes"),
            ));
        }
        self.decoded_stream_bytes = next_total;
        self.summary.decoded_bytes += u64::try_from(decoded_len).unwrap();
        let thread = self
            .threads
            .entry(thread_id)
            .or_insert_with(|| ThreadPacketSummary {
                thread_id,
                ..Default::default()
            });
        thread.packet_count += 1;
        thread.raw_bytes += u64::try_from(packet_size).unwrap();
        thread.decoded_bytes += u64::try_from(decoded_len).unwrap();
        if encoded {
            decompress_lz4_into_stream(
                self.streams.entry(thread_id).or_default(),
                &self.pending[6..packet_size],
                decoded_len,
                packet_offset,
            )?;
        } else {
            self.streams
                .entry(thread_id)
                .or_default()
                .extend_from_slice(&self.pending[4..packet_size]);
        }
        if thread_id <= 1 {
            let registry_len = self.bootstrap_registry.len();
            self.decode_bootstrap_events(thread_id)?;
            if self.bootstrap_registry.len() > registry_len {
                let normal_threads = self
                    .streams
                    .keys()
                    .copied()
                    .filter(|candidate| *candidate > 1)
                    .collect::<Vec<_>>();
                for normal_thread in normal_threads {
                    self.decode_normal_frame_events(normal_thread)?;
                }
            }
        } else {
            self.decode_normal_frame_events(thread_id)?;
        }
        Ok(())
    }

    fn consume(&mut self, count: usize) -> Result<(), TraceError> {
        self.pending.drain(..count);
        self.pending_offset = self
            .pending_offset
            .checked_add(u64::try_from(count).unwrap())
            .ok_or_else(|| {
                TraceError::new(
                    TraceErrorKind::ResourceLimit,
                    self.pending_offset,
                    "Session.Offset",
                    "input offset overflow",
                )
            })?;
        Ok(())
    }

    fn decode_bootstrap_events(&mut self, thread_id: u16) -> Result<(), TraceError> {
        let stream = self.streams.get(&thread_id).expect("stream was inserted");
        let mut cursor = *self.important_cursors.get(&thread_id).unwrap_or(&0);
        let mut frame_markers = Vec::new();
        while stream.len().saturating_sub(cursor) >= 4 {
            let event_offset = cursor;
            let uid = u16::from_le_bytes([stream[event_offset], stream[event_offset + 1]]);
            let size = usize::from(u16::from_le_bytes([
                stream[event_offset + 2],
                stream[event_offset + 3],
            ]));
            let Some(event_end) = event_offset
                .checked_add(4)
                .and_then(|start| start.checked_add(size))
            else {
                return Err(TraceError::new(
                    TraceErrorKind::ResourceLimit,
                    u64::try_from(event_offset).unwrap(),
                    "Events.Size",
                    "important event end overflow",
                ));
            };
            if event_end > stream.len() {
                break;
            }
            let data = &stream[event_offset + 4..event_end];
            if uid == 0 {
                let protocol = self
                    .header
                    .as_ref()
                    .expect("header decoded before packets")
                    .protocol;
                let declaration =
                    decode_new_event(data, protocol, u64::try_from(event_offset + 4).unwrap())?;
                self.bootstrap_registry.insert(declaration.uid, declaration);
            } else if let Some(event) = self.bootstrap_registry.get(&uid) {
                match (event.logger.as_str(), event.event.as_str()) {
                    ("$Trace", "NewTrace") => {
                        self.bootstrap_prologue = Some(decode_new_trace(
                            event,
                            data,
                            u64::try_from(event_offset + 4).unwrap(),
                        )?);
                    }
                    ("$Trace", "ThreadInfo") => {
                        let info = decode_thread_info(
                            event,
                            data,
                            u64::try_from(event_offset + 4).unwrap(),
                        )?;
                        if self.bootstrap_threads.len() < MAX_BOOTSTRAP_THREADS {
                            self.bootstrap_threads.push(info);
                        } else {
                            self.bootstrap_threads_truncated = true;
                        }
                    }
                    ("Misc", "BeginFrame" | "EndFrame") => {
                        let kind = if event.event == "BeginFrame" {
                            FrameMarkerKind::Begin
                        } else {
                            FrameMarkerKind::End
                        };
                        let marker = decode_frame_marker(
                            event,
                            data,
                            u64::try_from(event_offset + 4).unwrap(),
                            thread_id,
                            kind,
                        )?;
                        frame_markers.push(marker);
                    }
                    _ => {}
                }
            }
            cursor = event_end;
        }
        self.important_cursors.insert(thread_id, cursor);
        for marker in frame_markers {
            self.record_frame_marker(marker);
        }
        self.bootstrap_threads
            .sort_by_key(|thread| thread.thread_id);
        Ok(())
    }

    fn decode_normal_frame_events(&mut self, thread_id: u16) -> Result<(), TraceError> {
        loop {
            let cursor = *self.normal_cursors.get(&thread_id).unwrap_or(&0);
            let parsed = {
                let Some(stream) = self.streams.get(&thread_id) else {
                    return Ok(());
                };
                if cursor >= stream.len() {
                    return Ok(());
                }
                // Registry is append-only and never modified by this decode path;
                // pass it directly instead of rebuilding a BTreeMap per event.
                let mut reader = Reader::new(&stream[cursor..]);
                let event =
                    match parse_protocol5_normal_event(&mut reader, &self.bootstrap_registry) {
                        Ok(event) => event,
                        Err(_) => return Ok(()),
                    };
                let mut total_end = event.total_end;
                let mut data = stream[cursor + event.data_start..cursor + event.data_end].to_vec();
                if event.has_aux {
                    loop {
                        let Ok(aux) =
                            parse_protocol5_normal_event(&mut reader, &self.bootstrap_registry)
                        else {
                            return Ok(());
                        };
                        total_end = aux.total_end;
                        match aux.uid {
                            1 => {
                                let mut raw =
                                    stream[cursor + aux.offset..cursor + aux.total_end].to_vec();
                                raw[0] = 1;
                                data.extend_from_slice(&raw);
                            }
                            3 => {
                                data.push(3);
                                break;
                            }
                            _ => {}
                        }
                    }
                }
                (
                    self.bootstrap_registry.get(&event.uid).cloned(),
                    data,
                    total_end,
                )
            };
            let (event, data, consumed) = parsed;
            self.normal_cursors.insert(thread_id, cursor + consumed);
            let Some(event) = event else {
                continue;
            };
            if event.logger == "Misc" && matches!(event.event.as_str(), "BeginFrame" | "EndFrame") {
                let kind = if event.event == "BeginFrame" {
                    FrameMarkerKind::Begin
                } else {
                    FrameMarkerKind::End
                };
                let marker = decode_frame_marker(
                    &event,
                    &data,
                    u64::try_from(cursor).unwrap(),
                    thread_id,
                    kind,
                )?;
                self.record_frame_marker(marker);
            } else if event.logger == "GpuProfiler" {
                self.record_progressive_gpu_work_event(
                    &event,
                    &data,
                    u64::try_from(cursor).unwrap(),
                )?;
            }
        }
    }

    fn record_frame_marker(&mut self, marker: crate::utrace::FrameMarker) {
        match marker.kind {
            FrameMarkerKind::Begin => {
                self.frames_by_type
                    .entry(marker.frame_type)
                    .or_default()
                    .push(ProgressiveFrameSlot {
                        begin_cycle: marker.cycle,
                        end_cycle: None,
                        published_index: None,
                    });
            }
            FrameMarkerKind::End => {
                let frame_type = marker.frame_type;
                let Some((begin_cycle, published_index, duration_cycles)) =
                    self.frames_by_type.get_mut(&frame_type).and_then(|frames| {
                        let slot = frames.last_mut()?;
                        // Insights ignores EndFrame when no BeginFrame has been seen yet
                        // (handled by the Option above). It always writes EndTime onto the
                        // latest frame, even when that frame was already closed.
                        if marker.cycle < slot.begin_cycle {
                            return None;
                        }
                        slot.end_cycle = Some(marker.cycle);
                        Some((
                            slot.begin_cycle,
                            slot.published_index,
                            marker.cycle - slot.begin_cycle,
                        ))
                    })
                else {
                    return;
                };
                let duration_seconds = self
                    .bootstrap_prologue
                    .as_ref()
                    .map(|prologue| prologue.cycle_frequency)
                    .filter(|frequency| *frequency > 0)
                    .map(|frequency| duration_cycles as f64 / frequency as f64);

                if let Some(published_index) = published_index {
                    let frame = &mut self.progressive_frames[published_index];
                    frame.end_cycle = marker.cycle;
                    frame.duration_cycles = duration_cycles;
                    frame.duration_seconds = duration_seconds;
                    self.progressive_frame_revision =
                        self.progressive_frame_revision.saturating_add(1);
                    return;
                }

                let mut frame = ProgressiveFrameTiming {
                    frame_number: self.progressive_frame_count,
                    frame_type,
                    begin_cycle,
                    end_cycle: marker.cycle,
                    duration_cycles,
                    duration_seconds,
                    gpu_submitted_work_count: 0,
                    gpu_submitted_work_cycles: 0,
                };
                self.progressive_frame_count += 1;
                self.apply_pending_progressive_gpu_work(&mut frame);
                let published_index = self.progressive_frames.len();
                self.progressive_frames.push(frame);
                if let Some(slot) = self
                    .frames_by_type
                    .get_mut(&frame_type)
                    .and_then(|frames| frames.last_mut())
                {
                    slot.published_index = Some(published_index);
                }
                self.progressive_frame_revision = self.progressive_frame_revision.saturating_add(1);
            }
        }
    }

    fn record_progressive_gpu_work_event(
        &mut self,
        event: &EventTypeInfo,
        data: &[u8],
        base_offset: u64,
    ) -> Result<(), TraceError> {
        match event.event.as_str() {
            "EventBeginWork" => {
                let queue_id = read_u32_field(event, data, "QueueId", base_offset)?;
                let gpu_timestamp_top =
                    read_u64_field(event, data, "GPUTimestampTOP", base_offset)?;
                let cpu_timestamp = read_u64_field(event, data, "CPUTimestamp", base_offset)?;
                let works = if let Some(works) = self.progressive_gpu_open_work.get_mut(&queue_id) {
                    works
                } else {
                    if self.progressive_gpu_open_work.len() >= MAX_PROGRESSIVE_GPU_QUEUES {
                        return Ok(());
                    }
                    self.progressive_gpu_open_work.entry(queue_id).or_default()
                };
                if works.len() >= MAX_PROGRESSIVE_OPEN_GPU_WORK_PER_QUEUE {
                    works.pop_front();
                }
                works.push_back(ProgressiveGpuOpenWork {
                    gpu_timestamp_top,
                    cpu_timestamp,
                });
            }
            "EventEndWork" => {
                let queue_id = read_u32_field(event, data, "QueueId", base_offset)?;
                let gpu_timestamp_bop =
                    read_u64_field(event, data, "GPUTimestampBOP", base_offset)?;
                let Some(open) = self
                    .progressive_gpu_open_work
                    .get_mut(&queue_id)
                    .and_then(VecDeque::pop_back)
                else {
                    return Ok(());
                };
                if gpu_timestamp_bop < open.gpu_timestamp_top {
                    return Ok(());
                }
                self.record_progressive_gpu_work(ProgressiveGpuCompletedWork {
                    cpu_timestamp: open.cpu_timestamp,
                    duration_cycles: gpu_timestamp_bop - open.gpu_timestamp_top,
                });
            }
            _ => {}
        }
        Ok(())
    }

    fn record_progressive_gpu_work(&mut self, work: ProgressiveGpuCompletedWork) {
        if let Some(frame) = self.progressive_frames.iter_mut().rev().find(|frame| {
            work.cpu_timestamp >= frame.begin_cycle && work.cpu_timestamp <= frame.end_cycle
        }) {
            frame.gpu_submitted_work_count = frame.gpu_submitted_work_count.saturating_add(1);
            frame.gpu_submitted_work_cycles = frame
                .gpu_submitted_work_cycles
                .saturating_add(work.duration_cycles);
            self.progressive_frame_revision = self.progressive_frame_revision.saturating_add(1);
            return;
        }
        if self.pending_progressive_gpu_work.len() >= MAX_PENDING_PROGRESSIVE_GPU_WORK {
            self.pending_progressive_gpu_work.pop_front();
        }
        self.pending_progressive_gpu_work.push_back(work);
    }

    fn apply_pending_progressive_gpu_work(&mut self, frame: &mut ProgressiveFrameTiming) {
        let mut pending = std::mem::take(&mut self.pending_progressive_gpu_work);
        while let Some(work) = pending.pop_front() {
            if work.cpu_timestamp >= frame.begin_cycle && work.cpu_timestamp <= frame.end_cycle {
                frame.gpu_submitted_work_count = frame.gpu_submitted_work_count.saturating_add(1);
                frame.gpu_submitted_work_cycles = frame
                    .gpu_submitted_work_cycles
                    .saturating_add(work.duration_cycles);
            } else {
                self.pending_progressive_gpu_work.push_back(work);
            }
        }
    }

    fn packet_snapshot(&self) -> PacketSummary {
        let mut packets = self.summary.clone();
        packets.threads = self.threads.values().cloned().collect();
        packets.thread_count = packets.threads.len();
        packets
    }

    fn progress(&self, total_bytes: Option<u64>, phase: DecodePhase) -> DecodeProgress {
        DecodeProgress {
            bytes_consumed: self.pending_offset + u64::try_from(self.pending.len()).unwrap(),
            total_bytes,
            packets_observed: self.summary.count,
            phase,
        }
    }
}

pub(crate) type DashboardSession = ProgressiveDashboardSession;

fn complete_header_len(bytes: &[u8]) -> Result<Option<usize>, TraceError> {
    if bytes.len() < 4 {
        return Ok(None);
    }
    match &bytes[..4] {
        b"2CRT" => {
            if bytes.len() < 6 {
                return Ok(None);
            }
            let metadata = usize::from(u16::from_le_bytes([bytes[4], bytes[5]]));
            let length = 8_usize.checked_add(metadata).ok_or_else(|| {
                TraceError::new(
                    TraceErrorKind::ResourceLimit,
                    4,
                    "Header.MetadataSize",
                    "header size overflow",
                )
            })?;
            Ok((bytes.len() >= length).then_some(length))
        }
        b"ECRT" | b"TRC2" | b"TRCE" => Ok((bytes.len() >= 6).then_some(6)),
        [1, 0, 0, 0] => Ok(Some(2)),
        _ => Ok(Some(4)),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn minimal_trace() -> Vec<u8> {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"2CRT");
        bytes.extend_from_slice(&0_u16.to_le_bytes());
        bytes.extend_from_slice(&[4, 7]);
        bytes.extend_from_slice(&4_u16.to_le_bytes());
        bytes.extend_from_slice(&0x3fff_u16.to_le_bytes());
        bytes.extend_from_slice(&4_u16.to_le_bytes());
        bytes.extend_from_slice(&0x3fff_u16.to_le_bytes());
        bytes
    }

    fn declaration_with_flags(uid: u16, event: &str, flags: u8) -> Vec<u8> {
        let logger = "Misc";
        let fields = [(0_u16, 8_u16, 0x03_u8, "Cycle"), (8, 1, 0x00, "FrameType")];
        let mut data = Vec::new();
        data.extend_from_slice(&uid.to_le_bytes());
        data.push(2);
        data.push(flags);
        data.push(u8::try_from(logger.len()).unwrap());
        data.push(u8::try_from(event.len()).unwrap());
        for (offset, size, kind, name) in fields {
            data.extend_from_slice(&[0, 0]);
            data.extend_from_slice(&offset.to_le_bytes());
            data.extend_from_slice(&size.to_le_bytes());
            data.push(kind);
            data.push(u8::try_from(name.len()).unwrap());
        }
        data.extend_from_slice(logger.as_bytes());
        data.extend_from_slice(event.as_bytes());
        data.extend_from_slice(b"CycleFrameType");
        important_event(0, &data)
    }

    fn declaration(uid: u16, event: &str) -> Vec<u8> {
        declaration_with_flags(uid, event, 1)
    }

    fn important_event(uid: u16, data: &[u8]) -> Vec<u8> {
        let mut event = Vec::new();
        event.extend_from_slice(&uid.to_le_bytes());
        event.extend_from_slice(&u16::try_from(data.len()).unwrap().to_le_bytes());
        event.extend_from_slice(data);
        event
    }

    fn trace_with_frame() -> Vec<u8> {
        let mut begin = 100_u64.to_le_bytes().to_vec();
        begin.push(0);
        let mut end = 250_u64.to_le_bytes().to_vec();
        end.push(0);
        let payload = [
            declaration(16, "BeginFrame"),
            declaration(17, "EndFrame"),
            important_event(16, &begin),
            important_event(17, &end),
        ]
        .concat();
        let mut trace = b"2CRT\0\0\x04\x07".to_vec();
        trace.extend_from_slice(&u16::try_from(payload.len() + 4).unwrap().to_le_bytes());
        trace.extend_from_slice(&0_u16.to_le_bytes());
        trace.extend_from_slice(&payload);
        trace
    }

    fn trace_with_normal_frame() -> Vec<u8> {
        let declarations = [
            declaration_with_flags(16, "BeginFrame", 4),
            declaration_with_flags(17, "EndFrame", 4),
        ]
        .concat();
        let mut normal = Vec::new();
        normal.push(32);
        normal.extend_from_slice(&100_u64.to_le_bytes());
        normal.push(0);
        normal.push(34);
        normal.extend_from_slice(&250_u64.to_le_bytes());
        normal.push(0);
        let mut trace = b"2CRT\0\0\x04\x07".to_vec();
        trace.extend_from_slice(&u16::try_from(declarations.len() + 4).unwrap().to_le_bytes());
        trace.extend_from_slice(&0_u16.to_le_bytes());
        trace.extend_from_slice(&declarations);
        trace.extend_from_slice(&u16::try_from(normal.len() + 4).unwrap().to_le_bytes());
        trace.extend_from_slice(&2_u16.to_le_bytes());
        trace.extend_from_slice(&normal);
        trace
    }

    fn decode_in_chunks(
        bytes: &[u8],
        chunk_sizes: impl IntoIterator<Item = usize>,
    ) -> TraceDashboard {
        let mut session = DashboardSession::new(DashboardOptions::default());
        let mut offset = 0;
        for size in chunk_sizes {
            if offset == bytes.len() {
                break;
            }
            let end = offset.saturating_add(size).min(bytes.len());
            session.push_chunk(&bytes[offset..end]).unwrap();
            offset = end;
        }
        if offset < bytes.len() {
            session.push_chunk(&bytes[offset..]).unwrap();
        }
        session.finish().unwrap()
    }

    fn index_path(label: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("uasset-parser-session-{label}-{nonce}.utix"))
    }

    #[test]
    fn dashboard_is_invariant_across_adversarial_chunk_boundaries() {
        let bytes = minimal_trace();
        let expected = decode_in_chunks(&bytes, [bytes.len()]);
        let one_byte = decode_in_chunks(&bytes, std::iter::repeat_n(1, bytes.len()));
        let irregular = decode_in_chunks(&bytes, [3, 2, 4, 1, 5, 2]);
        let packet_aligned = decode_in_chunks(&bytes, [8, 4, 4]);
        assert_eq!(one_byte, expected);
        assert_eq!(irregular, expected);
        assert_eq!(packet_aligned, expected);
    }

    #[test]
    fn finish_rejects_each_partial_packet_header_position() {
        let bytes = minimal_trace();
        for truncated_len in [9, 10, 11] {
            let mut session = DashboardSession::new(DashboardOptions::default());
            session.push_chunk(&bytes[..truncated_len]).unwrap();
            let error = session.finish().unwrap_err();
            assert_eq!(error.kind(), TraceErrorKind::MalformedData);
            assert!(error.path().starts_with("Packet."));
        }
    }

    #[test]
    fn rejects_chunks_over_the_named_session_limit() {
        let mut session = DashboardSession::new(DashboardOptions::default());
        let error = session
            .push_chunk(&vec![0; MAX_PUSH_CHUNK_BYTES + 1])
            .unwrap_err();
        assert_eq!(error.kind(), TraceErrorKind::ResourceLimit);
        assert_eq!(error.path(), "Session.Chunk");
    }

    #[test]
    fn retains_frame_timing_for_the_completed_dashboard() {
        let bytes = trace_with_frame();
        let mut session = ProgressiveDashboardSession::new(DashboardOptions {
            max_frames: Some(1),
            ..DashboardOptions::default()
        });
        for byte in &bytes {
            session.push_chunk(std::slice::from_ref(byte)).unwrap();
        }
        let (_, patch) = session.frame_patch(Some(u64::try_from(bytes.len()).unwrap()));
        let DashboardPatch::Frames {
            total_frame_count,
            truncated,
            frames,
        } = patch
        else {
            panic!("expected frame patch");
        };
        assert_eq!(total_frame_count, 1);
        assert!(!truncated);
        assert_eq!(frames[0].begin_cycle, 100);
        assert_eq!(frames[0].end_cycle, 250);
        assert_eq!(frames[0].duration_cycles, 150);

        let dashboard = session.finish().unwrap();
        assert_eq!(
            dashboard
                .frame_timing
                .expect("completed dashboard retains streamed frame timing")
                .frames,
            frames,
        );
    }

    #[test]
    fn emits_frame_timing_from_normal_stream_before_finish() {
        let bytes = trace_with_normal_frame();
        let mut session = ProgressiveDashboardSession::new(DashboardOptions::default());
        session.push_chunk(&bytes).unwrap();
        let (_, patch) = session.frame_patch(Some(u64::try_from(bytes.len()).unwrap()));
        let DashboardPatch::Frames { frames, .. } = patch else {
            panic!("expected frame patch");
        };
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].duration_cycles, 150);
    }

    #[test]
    fn attributes_progressive_gpu_work_to_its_cpu_submission_frame() {
        let mut session = ProgressiveDashboardSession::new(DashboardOptions::default());
        session.record_frame_marker(crate::utrace::FrameMarker {
            kind: FrameMarkerKind::Begin,
            cycle: 100,
            frame_type: 0,
            thread_id: 2,
        });
        session.record_progressive_gpu_work(ProgressiveGpuCompletedWork {
            cpu_timestamp: 150,
            duration_cycles: 40,
        });
        session.record_frame_marker(crate::utrace::FrameMarker {
            kind: FrameMarkerKind::End,
            cycle: 200,
            frame_type: 0,
            thread_id: 2,
        });
        session.record_progressive_gpu_work(ProgressiveGpuCompletedWork {
            cpu_timestamp: 175,
            duration_cycles: 60,
        });

        let (_, patch) = session.frame_patch(None);
        let DashboardPatch::Frames { frames, .. } = patch else {
            panic!("expected frame patch");
        };
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].gpu_submitted_work_count, 2);
        assert_eq!(frames[0].gpu_submitted_work_cycles, 100);
        assert_eq!(session.frame_revision(), 2);
    }

    #[test]
    fn frame_pairing_matches_insights_begin_stack_and_end_extends_latest() {
        let mut session = ProgressiveDashboardSession::new(DashboardOptions::default());
        let mark = |kind, cycle, frame_type| crate::utrace::FrameMarker {
            kind,
            cycle,
            frame_type,
            thread_id: 2,
        };

        // Begin, Begin, End → Insights closes only the latest open frame.
        session.record_frame_marker(mark(FrameMarkerKind::Begin, 100, 0));
        session.record_frame_marker(mark(FrameMarkerKind::Begin, 150, 0));
        session.record_frame_marker(mark(FrameMarkerKind::End, 200, 0));
        assert_eq!(session.progressive_frames.len(), 1);
        assert_eq!(session.progressive_frames[0].begin_cycle, 150);
        assert_eq!(session.progressive_frames[0].end_cycle, 200);
        assert_eq!(session.progressive_frames[0].duration_cycles, 50);

        // Duplicate End extends the latest frame (Insights FFrameProvider).
        session.record_frame_marker(mark(FrameMarkerKind::End, 260, 0));
        assert_eq!(session.progressive_frames.len(), 1);
        assert_eq!(session.progressive_frames[0].end_cycle, 260);
        assert_eq!(session.progressive_frames[0].duration_cycles, 110);

        // Further Ends keep updating the latest slot; the earlier Begin stays
        // unpublished (Insights leaves it EndTime=infinity).
        session.record_frame_marker(mark(FrameMarkerKind::End, 400, 0));
        assert_eq!(session.progressive_frames.len(), 1);
        assert_eq!(session.progressive_frames[0].end_cycle, 400);
        assert_eq!(session.progressive_frames[0].duration_cycles, 250);
    }

    #[test]
    fn game_and_render_frame_types_pair_independently() {
        let mut session = ProgressiveDashboardSession::new(DashboardOptions::default());
        let mark = |kind, cycle, frame_type| crate::utrace::FrameMarker {
            kind,
            cycle,
            frame_type,
            thread_id: 2,
        };
        session.record_frame_marker(mark(FrameMarkerKind::Begin, 10, 0));
        session.record_frame_marker(mark(FrameMarkerKind::Begin, 20, 1));
        session.record_frame_marker(mark(FrameMarkerKind::End, 40, 0));
        session.record_frame_marker(mark(FrameMarkerKind::End, 35, 1));
        assert_eq!(session.progressive_frames.len(), 2);
        assert_eq!(session.progressive_frames[0].frame_type, 0);
        assert_eq!(session.progressive_frames[0].duration_cycles, 30);
        assert_eq!(session.progressive_frames[1].frame_type, 1);
        assert_eq!(session.progressive_frames[1].duration_cycles, 15);
    }

    #[test]
    fn session_built_index_matches_the_standalone_index() {
        let bytes = minimal_trace();
        let standalone_path = index_path("standalone");
        let session_path = index_path("session");
        crate::utrace::build_cpu_timeline_index(&bytes, &standalone_path, 1).unwrap();

        let mut session = ProgressiveDashboardSession::new(DashboardOptions::default());
        for chunk in bytes.chunks(3) {
            session.push_chunk(chunk).unwrap();
        }
        let (dashboard, _inventory, outcome) = session
            .finish_with_inventory_and_timeline_index(Some(TimelineIndexRequest {
                output: session_path.clone(),
                max_intervals: 1,
            }))
            .unwrap();
        assert_eq!(dashboard, decode_in_chunks(&bytes, [bytes.len()]));
        let outcome = outcome.expect("requested index outcome");
        assert_eq!(outcome.result.unwrap().source_bytes, bytes.len() as u64);
        assert_eq!(
            std::fs::read(&session_path).unwrap(),
            std::fs::read(&standalone_path).unwrap(),
        );
        std::fs::remove_file(standalone_path).unwrap();
        std::fs::remove_file(session_path).unwrap();
    }

    #[test]
    fn index_write_failure_does_not_fail_the_dashboard() {
        let bytes = minimal_trace();
        let missing_parent = index_path("missing-parent").join("never-created.utix");
        let mut session = ProgressiveDashboardSession::new(DashboardOptions::default());
        session.push_chunk(&bytes).unwrap();
        let (dashboard, _inventory, outcome) = session
            .finish_with_inventory_and_timeline_index(Some(TimelineIndexRequest {
                output: missing_parent,
                max_intervals: 1,
            }))
            .unwrap();
        assert_eq!(dashboard, decode_in_chunks(&bytes, [bytes.len()]));
        assert!(outcome.expect("requested index outcome").result.is_err());
    }
}
