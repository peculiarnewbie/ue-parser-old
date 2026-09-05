//! Resolve the small live-chart subset once, then decode borrowed event payloads.

use crate::utrace::{
    EventTypeInfo, FrameMarker, FrameMarkerKind, TraceError, decode_frame_marker, read_u32_field,
    read_u64_field,
};

#[derive(Clone, Copy)]
pub(super) enum LiveEventKind {
    Other,
    CpuBatch,
    Frame(FrameMarkerKind),
    GpuBeginWork,
    GpuEndWork,
}

pub(super) enum LiveEvent {
    Frame(FrameMarker),
    GpuBeginWork {
        queue_id: u32,
        gpu_timestamp_top: u64,
        cpu_timestamp: u64,
    },
    GpuEndWork {
        queue_id: u32,
        gpu_timestamp_bop: u64,
    },
}

impl From<&EventTypeInfo> for LiveEventKind {
    fn from(event: &EventTypeInfo) -> Self {
        match (event.logger.as_str(), event.event.as_str()) {
            ("CpuProfiler", "EventBatchV3") => Self::CpuBatch,
            ("Misc", "BeginFrame") => Self::Frame(FrameMarkerKind::Begin),
            ("Misc", "EndFrame") => Self::Frame(FrameMarkerKind::End),
            ("GpuProfiler", "EventBeginWork") => Self::GpuBeginWork,
            ("GpuProfiler", "EventEndWork") => Self::GpuEndWork,
            _ => Self::Other,
        }
    }
}

impl LiveEventKind {
    pub(super) fn needs_provider(self) -> bool {
        matches!(self, Self::Frame(_) | Self::GpuBeginWork | Self::GpuEndWork)
    }

    pub(super) fn decode(
        self,
        event: &EventTypeInfo,
        data: &[u8],
        base_offset: u64,
        thread_id: u16,
    ) -> Result<Option<LiveEvent>, TraceError> {
        Ok(Some(match self {
            Self::Frame(kind) => LiveEvent::Frame(decode_frame_marker(
                event,
                data,
                base_offset,
                thread_id,
                kind,
            )?),
            Self::GpuBeginWork => LiveEvent::GpuBeginWork {
                queue_id: read_u32_field(event, data, "QueueId", base_offset)?,
                gpu_timestamp_top: read_u64_field(event, data, "GPUTimestampTOP", base_offset)?,
                cpu_timestamp: read_u64_field(event, data, "CPUTimestamp", base_offset)?,
            },
            Self::GpuEndWork => LiveEvent::GpuEndWork {
                queue_id: read_u32_field(event, data, "QueueId", base_offset)?,
                gpu_timestamp_bop: read_u64_field(event, data, "GPUTimestampBOP", base_offset)?,
            },
            Self::Other | Self::CpuBatch => return Ok(None),
        }))
    }
}
