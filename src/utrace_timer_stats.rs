//! Exact CPU timer aggregation over a retained monotonic timeline range.
//!
//! Aggregation consumes every matching interval. Only the sorted response rows
//! are bounded, so returned timer values stay exact even when the response is
//! truncated.

use serde::Serialize;

use crate::utrace_timeline::CpuTimelineIndexInfo;

pub(crate) const MAX_AGGREGATED_TIMER_IDENTITIES: usize = 100_000;
pub(crate) const MAX_TIMER_STATS_ROWS: usize = 10_000;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CpuTimerStatsQuery {
    pub start_cycle: Option<u64>,
    pub end_cycle: Option<u64>,
    pub thread_id: Option<u16>,
    pub search: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CpuTimerStatsResult {
    pub index: CpuTimelineIndexInfo,
    pub begin_cycle: u64,
    pub end_cycle: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<f64>,
    pub interval_count: u64,
    pub distinct_timer_count: u64,
    /// True when the source index or sorted response row set was bounded.
    pub truncated: bool,
    pub timers: Vec<CpuTimerStatsRow>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CpuTimerStatsRow {
    pub spec_id: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata_id: Option<u32>,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rendered_name: Option<String>,
    /// Intervals overlapping the selected half-open range.
    pub overlap_count: u64,
    /// Intervals whose begin cycle falls inside the selected half-open range.
    pub begin_count: u64,
    /// Sum of each interval's intersection with the selected range.
    pub clipped_inclusive_cycles: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clipped_inclusive_seconds: Option<f64>,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TimerStatsAccumulator {
    pub overlap_count: u64,
    pub begin_count: u64,
    pub clipped_inclusive_cycles: u64,
}
