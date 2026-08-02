//! Stable DTOs emitted by progressive UTrace dashboard sessions.

use serde::Serialize;

use crate::utrace::{PacketSummary, TraceHeader, TracePrologue, TraceThreadInfo};

pub const PROGRESS_PROTOCOL_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum DecodePhase {
    Reading,
    Analyzing,
    Complete,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DecodeProgress {
    pub bytes_consumed: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_bytes: Option<u64>,
    pub packets_observed: u64,
    pub phase: DecodePhase,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DashboardBootstrap {
    pub header: TraceHeader,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prologue: Option<TracePrologue>,
    pub thread_info: Vec<TraceThreadInfo>,
    pub declared_event_types: u64,
    pub packets: PacketSummary,
    pub thread_info_truncated: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ProgressiveFrameTiming {
    pub frame_number: u64,
    pub frame_type: u8,
    pub begin_cycle: u64,
    pub end_cycle: u64,
    pub duration_cycles: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<f64>,
    /// GPU work submitted while this CPU frame was active. This is intentionally
    /// distinct from Insights GPU frame time: it is the **sum** of overlapping
    /// `EventBeginWork`/`EventEndWork` durations whose CPU submit timestamp fell
    /// inside the marker, not queue-local `EventFrameBoundary` wall time.
    pub gpu_submitted_work_count: u64,
    pub gpu_submitted_work_cycles: u64,
}

/// Frame-marker timings produced while the capture is being decoded.
///
/// These are part of the completed dashboard as well as progressive snapshots,
/// so a chart can continue using the exact data it displayed while loading.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct FrameTimingDashboard {
    pub total_frame_count: u64,
    pub frames: Vec<ProgressiveFrameTiming>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DashboardPatch {
    Transport {
        packets: PacketSummary,
    },
    Frames {
        total_frame_count: u64,
        truncated: bool,
        frames: Vec<ProgressiveFrameTiming>,
    },
}
