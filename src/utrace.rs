//! Read-only UTrace (`.utrace`) container inspection.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use rustc_hash::FxHashMap;
use serde::Serialize;

use crate::utrace_callstacks::{CallstackId, CallstackProvider, decode_callstack_spec};
use crate::utrace_format_args::{format_arg_display_strings, render_format_message};
use crate::utrace_memory::{
    LlmTag, LlmTagSet, LlmTracker, MemoryAllocation, MemoryFree, MemoryInit, MemoryProvider,
    MemoryTag,
};
use crate::utrace_modules::{
    ModuleProvider, decode_module_init, decode_module_load, decode_module_unload,
};
use crate::utrace_platform_file::PlatformFileProvider;
pub use crate::utrace_session::ProgressiveDashboardSession;
use crate::utrace_timeline::{CpuTimelineIndexBuilder, CpuTimelineSink, SinkAppetite};
pub use crate::utrace_timeline::{
    CpuTimelineIndexInfo, CpuTimelineMemoryIndex, CpuTimelineQuery, CpuTimelineQueryResult,
    DEFAULT_MAX_INDEXED_INTERVALS, SourceFingerprint, SourceIdentity, TimelineIndexBuild,
    TimelineIndexError, TimelineIndexRequest, query_cpu_timeline_index,
};
use crate::{ArchiveError, ArchiveErrorKind, Reader};

fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

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
    pub gpu: GpuDashboard,
    pub counters: CounterDashboard,
    pub stats: StatsDashboard,
    pub csv: CsvDashboard,
    pub loading: LoadingDashboard,
    pub io_store: IoStoreDashboard,
    pub platform_file: PlatformFileDashboard,
    pub trace_timing: TraceTimingDashboard,
    pub memory: MemoryDashboard,
    pub callstacks: CallstackDashboard,
    pub modules: ModuleDashboard,
    pub metadata_stack: MetadataStackDashboard,
    pub slate: SlateDashboard,
    pub channels: TraceChannelDashboard,
    pub thread_groups: ThreadGroupDashboard,
    pub tasks: TasksDashboard,
    pub annotations: AnnotationDashboard,
    pub logging: LogDashboard,
    pub unmodeled: UnmodeledTraceDashboard,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frame_timing: Option<crate::utrace_progress::FrameTimingDashboard>,
    pub frame_correlation: FrameCorrelationDashboard,
    pub frames: Vec<FrameMarker>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dispatch: Option<crate::utrace_dispatch::SerialDispatchSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionInfo>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct FrameCorrelationDashboard {
    pub total_frame_count: u64,
    pub truncated: bool,
    pub frames: Vec<CorrelatedFrameSummary>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CorrelatedFrameSummary {
    pub frame_number: u32,
    pub cpu_metadata_count: u64,
    pub cpu_metadata_cycles: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_metadata_seconds: Option<f64>,
    /// Bounds of the CPU scopes attributed to this metadata frame. These make
    /// the frame row directly queryable through the capture-wide CPU index.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_begin_cycle: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu_end_cycle: Option<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub top_cpu_scopes: Vec<CpuScopeSummary>,
    pub gpu_queue_count: u64,
    pub gpu_work_count: u64,
    pub gpu_work_cycles: u64,
    pub gpu_breadcrumb_count: u64,
    pub gpu_breadcrumb_cycles: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub top_gpu_breadcrumbs: Vec<GpuFrameBreadcrumbSummary>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TraceInventory {
    pub header: TraceHeader,
    pub summary: InventorySummary,
    pub events: Vec<EventInventoryEntry>,
    pub known_events: Vec<KnownEventInventoryEntry>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct InventorySummary {
    pub declared_event_types: u64,
    pub observed_event_types: u64,
    pub observed_events: u64,
    pub decoded_event_types: u64,
    pub partial_event_types: u64,
    pub raw_event_types: u64,
    pub known_event_types: u64,
    pub known_events: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EventInventoryEntry {
    pub uid: u16,
    pub logger: String,
    pub event: String,
    pub flags: EventFlags,
    pub fields: Vec<FieldInfo>,
    pub observed_count: u64,
    pub decode_status: DecodeStatus,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub samples: Vec<EventSample>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct EventSample {
    pub thread_id: u16,
    pub fields: BTreeMap<String, SampleValue>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct UnmodeledTraceDashboard {
    pub event_types: u64,
    pub observed_events: u64,
    pub events: Vec<UnmodeledTraceEventSummary>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct UnmodeledTraceEventSummary {
    pub logger: String,
    pub event: String,
    pub observed_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample: Option<EventSample>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum SampleValue {
    Unsigned(u64),
    Signed(i64),
    Float(f64),
    String(String),
    Raw(SampleRawValue),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SampleRawValue {
    pub kind: &'static str,
    pub byte_len: usize,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub hex_prefix: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct KnownEventInventoryEntry {
    pub uid: u16,
    pub name: &'static str,
    pub observed_count: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DecodeStatus {
    Decoded,
    Partial,
    Raw,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TraceCoverage {
    pub header: TraceHeader,
    pub summary: CoverageSummary,
    pub events: Vec<CoverageEntry>,
    pub decoders_not_observed: Vec<EventCoverage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub universe: Option<UniverseCoverage>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct CoverageSummary {
    pub declared_event_types: u64,
    pub decoded_event_types: u64,
    pub partial_event_types: u64,
    pub raw_event_types: u64,
    pub observed_events: u64,
    pub raw_observed_events: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CoverageEntry {
    pub logger: String,
    pub event: String,
    pub uid: u16,
    pub observed_count: u64,
    pub status: DecodeStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<&'static str>,
}

/// Cross-reference of the engine event universe against a trace's declared registry.
/// `unseen` are universe events this trace did not declare; `not_in_universe` are
/// events the trace declared that are absent from the harvested universe (game-specific
/// events, or `$Trace`-style declarations not written with `UE_TRACE_EVENT_BEGIN`).
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct UniverseCoverage {
    pub total: u64,
    pub declared_in_trace: u64,
    pub unseen: Vec<String>,
    pub not_in_universe: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct CpuDashboard {
    pub specs: Vec<CpuScopeSpec>,
    pub metadata: CpuMetadataDashboard,
    pub batches: CpuBatchSummary,
    pub scopes: Vec<CpuScopeSummary>,
    pub threads: Vec<CpuThreadSummary>,
    pub named_events: Vec<CpuNamedEventSummary>,
    pub end_threads: Vec<CpuEndThreadSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeline: Option<CpuTimelineDashboard>,
}

/// Bounded CPU timeline for one selected frame window.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct CpuTimelineDashboard {
    pub frame_number: u32,
    pub begin_cycle: u64,
    pub end_cycle: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<f64>,
    pub interval_count: u64,
    pub truncated: bool,
    pub intervals: Vec<CpuTimelineInterval>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CpuTimelineInterval {
    pub thread_id: u16,
    pub spec_id: u32,
    pub name: String,
    pub start_cycle: u64,
    pub end_cycle: u64,
    pub duration: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_seconds: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rendered_name: Option<String>,
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
    pub restored_metadata_scopes: u64,
    pub coroutine_records: u64,
    pub unmatched_ends: u64,
    pub unterminated_scopes: u64,
    /// Cold-start relative records that were StartCycle-anchored, then corrected
    /// when the thread jumped to flush-aligned absolute cycles.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub preamble_timeline_rebases: u64,
    /// Leave intervals whose duration still exceeded the capture-span safety net.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub implausible_duration_count: u64,
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub implausible_duration_cycles: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct CpuMetadataDashboard {
    pub specs: u64,
    pub specs_with_name_format: u64,
    pub field_names_bytes: u64,
    pub field_names: Vec<String>,
    pub records: u64,
    pub metadata_bytes: u64,
    pub scopes: u64,
    pub resolved_scopes: u64,
    pub unresolved_scopes: u64,
    pub decoded_records: u64,
    pub decoded_values: u64,
    pub decoded_metadata_bytes: u64,
    pub undecoded_records: u64,
    pub decode_failed_records: u64,
    pub undecoded_metadata_bytes: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub strings: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub samples: Vec<CpuMetadataSample>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub spec_summaries: Vec<CpuMetadataSpecSummary>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rendered_scopes: Vec<CpuMetadataRenderedScopeSummary>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub interval_samples: Vec<CpuMetadataIntervalSample>,
    pub top: Vec<CpuMetadataScopeSummary>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CpuMetadataSample {
    pub metadata_id: u32,
    pub spec_id: u32,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rendered_name: Option<String>,
    pub fields: BTreeMap<String, MetadataValue>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CpuMetadataSpecSummary {
    pub spec_id: u32,
    pub name: String,
    pub records: u64,
    pub metadata_bytes: u64,
    pub decoded_records: u64,
    pub decoded_values: u64,
    pub decoded_metadata_bytes: u64,
    pub undecoded_records: u64,
    pub decode_failed_records: u64,
    pub undecoded_metadata_bytes: u64,
    pub scopes: u64,
    pub total_cycles: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub strings: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub rendered_names: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample: Option<CpuMetadataSample>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CpuMetadataRenderedScopeSummary {
    pub spec_id: u32,
    pub name: String,
    pub rendered_name: String,
    pub count: u64,
    pub total_cycles: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CpuMetadataIntervalSample {
    pub spec_id: u32,
    pub metadata_id: u32,
    pub attribution: CpuMetadataAttribution,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rendered_name: Option<String>,
    pub start_cycle: u64,
    pub end_cycle: u64,
    pub duration_cycles: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CpuMetadataAttribution {
    Inline,
    RestoredStack,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum MetadataValue {
    Null,
    Bool { value: bool },
    Unsigned { value: u64 },
    Signed { value: i64 },
    Float { value: f64 },
    Text { value: String },
    Bytes { byte_len: usize, hex_prefix: String },
    Array { values: Vec<MetadataValue> },
    Map { entries: Vec<MetadataMapEntry> },
    Unknown { kind: &'static str, byte_len: usize },
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct MetadataMapEntry {
    pub key: MetadataValue,
    pub value: MetadataValue,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CpuMetadataScopeSummary {
    pub spec_id: u32,
    pub name: String,
    pub count: u64,
    pub total_cycles: u64,
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_group: Option<String>,
    pub count: u64,
    pub total_cycles: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_seconds: Option<f64>,
    pub scopes: Vec<CpuScopeSummary>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CpuNamedEventSummary {
    pub event: String,
    pub observed_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample: Option<EventSample>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CpuEndThreadSummary {
    pub thread_id: u16,
    pub cycle: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct GpuDashboard {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<u8>,
    pub queues: Vec<GpuQueueSummary>,
    pub frames: Vec<GpuFrameSummary>,
    pub total_frame_count: u64,
    pub frames_truncated: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeline: Option<GpuTimelineDashboard>,
    pub work: GpuWorkSummary,
    pub breadcrumbs: GpuBreadcrumbDashboard,
    /// Bounded samples of GPU-start vs CPU-submit times from `EventBeginWork`.
    /// This is submission latency / queue delay, not clock-domain calibration:
    /// `CPUTimestamp` is when work was submitted, `GPUTimestampTOP` is when it
    /// began executing (see UE `GPUProfiler.h`). Insights does not use the CPU
    /// field for time conversion.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub submission_latency: Option<GpuSubmissionLatency>,
}

/// Bounded GPU work and breadcrumb timeline for one queue-local frame number.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct GpuTimelineDashboard {
    pub frame_number: u32,
    pub begin_timestamp: u64,
    pub end_timestamp: u64,
    pub interval_count: u64,
    pub truncated: bool,
    pub intervals: Vec<GpuTimelineInterval>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum GpuTimelineIntervalKind {
    Work,
    Breadcrumb,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GpuTimelineInterval {
    pub queue_id: u32,
    pub kind: GpuTimelineIntervalKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec_id: Option<u32>,
    pub name: String,
    pub start_timestamp: u64,
    pub end_timestamp: u64,
    pub duration: u64,
}

/// Bounded GPU-start vs CPU-submit pairing from `EventBeginWork`.
///
/// This is not clock calibration: the two timestamps measure different events
/// (execution begin vs CPU submission).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GpuSubmissionLatency {
    pub sample_count: u64,
    /// Median (GPU TOP − CPU submit) in cycles; positive means GPU started after submit.
    pub median_delay_cycles: i128,
    pub min_delay_cycles: i128,
    pub max_delay_cycles: i128,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub samples: Vec<GpuSubmissionLatencySample>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GpuSubmissionLatencySample {
    pub queue_id: u32,
    pub gpu_timestamp_top: u64,
    pub cpu_submit_timestamp: u64,
    pub delay_cycles: i128,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GpuFrameSummary {
    pub queue_id: u32,
    pub frame_number: u32,
    pub boundary_count: u64,
    pub work_count: u64,
    pub work_total_cycles: u64,
    pub breadcrumb_count: u64,
    pub breadcrumb_total_cycles: u64,
    pub wait_count: u64,
    pub wait_total_cycles: u64,
    pub draw_count: u64,
    pub primitive_count: u64,
    pub signal_fence_count: u64,
    pub wait_fence_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_gpu_timestamp: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_gpu_timestamp: Option<u64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub top_breadcrumbs: Vec<GpuFrameBreadcrumbSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GpuFrameBreadcrumbSummary {
    pub name: String,
    pub count: u64,
    pub total_cycles: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GpuQueueSummary {
    pub queue_id: u32,
    pub gpu: u8,
    pub index: u8,
    pub queue_type: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub work_count: u64,
    pub work_total_cycles: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_gpu_timestamp: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_gpu_timestamp: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_cpu_timestamp: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_cpu_timestamp: Option<u64>,
    pub wait_count: u64,
    pub wait_total_cycles: u64,
    pub frame_boundary_count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_frame_number: Option<u32>,
    pub draw_count: u64,
    pub primitive_count: u64,
    pub signal_fence_count: u64,
    pub wait_fence_count: u64,
    pub breadcrumb_count: u64,
    pub breadcrumb_total_cycles: u64,
    pub breadcrumb_metadata_count: u64,
    pub breadcrumb_metadata_bytes: u64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub breadcrumb_metadata_hex_prefix: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub breadcrumb_metadata_strings: Vec<String>,
    pub unmatched_breadcrumb_ends: u64,
    pub negative_breadcrumb_durations: u64,
    pub unterminated_breadcrumbs: u64,
    pub unmatched_work_ends: u64,
    pub negative_work_durations: u64,
    pub unterminated_work: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct GpuWorkSummary {
    pub queues: u64,
    pub intervals: u64,
    pub total_cycles: u64,
    pub unmatched_ends: u64,
    pub negative_durations: u64,
    pub unterminated_scopes: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct GpuBreadcrumbDashboard {
    pub specs: u64,
    pub specs_with_name_format: u64,
    pub field_names_bytes: u64,
    pub field_names: Vec<String>,
    pub intervals: u64,
    pub total_cycles: u64,
    pub metadata_events: u64,
    pub metadata_bytes: u64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub metadata_hex_prefix: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metadata_strings: Vec<String>,
    pub decoded_metadata_bytes: u64,
    pub undecoded_metadata_bytes: u64,
    pub decode_failed_events: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metadata_samples: Vec<GpuBreadcrumbMetadataSample>,
    pub unmatched_ends: u64,
    pub negative_durations: u64,
    pub unterminated_scopes: u64,
    pub top: Vec<GpuBreadcrumbSummary>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct GpuBreadcrumbMetadataSample {
    pub spec_id: u32,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rendered_name: Option<String>,
    pub fields: BTreeMap<String, MetadataValue>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct GpuBreadcrumbSummary {
    pub spec_id: u32,
    pub name: String,
    pub count: u64,
    pub total_cycles: u64,
    pub metadata_events: u64,
    pub metadata_bytes: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct CounterDashboard {
    pub specs: u64,
    pub counters: Vec<CounterSummary>,
    pub samples: u64,
    pub int_samples: u64,
    pub float_samples: u64,
    pub unresolved_samples: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CounterSummary {
    pub id: u16,
    pub name: String,
    pub kind: CounterKind,
    pub display_hint: CounterDisplayHint,
    pub samples: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_cycle: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_cycle: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest: Option<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sample_points: Vec<CounterSamplePoint>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CounterSamplePoint {
    pub cycle: u64,
    pub value: f64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CounterKind {
    Int,
    Float,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CounterDisplayHint {
    None,
    Memory,
    Unknown,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct StatsDashboard {
    pub specs: u64,
    pub floating_point_specs: u64,
    pub memory_specs: u64,
    pub clear_every_frame_specs: u64,
    pub sample_events: u64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub unresolved_samples: u64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub sample_state_overflow: u64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub malformed_batches: u64,
    pub groups: Vec<StatGroupSummary>,
    pub stats: Vec<StatSpecSummary>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub samples: Vec<StatSampleSummary>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StatSampleSummary {
    pub id: u32,
    pub samples: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_cycle: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_cycle: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest: Option<f64>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sample_points: Vec<StatSamplePoint>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StatSamplePoint {
    pub cycle: u64,
    pub value: f64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StatGroupSummary {
    pub name: String,
    pub specs: u64,
    pub floating_point_specs: u64,
    pub memory_specs: u64,
    pub clear_every_frame_specs: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct StatSpecSummary {
    pub id: u32,
    pub name: String,
    pub description: String,
    pub group: String,
    pub is_floating_point: bool,
    pub is_memory: bool,
    pub should_clear_every_frame: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct CsvDashboard {
    pub categories: u64,
    pub stats: u64,
    pub declared_stats: u64,
    pub inline_stats: u64,
    pub unresolved_stats: u64,
    pub sample_events: u64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub begin_events: u64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub end_events: u64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub unmatched_ends: u64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub custom_int_samples: u64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub custom_float_samples: u64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub open_begins: u64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub sample_unresolved_stats: u64,
    pub top_categories: Vec<CsvCategorySummary>,
    pub stat_defs: Vec<CsvStatSummary>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub duration_samples: Vec<CsvDurationSample>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub value_samples: Vec<CsvValueSample>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CsvDurationSample {
    pub thread_id: u16,
    pub stat_id: u64,
    pub begin_cycle: u64,
    pub end_cycle: u64,
    pub duration_cycles: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct CsvValueSample {
    pub thread_id: u16,
    pub stat_id: u64,
    pub cycle: u64,
    pub value: f64,
    pub op_type: u8,
    pub kind: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct LoadingDashboard {
    pub class_count: u64,
    pub classes: Vec<LoadTimeClassSummary>,
    pub package_count: u64,
    pub packages: Vec<LoadTimePackageSummary>,
    pub requests: LoadTimeRequestDashboard,
    pub async_loading: LoadTimeAsyncLoadingSummary,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LoadTimeClassSummary {
    pub class: u64,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LoadTimePackageSummary {
    pub async_package: u64,
    pub name: String,
    pub total_header_size: u32,
    pub import_count: u32,
    pub export_count: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub priority: Option<i32>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct LoadTimeRequestDashboard {
    pub begun: u64,
    pub ended: u64,
    pub completed: u64,
    pub unmatched_ends: u64,
    pub open: u64,
    pub total_cycles: u64,
    pub samples: Vec<LoadTimeRequestSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LoadTimeRequestSummary {
    pub request_id: u64,
    pub start_cycle: u64,
    pub end_cycle: u64,
    pub duration_cycles: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct LoadTimeAsyncLoadingSummary {
    pub starts: u64,
    pub suspends: u64,
    pub resumes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_cycle: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_cycle: Option<u64>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct IoStoreDashboard {
    pub backend_count: u64,
    pub backends: Vec<IoStoreBackendSummary>,
    pub requests_created: u64,
    pub requests_started: u64,
    pub requests_completed: u64,
    pub requests_failed: u64,
    pub requests_unresolved: u64,
    pub bytes_requested: u64,
    pub bytes_completed: u64,
    pub request_samples: Vec<IoStoreRequestSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct IoStoreBackendSummary {
    pub backend_handle: u64,
    pub name: String,
    pub starts: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct IoStoreRequestSummary {
    pub request_handle: u64,
    pub batch_handle: u64,
    pub chunk_id_hash: u32,
    pub chunk_type: u8,
    pub offset: u64,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend_handle: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backend_name: Option<String>,
    pub create_cycle: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_cycle: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub complete_cycle: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_size: Option<u64>,
    pub status: IoStoreRequestStatus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IoStoreRequestStatus {
    Created,
    Started,
    Completed,
    Failed,
    Unresolved,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct PlatformFileDashboard {
    pub file_count: u64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub file_overflow: u64,
    pub files: Vec<PlatformFileSummary>,
    pub opens: u64,
    pub open_failures: u64,
    pub reopens: u64,
    pub closes: u64,
    pub reads: u64,
    pub writes: u64,
    pub bytes_read: u64,
    pub bytes_written: u64,
    pub bytes_requested_read: u64,
    pub bytes_requested_write: u64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub unpaired_ends: u64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub unknown_handle_ops: u64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub open_handle_overflow: u64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub active_op_overflow: u64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub activity_sample_overflow: u64,
    pub activity_samples: Vec<PlatformFileActivitySample>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PlatformFileSummary {
    pub path: String,
    pub opens: u64,
    pub open_failures: u64,
    pub reopens: u64,
    pub closes: u64,
    pub reads: u64,
    pub writes: u64,
    pub bytes_read: u64,
    pub bytes_written: u64,
    pub bytes_requested_read: u64,
    pub bytes_requested_write: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum PlatformFileActivityKind {
    Open,
    ReOpen,
    Close,
    Read,
    Write,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct PlatformFileActivitySample {
    pub kind: PlatformFileActivityKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub thread_id: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_handle: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub op_handle: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub offset: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual_size: Option<u64>,
    pub start_cycle: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub end_cycle: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_cycles: Option<u64>,
    pub failed: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct TraceTimingDashboard {
    pub thread_count: u64,
    pub threads: Vec<TraceThreadTimingSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TraceThreadTimingSummary {
    pub thread_id: u16,
    pub base_timestamp: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct MemoryDashboard {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub init: Option<MemoryInitSummary>,
    pub tag_count: u64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub tag_overflow: u64,
    pub tags: Vec<MemoryTagSummary>,
    pub scope_count: u64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub scope_tag_overflow: u64,
    pub scopes: Vec<MemoryScopeSummary>,
    pub allocs: MemoryAllocationDashboard,
    pub llm: MemoryLlmDashboard,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct CallstackDashboard {
    pub observed: u64,
    pub retained: u64,
    pub dropped: u64,
    pub truncated: bool,
    pub duplicate_ids: u64,
    pub malformed: u64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub id_zero: u64,
    pub total_frames_retained: u64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub unresolved_references: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stacks: Vec<CallstackEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CallstackEntry {
    pub id: u32,
    pub frame_count: u64,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub frames_truncated: bool,
    /// Raw program-counter addresses as JSON-safe hex strings.
    pub frames: Vec<String>,
    /// Module/relative mapping for the same ordered frames (empty when unmapped).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub mapped_frames: Vec<MappedCallstackFrame>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MappedCallstackFrame {
    pub address: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relative_address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<ModuleIdentity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    pub status: MappedFrameStatus,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum MappedFrameStatus {
    Unmapped,
    ModuleOffset,
    Symbol,
    Ambiguous,
    IdentityMismatch,
    SymbolsMissing,
    ResolverError,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum CallstackResolution {
    None,
    Resolved,
    Missing,
    CatalogTruncated,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ModuleDashboard {
    pub init_seen: bool,
    pub missing_init: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_format: Option<SymbolFormat>,
    pub module_base_shift: u8,
    pub observed_loads: u64,
    pub observed_unloads: u64,
    pub retained: u64,
    pub dropped: u64,
    pub truncated: bool,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub duplicate_bases: u64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub unload_without_load: u64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub malformed: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub modules: Vec<ModuleEntry>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ModuleEntry {
    pub name: String,
    pub base: String,
    pub size: u32,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub image_id_hex: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<ModuleIdentity>,
    pub unloaded: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ModuleIdentity {
    pub guid: String,
    pub age: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum SymbolFormat {
    Pdb,
    Dwarf,
    Psym,
    Other,
}

impl SymbolFormat {
    pub(crate) fn parse(value: &str) -> Self {
        match value {
            "pdb" => Self::Pdb,
            "dwarf" => Self::Dwarf,
            "psym" => Self::Psym,
            _ => Self::Other,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum ModuleFrameMapping {
    Unmapped,
    Mapped(ModuleFrameMap),
    Ambiguous { candidates: Vec<String> },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModuleFrameMap {
    pub module: String,
    pub base: u64,
    pub relative_address: u64,
    pub identity: Option<ModuleIdentity>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MemoryInitSummary {
    pub version: u8,
    pub page_size: u64,
    pub marker_period: u32,
    pub min_alignment: u8,
    pub size_shift: u8,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MemoryTagSummary {
    pub tag: i32,
    pub parent: i32,
    pub display: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct MemoryAllocationDashboard {
    pub count: u64,
    pub free_count: u64,
    pub realloc_alloc_count: u64,
    pub realloc_free_count: u64,
    pub bytes_allocated: u64,
    pub bytes_freed: u64,
    pub net_bytes: i64,
    pub unresolved_free: u64,
    pub outstanding_allocations: u64,
    pub outstanding_bytes: u64,
    pub outstanding_overflow: bool,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub outstanding_dropped: u64,
    pub by_root_heap: Vec<MemoryRootHeapSummary>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub samples: Vec<MemoryAllocationSample>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MemoryRootHeapSummary {
    pub root_heap: u8,
    pub name: String,
    pub alloc_count: u64,
    pub free_count: u64,
    pub bytes_allocated: u64,
    pub bytes_freed: u64,
    pub net_bytes: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MemoryAllocationSample {
    pub address: u64,
    pub size: u64,
    pub root_heap: u8,
    pub callstack_id: u32,
    pub callstack: CallstackResolution,
    pub kind: MemoryAllocationKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum MemoryAllocationKind {
    Alloc,
    ReallocAlloc,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct MemoryLlmDashboard {
    pub tag_count: u64,
    pub tracker_count: u64,
    pub tag_set_count: u64,
    pub sample_events: u64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub tag_overflow: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<MemoryLlmTagSummary>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub trackers: Vec<MemoryLlmTrackerSummary>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tag_sets: Vec<MemoryLlmTagSetSummary>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub latest_values: Vec<MemoryLlmValueSummary>,
    pub latest_values_overflow: bool,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub latest_values_dropped: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MemoryLlmTagSummary {
    pub tag: i64,
    pub parent: i64,
    pub tag_set: u8,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MemoryLlmTrackerSummary {
    pub tracker_id: u8,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MemoryLlmTagSetSummary {
    pub tag_set: u8,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MemoryLlmValueSummary {
    pub tracker_id: u8,
    pub cycle: u64,
    pub tag: i64,
    pub value: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MemoryScopeSummary {
    pub tag: i32,
    pub count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct MetadataStackDashboard {
    pub clear_scope_count: u64,
    pub saved_stack_count: u64,
    pub restored_stack_count: u64,
    pub unmatched_restore_count: u64,
    pub saved_stacks: Vec<MetadataSavedStackSummary>,
    pub restored_stacks: Vec<MetadataRestoredStackSummary>,
    pub stack_ids: Vec<MetadataStackIdSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MetadataSavedStackSummary {
    pub id: u32,
    pub count: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MetadataRestoredStackSummary {
    pub id: u32,
    pub count: u64,
    pub saved: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct MetadataStackIdSummary {
    pub id: u32,
    pub saves: u64,
    pub restores: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct SlateDashboard {
    pub added_widgets: u64,
    pub widgets: Vec<SlateWidgetSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct SlateWidgetSummary {
    pub widget_id: u64,
    pub count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_cycle: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_cycle: Option<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CsvCategorySummary {
    pub index: i32,
    pub name: String,
    pub stats: u64,
    pub declared_stats: u64,
    pub inline_stats: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct CsvStatSummary {
    pub stat_id: u64,
    pub name: String,
    pub category_index: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub kind: CsvStatKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CsvStatKind {
    Declared,
    Inline,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct TraceChannelDashboard {
    pub count: u64,
    pub enabled: u64,
    pub read_only: u64,
    pub toggles: u64,
    pub channels: Vec<TraceChannelSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TraceChannelSummary {
    pub id: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub is_enabled: bool,
    pub read_only: bool,
    pub toggle_count: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct ThreadGroupDashboard {
    pub begin_events: u64,
    pub end_events: u64,
    pub unmatched_ends: u64,
    pub unclosed_groups: u64,
    pub groups: Vec<ThreadGroupSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ThreadGroupSummary {
    pub name: String,
    pub begin_count: u64,
    pub end_count: u64,
    pub balanced: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct TasksDashboard {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub init_version: Option<u32>,
    pub created: u64,
    pub launched: u64,
    pub scheduled: u64,
    pub started: u64,
    pub finished: u64,
    pub completed: u64,
    pub destroyed: u64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub subsequent_added: u64,
    pub wait_count: u64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub wait_started: u64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub wait_finished: u64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub unmatched_wait_ends: u64,
    #[serde(skip_serializing_if = "is_zero_u64")]
    pub open_waits: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub wait_samples: Vec<TaskWaitSample>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub named_tasks: Vec<TaskNameSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TaskWaitSample {
    pub thread_id: u16,
    pub start_cycle: u64,
    pub end_cycle: u64,
    pub duration_cycles: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub task_ids: Vec<u64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TaskNameSummary {
    pub task_id: u64,
    pub debug_name: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct AnnotationDashboard {
    pub bookmarks: BookmarkDashboard,
    pub regions: RegionDashboard,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct BookmarkDashboard {
    pub specs: u64,
    pub events: u64,
    pub format_args_bytes: u64,
    pub unresolved_events: u64,
    pub bookmarks: Vec<BookmarkSummary>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BookmarkSummary {
    pub bookmark_point: u64,
    pub format_string: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<i32>,
    pub count: u64,
    pub format_args_bytes: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sample_args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_cycle: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_cycle: Option<u64>,
    pub callstack_count: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub callstack_samples: Vec<BookmarkCallstackSample>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BookmarkCallstackSample {
    pub cycle: u64,
    pub callstack_id: u32,
    pub callstack: CallstackResolution,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct RegionDashboard {
    pub begin_events: u64,
    pub end_events: u64,
    pub completed: u64,
    pub unmatched_ends: u64,
    pub unterminated: u64,
    pub with_id_begin_events: u64,
    pub with_id_end_events: u64,
    pub regions: Vec<RegionSummary>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RegionSummary {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub count: u64,
    pub total_cycles: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct LogDashboard {
    pub categories: u64,
    pub message_specs: u64,
    pub messages: u64,
    pub format_args_bytes: u64,
    pub unresolved_messages: u64,
    pub specs_with_unknown_category: u64,
    pub verbosity: Vec<LogVerbosityCount>,
    pub top_categories: Vec<LogCategorySummary>,
    pub top_messages: Vec<LogMessageSummary>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LogVerbosityCount {
    pub verbosity: LogVerbosity,
    pub message_specs: u64,
    pub messages: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LogCategorySummary {
    pub name: String,
    pub default_verbosity: LogVerbosity,
    pub message_specs: u64,
    pub messages: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LogMessageSummary {
    pub log_point: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub verbosity: LogVerbosity,
    pub format_string: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<i32>,
    pub count: u64,
    pub format_args_bytes: u64,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sample_args: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_cycle: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_cycle: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogVerbosity {
    NoLogging,
    Fatal,
    Error,
    Warning,
    Display,
    Log,
    Verbose,
    VeryVerbose,
    Unknown,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SessionInfo {
    pub platform: String,
    pub app_name: String,
    pub project_name: String,
    pub command_line: String,
    pub branch: String,
    pub build_version: String,
    pub changelist: u32,
    pub configuration: BuildConfiguration,
    pub target_type: BuildTargetType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub instance_id: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub vfs_paths: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildConfiguration {
    Unknown,
    Debug,
    DebugGame,
    Development,
    Shipping,
    Test,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildTargetType {
    Unknown,
    Game,
    Server,
    Client,
    Editor,
    Program,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_date_time: Option<f64>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TraceThreadInfo {
    pub thread_id: u32,
    pub system_id: u32,
    pub sort_hint: i32,
    pub name: String,
    /// Groups active when this thread was registered (`$Trace.ThreadGroupBegin` stack).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub groups: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_group: Option<String>,
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
pub(super) struct DecodedStreams {
    pub(super) summary: PacketSummary,
    pub(super) streams: BTreeMap<u16, Vec<u8>>,
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
    ResourceLimit,
    UnsupportedFormat,
}

impl TraceError {
    pub(crate) fn new(
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
            ArchiveErrorKind::AllocationLimit => TraceErrorKind::ResourceLimit,
            ArchiveErrorKind::MissingNullTerminator
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

pub fn inventory(source: &[u8]) -> Result<TraceInventory, TraceError> {
    let mut reader = Reader::new(source);
    let header = read_header(&mut reader)?;
    let decoded = read_packets(&mut reader)?;
    inventory_from_decoded(header, &decoded)
}

pub(super) fn inventory_from_decoded(
    header: TraceHeader,
    decoded: &DecodedStreams,
) -> Result<TraceInventory, TraceError> {
    let events = read_event_registry(&header, &decoded.streams)?;
    let registry = events
        .iter()
        .map(|event| (event.uid, event))
        .collect::<BTreeMap<_, _>>();
    let mut observed = BTreeMap::<u16, u64>::new();
    let mut known_observed = BTreeMap::<u16, u64>::new();
    let mut samples_by_uid = BTreeMap::<u16, RawSample>::new();

    for (thread_id, stream) in &decoded.streams {
        if *thread_id <= 1 {
            for raw_event in read_protocol5_important_events(stream)? {
                if registry.contains_key(&raw_event.uid) {
                    *observed.entry(raw_event.uid).or_default() += 1;
                    samples_by_uid
                        .entry(raw_event.uid)
                        .or_insert_with(|| RawSample {
                            thread_id: *thread_id,
                            data: raw_event.data.to_vec(),
                        });
                } else {
                    *known_observed.entry(raw_event.uid).or_default() += 1;
                }
            }
        } else {
            for raw_event in read_protocol5_normal_events(stream, &registry)? {
                if registry.contains_key(&raw_event.uid) {
                    *observed.entry(raw_event.uid).or_default() += 1;
                    samples_by_uid
                        .entry(raw_event.uid)
                        .or_insert_with(|| RawSample {
                            thread_id: *thread_id,
                            data: raw_event.data.clone(),
                        });
                } else {
                    *known_observed.entry(raw_event.uid).or_default() += 1;
                }
            }
        }
    }

    let mut inventory_events = events
        .into_iter()
        .map(|event| {
            let observed_count = observed.get(&event.uid).copied().unwrap_or(0);
            let samples = samples_by_uid
                .get(&event.uid)
                .map(|sample| decode_event_sample(&event, sample))
                .transpose()?
                .into_iter()
                .collect();
            Ok(EventInventoryEntry {
                decode_status: decode_status_for(&event),
                uid: event.uid,
                logger: event.logger,
                event: event.event,
                flags: event.flags,
                fields: event.fields,
                observed_count,
                samples,
            })
        })
        .collect::<Result<Vec<_>, TraceError>>()?;
    inventory_events.sort_by(|left, right| {
        right
            .observed_count
            .cmp(&left.observed_count)
            .then_with(|| left.logger.cmp(&right.logger))
            .then_with(|| left.event.cmp(&right.event))
    });

    let known_events = known_observed
        .iter()
        .map(|(uid, observed_count)| KnownEventInventoryEntry {
            uid: *uid,
            name: known_event_name(*uid),
            observed_count: *observed_count,
        })
        .collect::<Vec<_>>();

    let observed_event_types =
        u64::try_from(observed.values().filter(|count| **count > 0).count()).unwrap();
    let observed_events = observed.values().sum();
    let decoded_event_types = u64::try_from(
        inventory_events
            .iter()
            .filter(|event| {
                event.observed_count > 0 && event.decode_status == DecodeStatus::Decoded
            })
            .count(),
    )
    .unwrap();
    let partial_event_types = u64::try_from(
        inventory_events
            .iter()
            .filter(|event| {
                event.observed_count > 0 && event.decode_status == DecodeStatus::Partial
            })
            .count(),
    )
    .unwrap();
    let raw_event_types = u64::try_from(
        inventory_events
            .iter()
            .filter(|event| event.observed_count > 0 && event.decode_status == DecodeStatus::Raw)
            .count(),
    )
    .unwrap();
    Ok(TraceInventory {
        header,
        summary: InventorySummary {
            declared_event_types: u64::try_from(inventory_events.len()).unwrap(),
            observed_event_types,
            observed_events,
            decoded_event_types,
            partial_event_types,
            raw_event_types,
            known_event_types: u64::try_from(known_events.len()).unwrap(),
            known_events: known_events
                .iter()
                .map(|event| event.observed_count)
                .sum::<u64>(),
        },
        events: inventory_events,
        known_events,
    })
}

/// A single (logger, event) pair this parser knows how to decode, its decode
/// status, and a short note on what is decoded or knowingly dropped.
///
/// This table is the single source of truth for decode coverage: `decode_status_for`
/// looks up here, and the coverage report ([`coverage`]) renders and cross-references
/// it against a trace's declared registry (and, optionally, the engine event universe).
/// Adding a decoder means adding a row here — nothing else classifies events.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct EventCoverage {
    pub logger: &'static str,
    pub event: &'static str,
    pub status: DecodeStatus,
    pub note: &'static str,
}

/// Every (logger, event) pair with a dedicated decoder, most-decoded first.
/// Anything absent is [`DecodeStatus::Raw`] (counted only).
pub const EVENT_COVERAGE: &[EventCoverage] = &[
    EventCoverage {
        logger: "$Trace",
        event: "NewTrace",
        status: DecodeStatus::Decoded,
        note: "Trace prologue: start cycle, cycle frequency, endian, pointer size, and optional start date.",
    },
    EventCoverage {
        logger: "$Trace",
        event: "ThreadInfo",
        status: DecodeStatus::Decoded,
        note: "Thread id, system id, sort hint, and name.",
    },
    EventCoverage {
        logger: "$Trace",
        event: "ThreadGroupBegin",
        status: DecodeStatus::Partial,
        note: "Thread group begin; ThreadInfo rows inherit the active group stack.",
    },
    EventCoverage {
        logger: "$Trace",
        event: "ThreadGroupEnd",
        status: DecodeStatus::Partial,
        note: "Thread group end stack accounting and unmatched end counts.",
    },
    EventCoverage {
        logger: "$Trace",
        event: "ThreadTiming",
        status: DecodeStatus::Partial,
        note: "Per-thread base timestamp catalog.",
    },
    EventCoverage {
        logger: "CpuProfiler",
        event: "EventSpec",
        status: DecodeStatus::Decoded,
        note: "CPU scope spec: id, name, file, line.",
    },
    EventCoverage {
        logger: "Misc",
        event: "BeginFrame",
        status: DecodeStatus::Decoded,
        note: "Frame begin marker.",
    },
    EventCoverage {
        logger: "Misc",
        event: "EndFrame",
        status: DecodeStatus::Decoded,
        note: "Frame end marker.",
    },
    EventCoverage {
        logger: "CpuProfiler",
        event: "MetadataSpec",
        status: DecodeStatus::Partial,
        note: "Metadata spec header; CBOR field layout not expanded.",
    },
    EventCoverage {
        logger: "CpuProfiler",
        event: "Metadata",
        status: DecodeStatus::Partial,
        note: "Metadata record linked to its spec; bounded CBOR values and representative names are expanded.",
    },
    EventCoverage {
        logger: "CpuProfiler",
        event: "EventBatchV3",
        status: DecodeStatus::Partial,
        note: "Scope enter/leave intervals aggregated to cycle totals; per-thread batch, coroutine, and late-connect base-cycle state follow Insights ProcessBufferV2. Optional bounded per-frame timelines via dashboard --frame.",
    },
    EventCoverage {
        logger: "CpuProfiler",
        event: "EndThread",
        status: DecodeStatus::Partial,
        note: "CPU profiler thread end cycle marker.",
    },
    EventCoverage {
        logger: "Counters",
        event: "Spec",
        status: DecodeStatus::Partial,
        note: "Counter spec: id, type, display hint, name.",
    },
    EventCoverage {
        logger: "Counters",
        event: "SetValueInt",
        status: DecodeStatus::Partial,
        note: "Integer counter samples summarized (min/max/latest); no full time series.",
    },
    EventCoverage {
        logger: "Counters",
        event: "SetValueFloat",
        status: DecodeStatus::Partial,
        note: "Float counter samples summarized (min/max/latest); no full time series.",
    },
    EventCoverage {
        logger: "Stats",
        event: "Spec",
        status: DecodeStatus::Partial,
        note: "Stat catalog: id, name, description, group, and stat flags.",
    },
    EventCoverage {
        logger: "Stats",
        event: "EventBatch2",
        status: DecodeStatus::Partial,
        note: "Batch samples summarized (min/max/latest, ≤40 points / top 64 stats); no full series.",
    },
    EventCoverage {
        logger: "CsvProfiler",
        event: "RegisterCategory",
        status: DecodeStatus::Partial,
        note: "CSV profiler category catalog: index and name.",
    },
    EventCoverage {
        logger: "CsvProfiler",
        event: "DefineDeclaredStat",
        status: DecodeStatus::Partial,
        note: "CSV declared stat catalog: id, category, and name.",
    },
    EventCoverage {
        logger: "CsvProfiler",
        event: "DefineInlineStat",
        status: DecodeStatus::Partial,
        note: "CSV inline stat catalog: id, category, and name.",
    },
    EventCoverage {
        logger: "CsvProfiler",
        event: "BeginStat",
        status: DecodeStatus::Partial,
        note: "Non-exclusive CSV begin; paired with EndStat into bounded duration samples.",
    },
    EventCoverage {
        logger: "CsvProfiler",
        event: "EndStat",
        status: DecodeStatus::Partial,
        note: "Non-exclusive CSV end; unmatched ends counted.",
    },
    EventCoverage {
        logger: "CsvProfiler",
        event: "CustomStatInt",
        status: DecodeStatus::Partial,
        note: "CSV integer custom-stat samples (bounded).",
    },
    EventCoverage {
        logger: "CsvProfiler",
        event: "CustomStatFloat",
        status: DecodeStatus::Partial,
        note: "CSV float custom-stat samples (bounded).",
    },
    EventCoverage {
        logger: "TaskTrace",
        event: "Init",
        status: DecodeStatus::Partial,
        note: "TaskTrace init version.",
    },
    EventCoverage {
        logger: "TaskTrace",
        event: "Created",
        status: DecodeStatus::Partial,
        note: "Task created count.",
    },
    EventCoverage {
        logger: "TaskTrace",
        event: "Launched",
        status: DecodeStatus::Partial,
        note: "Task launched; optional debug name retained for a bounded set.",
    },
    EventCoverage {
        logger: "TaskTrace",
        event: "Scheduled",
        status: DecodeStatus::Partial,
        note: "Task scheduled count.",
    },
    EventCoverage {
        logger: "TaskTrace",
        event: "SubsequentAdded",
        status: DecodeStatus::Partial,
        note: "Subsequent dependency edge count (no full graph).",
    },
    EventCoverage {
        logger: "TaskTrace",
        event: "Started",
        status: DecodeStatus::Partial,
        note: "Task started count.",
    },
    EventCoverage {
        logger: "TaskTrace",
        event: "Finished",
        status: DecodeStatus::Partial,
        note: "Task finished count.",
    },
    EventCoverage {
        logger: "TaskTrace",
        event: "Completed",
        status: DecodeStatus::Partial,
        note: "Task completed count.",
    },
    EventCoverage {
        logger: "TaskTrace",
        event: "Destroyed",
        status: DecodeStatus::Partial,
        note: "Task destroyed count.",
    },
    EventCoverage {
        logger: "TaskTrace",
        event: "WaitingStarted",
        status: DecodeStatus::Partial,
        note: "Wait begin with optional waited-task id array.",
    },
    EventCoverage {
        logger: "TaskTrace",
        event: "WaitingFinished",
        status: DecodeStatus::Partial,
        note: "Wait end paired by thread into bounded duration samples.",
    },
    EventCoverage {
        logger: "LoadTime",
        event: "ClassInfo",
        status: DecodeStatus::Partial,
        note: "Load-time class catalog: class pointer and name.",
    },
    EventCoverage {
        logger: "LoadTime",
        event: "PackageSummary",
        status: DecodeStatus::Partial,
        note: "Async package summary: package pointer, name, header size, import/export counts, and optional priority.",
    },
    EventCoverage {
        logger: "LoadTime",
        event: "BeginRequest",
        status: DecodeStatus::Partial,
        note: "Async loading request begin cycle keyed by request id.",
    },
    EventCoverage {
        logger: "LoadTime",
        event: "EndRequest",
        status: DecodeStatus::Partial,
        note: "Async loading request end cycle paired with begin events into bounded duration samples.",
    },
    EventCoverage {
        logger: "LoadTime",
        event: "StartAsyncLoading",
        status: DecodeStatus::Partial,
        note: "Async loading start count and cycle bounds.",
    },
    EventCoverage {
        logger: "LoadTime",
        event: "SuspendAsyncLoading",
        status: DecodeStatus::Partial,
        note: "Async loading suspend count and cycle bounds.",
    },
    EventCoverage {
        logger: "LoadTime",
        event: "ResumeAsyncLoading",
        status: DecodeStatus::Partial,
        note: "Async loading resume count and cycle bounds.",
    },
    EventCoverage {
        logger: "IoStore",
        event: "BackendName",
        status: DecodeStatus::Partial,
        note: "IoStore backend handle and name catalog.",
    },
    EventCoverage {
        logger: "IoStore",
        event: "RequestCreate",
        status: DecodeStatus::Partial,
        note: "IoStore request creation metadata: request, batch, chunk, offset, and requested size.",
    },
    EventCoverage {
        logger: "IoStore",
        event: "RequestStarted",
        status: DecodeStatus::Partial,
        note: "IoStore request start cycle and backend association.",
    },
    EventCoverage {
        logger: "IoStore",
        event: "RequestCompleted",
        status: DecodeStatus::Partial,
        note: "IoStore request completion cycle and completed byte count.",
    },
    EventCoverage {
        logger: "IoStore",
        event: "RequestFailed",
        status: DecodeStatus::Partial,
        note: "IoStore failed request count and sample status.",
    },
    EventCoverage {
        logger: "IoStore",
        event: "RequestUnresolved",
        status: DecodeStatus::Partial,
        note: "IoStore unresolved request count and sample status.",
    },
    EventCoverage {
        logger: "PlatformFile",
        event: "BeginOpen",
        status: DecodeStatus::Partial,
        note: "Platform file open begin with wide path; paired by thread to EndOpen.",
    },
    EventCoverage {
        logger: "PlatformFile",
        event: "EndOpen",
        status: DecodeStatus::Partial,
        note: "Platform file open end; handle u64::MAX marks a failed open.",
    },
    EventCoverage {
        logger: "PlatformFile",
        event: "BeginReOpen",
        status: DecodeStatus::Partial,
        note: "Platform file reopen begin against an existing file handle.",
    },
    EventCoverage {
        logger: "PlatformFile",
        event: "EndReOpen",
        status: DecodeStatus::Partial,
        note: "Platform file reopen end with replacement handle.",
    },
    EventCoverage {
        logger: "PlatformFile",
        event: "BeginClose",
        status: DecodeStatus::Partial,
        note: "Platform file close begin; drops the open-handle mapping.",
    },
    EventCoverage {
        logger: "PlatformFile",
        event: "EndClose",
        status: DecodeStatus::Partial,
        note: "Platform file close end paired by thread to BeginClose.",
    },
    EventCoverage {
        logger: "PlatformFile",
        event: "BeginRead",
        status: DecodeStatus::Partial,
        note: "Platform file read begin with offset/size; paired by ReadHandle.",
    },
    EventCoverage {
        logger: "PlatformFile",
        event: "EndRead",
        status: DecodeStatus::Partial,
        note: "Platform file read end with SizeRead byte accounting.",
    },
    EventCoverage {
        logger: "PlatformFile",
        event: "BeginWrite",
        status: DecodeStatus::Partial,
        note: "Platform file write begin with offset/size; paired by WriteHandle.",
    },
    EventCoverage {
        logger: "PlatformFile",
        event: "EndWrite",
        status: DecodeStatus::Partial,
        note: "Platform file write end with SizeWritten byte accounting.",
    },
    EventCoverage {
        logger: "Memory",
        event: "Init",
        status: DecodeStatus::Partial,
        note: "Memory allocation format version and packed-size parameters.",
    },
    EventCoverage {
        logger: "Memory",
        event: "TagSpec",
        status: DecodeStatus::Partial,
        note: "Memory tag catalog with parent ids and display names.",
    },
    EventCoverage {
        logger: "Memory",
        event: "MemoryScope",
        status: DecodeStatus::Partial,
        note: "Memory scope tag counts resolved through the bounded tag catalog.",
    },
    EventCoverage {
        logger: "Memory",
        event: "Alloc",
        status: DecodeStatus::Partial,
        note: "Root-heap allocation summary with bounded outstanding-address tracking.",
    },
    EventCoverage {
        logger: "Memory",
        event: "AllocSystem",
        status: DecodeStatus::Partial,
        note: "System-root allocation summary with bounded outstanding-address tracking.",
    },
    EventCoverage {
        logger: "Memory",
        event: "AllocVideo",
        status: DecodeStatus::Partial,
        note: "Video-root allocation summary with bounded outstanding-address tracking.",
    },
    EventCoverage {
        logger: "Memory",
        event: "Free",
        status: DecodeStatus::Partial,
        note: "Allocation frees paired only against bounded tracked addresses.",
    },
    EventCoverage {
        logger: "Memory",
        event: "FreeSystem",
        status: DecodeStatus::Partial,
        note: "System-root allocation frees paired only against bounded tracked addresses.",
    },
    EventCoverage {
        logger: "Memory",
        event: "FreeVideo",
        status: DecodeStatus::Partial,
        note: "Video-root allocation frees paired only against bounded tracked addresses.",
    },
    EventCoverage {
        logger: "Memory",
        event: "ReallocAlloc",
        status: DecodeStatus::Partial,
        note: "Root-heap reallocation allocation summary.",
    },
    EventCoverage {
        logger: "Memory",
        event: "ReallocAllocSystem",
        status: DecodeStatus::Partial,
        note: "System-root reallocation allocation summary.",
    },
    EventCoverage {
        logger: "Memory",
        event: "ReallocAllocVideo",
        status: DecodeStatus::Partial,
        note: "Video-root reallocation allocation summary.",
    },
    EventCoverage {
        logger: "Memory",
        event: "ReallocFree",
        status: DecodeStatus::Partial,
        note: "Root-heap reallocation frees paired only against bounded tracked addresses.",
    },
    EventCoverage {
        logger: "Memory",
        event: "ReallocFreeSystem",
        status: DecodeStatus::Partial,
        note: "System-root reallocation frees paired only against bounded tracked addresses.",
    },
    EventCoverage {
        logger: "Memory",
        event: "ReallocFreeVideo",
        status: DecodeStatus::Partial,
        note: "Video-root reallocation frees paired only against bounded tracked addresses.",
    },
    EventCoverage {
        logger: "Memory",
        event: "CallstackSpec",
        status: DecodeStatus::Partial,
        note: "Bounded raw callstack catalog (CallstackId + u64 program counters); no module/symbol resolution.",
    },
    EventCoverage {
        logger: "LLM",
        event: "TagsSpec",
        status: DecodeStatus::Partial,
        note: "Bounded LLM tag catalog with parent and tag-set metadata.",
    },
    EventCoverage {
        logger: "LLM",
        event: "TrackerSpec",
        status: DecodeStatus::Partial,
        note: "LLM tracker catalog with names.",
    },
    EventCoverage {
        logger: "LLM",
        event: "TagSetSpec",
        status: DecodeStatus::Partial,
        note: "LLM tag-set catalog with names.",
    },
    EventCoverage {
        logger: "LLM",
        event: "TagValue",
        status: DecodeStatus::Partial,
        note: "Latest LLM tag values by tracker with bounded retained tags per sample and globally.",
    },
    EventCoverage {
        logger: "MetadataStack",
        event: "ClearScope",
        status: DecodeStatus::Partial,
        note: "Metadata stack clear-scope events counted.",
    },
    EventCoverage {
        logger: "MetadataStack",
        event: "SaveStack",
        status: DecodeStatus::Partial,
        note: "Metadata stack save ids counted.",
    },
    EventCoverage {
        logger: "MetadataStack",
        event: "RestoreStack",
        status: DecodeStatus::Partial,
        note: "Metadata stack restore ids counted and matched against observed saves.",
    },
    EventCoverage {
        logger: "SlateTrace",
        event: "AddWidget",
        status: DecodeStatus::Partial,
        note: "Slate widget add events counted by widget id with cycle bounds.",
    },
    EventCoverage {
        logger: "Trace",
        event: "ChannelAnnounce",
        status: DecodeStatus::Partial,
        note: "Trace channel catalog: id, enabled/read-only flags, and name.",
    },
    EventCoverage {
        logger: "Trace",
        event: "ChannelToggle",
        status: DecodeStatus::Partial,
        note: "Trace channel latest enabled state and toggle counts.",
    },
    EventCoverage {
        logger: "Misc",
        event: "BookmarkSpec",
        status: DecodeStatus::Partial,
        note: "Bookmark spec: point, format string, file, line.",
    },
    EventCoverage {
        logger: "Misc",
        event: "Bookmark",
        status: DecodeStatus::Partial,
        note: "Bookmark events with typed FormatArgs samples rendered against the spec format string.",
    },
    EventCoverage {
        logger: "Misc",
        event: "RegionBegin",
        status: DecodeStatus::Partial,
        note: "Named region begin paired into region totals.",
    },
    EventCoverage {
        logger: "Misc",
        event: "RegionBeginWithId",
        status: DecodeStatus::Partial,
        note: "Id-keyed region begin paired into region totals.",
    },
    EventCoverage {
        logger: "Misc",
        event: "RegionEnd",
        status: DecodeStatus::Partial,
        note: "Named region end paired into region totals.",
    },
    EventCoverage {
        logger: "Misc",
        event: "RegionEndWithId",
        status: DecodeStatus::Partial,
        note: "Id-keyed region end paired into region totals.",
    },
    EventCoverage {
        logger: "GpuProfiler",
        event: "Init",
        status: DecodeStatus::Partial,
        note: "GPU profiler version.",
    },
    EventCoverage {
        logger: "GpuProfiler",
        event: "QueueSpec",
        status: DecodeStatus::Partial,
        note: "GPU queue id and name.",
    },
    EventCoverage {
        logger: "GpuProfiler",
        event: "EventFrameBoundary",
        status: DecodeStatus::Partial,
        note: "GPU frame boundary counted per queue.",
    },
    EventCoverage {
        logger: "GpuProfiler",
        event: "EventBreadcrumbSpec",
        status: DecodeStatus::Partial,
        note: "GPU breadcrumb spec: id, name, name format.",
    },
    EventCoverage {
        logger: "GpuProfiler",
        event: "EventBeginBreadcrumb",
        status: DecodeStatus::Partial,
        note: "GPU breadcrumb begin paired into intervals with bounded CBOR metadata expansion.",
    },
    EventCoverage {
        logger: "GpuProfiler",
        event: "EventEndBreadcrumb",
        status: DecodeStatus::Partial,
        note: "GPU breadcrumb end paired into intervals; zero timestamps ignored without touching the open stack (Insights parity); negative durations counted but still closed.",
    },
    EventCoverage {
        logger: "GpuProfiler",
        event: "EventBeginWork",
        status: DecodeStatus::Partial,
        note: "GPU work begin paired into intervals; CPUTimestamp retained for submission-latency samples (not clock calibration).",
    },
    EventCoverage {
        logger: "GpuProfiler",
        event: "EventEndWork",
        status: DecodeStatus::Partial,
        note: "GPU work end paired into intervals and timestamp bounds.",
    },
    EventCoverage {
        logger: "GpuProfiler",
        event: "EventWait",
        status: DecodeStatus::Partial,
        note: "GPU wait counted per queue.",
    },
    EventCoverage {
        logger: "GpuProfiler",
        event: "EventStats",
        status: DecodeStatus::Partial,
        note: "GPU per-frame draw/primitive stats.",
    },
    EventCoverage {
        logger: "GpuProfiler",
        event: "SignalFence",
        status: DecodeStatus::Partial,
        note: "GPU fence signal counted per queue.",
    },
    EventCoverage {
        logger: "GpuProfiler",
        event: "WaitFence",
        status: DecodeStatus::Partial,
        note: "GPU fence wait counted per queue.",
    },
    EventCoverage {
        logger: "Logging",
        event: "LogCategory",
        status: DecodeStatus::Partial,
        note: "Log category: pointer, default verbosity, name.",
    },
    EventCoverage {
        logger: "Logging",
        event: "LogMessageSpec",
        status: DecodeStatus::Partial,
        note: "Log point spec: category, file, line, verbosity, format string.",
    },
    EventCoverage {
        logger: "Logging",
        event: "LogMessage",
        status: DecodeStatus::Partial,
        note: "Log messages counted per log point; FormatArgs rendered when the typed stream is present.",
    },
    EventCoverage {
        logger: "Diagnostics",
        event: "Session2",
        status: DecodeStatus::Partial,
        note: "Session identity fields; instance id emitted as raw hex.",
    },
    EventCoverage {
        logger: "Diagnostics",
        event: "ModuleInit",
        status: DecodeStatus::Partial,
        note: "Symbol format and ModuleBaseShift for address reconstruction (Insights parity).",
    },
    EventCoverage {
        logger: "Diagnostics",
        event: "ModuleLoad",
        status: DecodeStatus::Partial,
        note: "Bounded module catalog with base/size/ImageId; Windows PDB identity is Guid+Age.",
    },
    EventCoverage {
        logger: "Diagnostics",
        event: "ModuleUnload",
        status: DecodeStatus::Partial,
        note: "Module unload by reconstructed base address.",
    },
];

const CPU_NAMED_EVENT_COVERAGE_NOTE: &str =
    "Named CPU event counted with one generic decoded payload sample; no timeline reconstruction.";

fn decode_status_for(event: &EventTypeInfo) -> DecodeStatus {
    if event.logger == "Cpu" {
        return DecodeStatus::Partial;
    }
    EVENT_COVERAGE
        .iter()
        .find(|entry| entry.logger == event.logger && entry.event == event.event)
        .map_or(DecodeStatus::Raw, |entry| entry.status)
}

/// Cross-reference decode coverage ([`EVENT_COVERAGE`]) against the events a trace
/// actually declares, and optionally against the engine event `universe` (a set of
/// `"Logger.Event"` keys, e.g. harvested from `UE_TRACE_EVENT_BEGIN` declarations).
///
/// Surfaces the real gaps automatically: which declared events are still `raw`
/// (by type and by observed volume), which decoders never fired for this trace, and
/// which engine events this trace never declared.
pub fn coverage(
    source: &[u8],
    universe: Option<&std::collections::BTreeSet<String>>,
) -> Result<TraceCoverage, TraceError> {
    let inventory = inventory(source)?;
    let notes = EVENT_COVERAGE
        .iter()
        .map(|entry| ((entry.logger, entry.event), entry.note))
        .collect::<BTreeMap<_, _>>();

    let events = inventory
        .events
        .iter()
        .map(|event| CoverageEntry {
            logger: event.logger.clone(),
            event: event.event.clone(),
            uid: event.uid,
            observed_count: event.observed_count,
            status: event.decode_status,
            note: if event.logger == "Cpu" {
                Some(CPU_NAMED_EVENT_COVERAGE_NOTE)
            } else {
                notes
                    .get(&(event.logger.as_str(), event.event.as_str()))
                    .copied()
            },
        })
        .collect::<Vec<_>>();

    let declared = inventory
        .events
        .iter()
        .map(|event| (event.logger.as_str(), event.event.as_str()))
        .collect::<std::collections::BTreeSet<_>>();
    let decoders_not_observed = EVENT_COVERAGE
        .iter()
        .filter(|entry| !declared.contains(&(entry.logger, entry.event)))
        .copied()
        .collect::<Vec<_>>();

    let summary = CoverageSummary {
        declared_event_types: u64::try_from(events.len()).unwrap(),
        decoded_event_types: count_status(&events, DecodeStatus::Decoded),
        partial_event_types: count_status(&events, DecodeStatus::Partial),
        raw_event_types: count_status(&events, DecodeStatus::Raw),
        observed_events: events.iter().map(|event| event.observed_count).sum(),
        raw_observed_events: events
            .iter()
            .filter(|event| event.status == DecodeStatus::Raw)
            .map(|event| event.observed_count)
            .sum(),
    };

    let universe_coverage = universe.map(|universe| {
        let declared_keys = inventory
            .events
            .iter()
            .map(|event| format!("{}.{}", event.logger, event.event))
            .collect::<std::collections::BTreeSet<_>>();
        let unseen = universe
            .iter()
            .filter(|key| !declared_keys.contains(*key))
            .cloned()
            .collect::<Vec<_>>();
        let not_in_universe = declared_keys
            .iter()
            .filter(|key| !universe.contains(*key))
            .cloned()
            .collect::<Vec<_>>();
        UniverseCoverage {
            total: u64::try_from(universe.len()).unwrap(),
            declared_in_trace: u64::try_from(universe.len() - unseen.len()).unwrap(),
            unseen,
            not_in_universe,
        }
    });

    Ok(TraceCoverage {
        header: inventory.header,
        summary,
        events,
        decoders_not_observed,
        universe: universe_coverage,
    })
}

fn count_status(events: &[CoverageEntry], status: DecodeStatus) -> u64 {
    u64::try_from(events.iter().filter(|event| event.status == status).count()).unwrap()
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RawSample {
    thread_id: u16,
    data: Vec<u8>,
}

fn decode_event_sample(
    event: &EventTypeInfo,
    sample: &RawSample,
) -> Result<EventSample, TraceError> {
    let aux = parse_protocol5_aux(&sample.data, event_data_size(event), 0).unwrap_or_default();
    let mut fields = BTreeMap::new();
    for (index, field) in event.fields.iter().enumerate() {
        let value = if field.size == 0 {
            aux.get(&(index as u8))
                .map(|bytes| decode_variable_field(event, field, bytes))
                .transpose()?
                .unwrap_or_else(|| raw_value("missing_aux", &[]))
        } else {
            decode_fixed_sample_field(field, &sample.data)
        };
        fields.insert(field.name.clone(), value);
    }
    Ok(EventSample {
        thread_id: sample.thread_id,
        fields,
    })
}

fn decode_fixed_sample_field(field: &FieldInfo, data: &[u8]) -> SampleValue {
    let start = usize::from(field.offset);
    let end = start.saturating_add(usize::from(field.size));
    if end > data.len() {
        return raw_value("out_of_bounds", &data[start.min(data.len())..]);
    }
    let bytes = &data[start..end];
    match (field.type_name.as_str(), bytes.len()) {
        ("uint8", 1) => SampleValue::Unsigned(u64::from(bytes[0])),
        ("uint16", 2) => SampleValue::Unsigned(u64::from(u16::from_le_bytes(
            bytes.try_into().expect("length checked"),
        ))),
        ("uint32", 4) => SampleValue::Unsigned(u64::from(u32::from_le_bytes(
            bytes.try_into().expect("length checked"),
        ))),
        ("uint64", 8) => SampleValue::Unsigned(u64::from_le_bytes(
            bytes.try_into().expect("length checked"),
        )),
        ("int8", 1) => SampleValue::Signed(i64::from(i8::from_le_bytes(
            bytes.try_into().expect("length checked"),
        ))),
        ("int16", 2) => SampleValue::Signed(i64::from(i16::from_le_bytes(
            bytes.try_into().expect("length checked"),
        ))),
        ("int32", 4) => SampleValue::Signed(i64::from(i32::from_le_bytes(
            bytes.try_into().expect("length checked"),
        ))),
        ("int64", 8) => SampleValue::Signed(i64::from_le_bytes(
            bytes.try_into().expect("length checked"),
        )),
        ("float32", 4) => SampleValue::Float(f64::from(f32::from_le_bytes(
            bytes.try_into().expect("length checked"),
        ))),
        ("float64", 8) => SampleValue::Float(f64::from_le_bytes(
            bytes.try_into().expect("length checked"),
        )),
        _ => raw_value("raw", bytes),
    }
}

fn decode_variable_field(
    event: &EventTypeInfo,
    field: &FieldInfo,
    bytes: &[u8],
) -> Result<SampleValue, TraceError> {
    match field.type_name.as_str() {
        "ansi_string" => Ok(SampleValue::String(decode_ansi_bytes(bytes))),
        "wide_string" => decode_wide_bytes(bytes)
            .map(SampleValue::String)
            .map_err(|detail| {
                TraceError::new(
                    TraceErrorKind::MalformedData,
                    0,
                    format!("{}.{}", event.event, field.name),
                    detail,
                )
            }),
        "array" => Ok(raw_value("array", bytes)),
        _ => Ok(raw_value("raw", bytes)),
    }
}

fn decode_wide_bytes(bytes: &[u8]) -> Result<String, String> {
    if bytes.len() % 2 != 0 {
        return Err("wide string has an odd byte length".to_owned());
    }
    let mut words = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes(chunk.try_into().expect("chunk size checked")))
        .collect::<Vec<_>>();
    if words.last() == Some(&0) {
        words.pop();
    }
    String::from_utf16(&words).map_err(|error| format!("wide string is invalid UTF-16: {error}"))
}

fn raw_value(kind: &'static str, bytes: &[u8]) -> SampleValue {
    SampleValue::Raw(SampleRawValue {
        kind,
        byte_len: bytes.len(),
        hex_prefix: hex_prefix(bytes, 32),
    })
}

fn hex_prefix(bytes: &[u8], max_bytes: usize) -> String {
    bytes
        .iter()
        .take(max_bytes)
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join("")
}

fn known_event_name(uid: u16) -> &'static str {
    match uid {
        0 => "NewEvent",
        1 => "AuxData",
        3 => "AuxDataTerminal",
        4 => "EnterScope",
        5 => "LeaveScope",
        6 => "EnterScope_TA",
        7 => "LeaveScope_TA",
        8 => "EnterScope_TB",
        9 => "LeaveScope_TB",
        _ => "UnknownKnownEvent",
    }
}

pub(super) fn read_header(reader: &mut Reader<'_>) -> Result<TraceHeader, TraceError> {
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

/// Decompress an LZ4 block directly into `stream`, growing it in place.
/// On failure the stream length is restored so partial output is never retained.
pub(super) fn decompress_lz4_into_stream(
    stream: &mut Vec<u8>,
    compressed: &[u8],
    decoded_size: usize,
    packet_offset: u64,
) -> Result<(), TraceError> {
    if decoded_size == 0 {
        return Ok(());
    }
    let start = stream.len();
    stream.resize(start + decoded_size, 0);
    match lz4_flex::block::decompress_into(compressed, &mut stream[start..]) {
        Ok(actual) if actual == decoded_size => Ok(()),
        Ok(actual) => {
            stream.truncate(start);
            Err(TraceError::new(
                TraceErrorKind::MalformedData,
                packet_offset,
                "Packet.DecodedSize",
                format!("expected {decoded_size} decoded bytes, got {actual}"),
            ))
        }
        Err(error) => {
            stream.truncate(start);
            Err(TraceError::new(
                TraceErrorKind::MalformedData,
                packet_offset,
                "Packet.CompressedData",
                format!("LZ4 block decompression failed: {error}"),
            ))
        }
    }
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
            decompress_lz4_into_stream(
                stream,
                compressed,
                usize::from(decoded_size),
                packet_offset,
            )?;
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

pub(super) fn decode_new_event(
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

    let field_capacity = reader.checked_vec_capacity::<RawFieldInfo>(
        usize::from(field_count),
        if protocol >= 6 { 8 } else { 6 },
        "NewEvent.FieldCount",
    )?;
    let mut raw_fields = Vec::with_capacity(field_capacity);
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
    thread_groups: ThreadGroupDashboard,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct DecodedDashboardEvents {
    cpu: CpuDashboard,
    gpu: GpuDashboard,
    counters: CounterDashboard,
    stats: StatsDashboard,
    csv: CsvDashboard,
    loading: LoadingDashboard,
    io_store: IoStoreDashboard,
    platform_file: PlatformFileDashboard,
    trace_timing: TraceTimingDashboard,
    memory: MemoryDashboard,
    callstacks: CallstackDashboard,
    modules: ModuleDashboard,
    metadata_stack: MetadataStackDashboard,
    slate: SlateDashboard,
    channels: TraceChannelDashboard,
    thread_groups: ThreadGroupDashboard,
    tasks: TasksDashboard,
    annotations: AnnotationDashboard,
    logging: LogDashboard,
    unmodeled: UnmodeledTraceDashboard,
    frame_correlation: FrameCorrelationDashboard,
    frames: Vec<FrameMarker>,
    dispatch: Option<crate::utrace_dispatch::SerialDispatchSummary>,
    session: Option<SessionInfo>,
}

/// Options for dashboard decode beyond the default aggregate summary.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct DashboardOptions {
    /// When set, retain a bounded CPU timeline for this metadata frame number.
    pub timeline_frame: Option<u32>,
    /// Max intervals retained in `cpu.timeline` (default 500).
    pub timeline_limit: Option<usize>,
    /// Max rows retained in `gpu.frames` and `frame_correlation.frames` (default 120).
    pub max_frames: Option<usize>,
    /// When set, retain a bounded GPU timeline for this queue-local frame number.
    pub gpu_timeline_frame: Option<u32>,
    /// Max intervals retained in `gpu.timeline` (default 500).
    pub gpu_timeline_limit: Option<usize>,
}

pub fn dashboard(source: &[u8]) -> Result<TraceDashboard, TraceError> {
    dashboard_with_options(source, DashboardOptions::default())
}

pub fn dashboard_with_options(
    source: &[u8],
    options: DashboardOptions,
) -> Result<TraceDashboard, TraceError> {
    let mut session = crate::utrace_session::DashboardSession::new(options);
    for chunk in source.chunks(crate::utrace_session::MAX_PUSH_CHUNK_BYTES) {
        session.push_chunk(chunk)?;
    }
    session.finish()
}

/// Decode dashboard and inventory projections from one incremental packet pass.
pub fn dashboard_and_inventory_with_options(
    source: &[u8],
    options: DashboardOptions,
) -> Result<(TraceDashboard, TraceInventory), TraceError> {
    let mut session = crate::utrace_session::DashboardSession::new(options);
    for chunk in source.chunks(crate::utrace_session::MAX_PUSH_CHUNK_BYTES) {
        session.push_chunk(chunk)?;
    }
    session.finish_with_inventory()
}

pub(super) fn dashboard_from_decoded(
    header: TraceHeader,
    decoded: DecodedStreams,
    options: DashboardOptions,
) -> Result<TraceDashboard, TraceError> {
    dashboard_from_decoded_with_timeline_index(header, decoded, options, None)
        .map(|(dashboard, _)| dashboard)
}

pub(super) fn dashboard_from_decoded_with_timeline_index(
    header: TraceHeader,
    decoded: DecodedStreams,
    options: DashboardOptions,
    timeline_index_request: Option<(TimelineIndexRequest, SourceIdentity)>,
) -> Result<(TraceDashboard, Option<TimelineIndexBuild>), TraceError> {
    let (mut timeline_index_builder, timeline_index_initialization_error) =
        match timeline_index_request.as_ref() {
            Some((request, _)) => match CpuTimelineIndexBuilder::new(request.max_intervals) {
                Ok(builder) => (Some(builder), None),
                Err(error) => (None, Some(error)),
            },
            None => (None, None),
        };
    let (dashboard, timeline_index_builder, cycle_frequency) =
        dashboard_from_decoded_with_timeline_builder(
            header,
            decoded,
            options,
            timeline_index_builder.take(),
        )?;
    let timeline_index = timeline_index_request.map(|(request, source)| TimelineIndexBuild {
        output: request.output.clone(),
        result: match (timeline_index_builder, timeline_index_initialization_error) {
            (Some(builder), None) => builder.finish(&request.output, source, cycle_frequency),
            (None, Some(error)) => Err(error),
            (None, None) => Err(TimelineIndexError::ResourceLimit(
                "timeline index builder was not initialized".to_owned(),
            )),
            (Some(_), Some(_)) => unreachable!("a builder cannot both succeed and fail"),
        },
    });
    Ok((dashboard, timeline_index))
}

pub(super) fn dashboard_from_decoded_with_memory_timeline_index(
    header: TraceHeader,
    decoded: DecodedStreams,
    options: DashboardOptions,
    source: SourceIdentity,
) -> Result<
    (
        TraceDashboard,
        crate::utrace_timeline::CpuTimelineMemoryIndex,
    ),
    TraceError,
> {
    // A browser session cannot retain every decoded CPU span: a 260 MB trace
    // can contain tens of millions of scopes, which exceeds practical WASM
    // linear memory. Keep a bounded representative sample across the complete
    // capture instead of failing after its frame markers have already streamed.
    let builder = CpuTimelineIndexBuilder::new_reservoir_sample(DEFAULT_MAX_INDEXED_INTERVALS)
        .map_err(trace_error_from_timeline_index_error)?;
    let (dashboard, builder, cycle_frequency) =
        dashboard_from_decoded_with_timeline_builder(header, decoded, options, Some(builder))?;
    let index = builder
        .ok_or_else(|| {
            TraceError::new(
                TraceErrorKind::ResourceLimit,
                0,
                "TimelineIndex",
                "memory timeline builder was not retained",
            )
        })?
        .finish_in_memory(source, cycle_frequency)
        .map_err(trace_error_from_timeline_index_error)?;
    Ok((dashboard, index))
}

fn dashboard_from_decoded_with_timeline_builder(
    header: TraceHeader,
    decoded: DecodedStreams,
    options: DashboardOptions,
    mut timeline_index_builder: Option<CpuTimelineIndexBuilder>,
) -> Result<(TraceDashboard, Option<CpuTimelineIndexBuilder>, Option<u64>), TraceError> {
    let events = read_event_registry(&header, &decoded.streams)?;
    let decoded_importants = read_known_important_events(&header, &decoded.streams, &events)?;
    let cycle_frequency = decoded_importants
        .prologue
        .as_ref()
        .map(|prologue| prologue.cycle_frequency)
        .filter(|frequency| *frequency > 0);
    let mut timeline_collector = options.timeline_frame.map(|frame_number| {
        CpuTimelineCollector::new(frame_number, options.timeline_limit.unwrap_or(500))
    });
    let mut dashboard = {
        let mut cpu_timeline_sink =
            match (timeline_collector.as_mut(), timeline_index_builder.as_mut()) {
                (Some(collector), Some(index)) => Some(CpuTimelineSinks::Both(
                    CpuTimelineFanout::new(collector, index),
                )),
                (Some(collector), None) => Some(CpuTimelineSinks::Collector(collector)),
                (None, Some(index)) => Some(CpuTimelineSinks::Index(index)),
                (None, None) => None,
            };
        read_dashboard_events(
            &header,
            &decoded.streams,
            &events,
            &decoded_importants,
            decoded.summary.sync_count,
            DashboardDecodeOptions::full(options),
            cpu_timeline_sink
                .as_mut()
                .map(|sink| sink as &mut dyn CpuTimelineSink),
        )?
    };
    if let Some(collector) = timeline_collector {
        dashboard.cpu.timeline = Some(collector.into_dashboard(cycle_frequency));
    }
    let dashboard = TraceDashboard {
        header,
        prologue: decoded_importants.prologue,
        thread_info: decoded_importants.thread_info,
        cpu: dashboard.cpu,
        gpu: dashboard.gpu,
        counters: dashboard.counters,
        stats: dashboard.stats,
        csv: dashboard.csv,
        loading: dashboard.loading,
        io_store: dashboard.io_store,
        platform_file: dashboard.platform_file,
        trace_timing: dashboard.trace_timing,
        memory: dashboard.memory,
        callstacks: dashboard.callstacks,
        modules: dashboard.modules,
        metadata_stack: dashboard.metadata_stack,
        slate: dashboard.slate,
        channels: dashboard.channels,
        thread_groups: dashboard.thread_groups,
        tasks: dashboard.tasks,
        annotations: dashboard.annotations,
        logging: dashboard.logging,
        unmodeled: dashboard.unmodeled,
        frame_timing: None,
        frame_correlation: dashboard.frame_correlation,
        frames: dashboard.frames,
        dispatch: dashboard.dispatch,
        session: dashboard.session,
    };
    Ok((dashboard, timeline_index_builder, cycle_frequency))
}

fn trace_error_from_timeline_index_error(error: TimelineIndexError) -> TraceError {
    TraceError::new(
        TraceErrorKind::ResourceLimit,
        0,
        "TimelineIndex",
        error.to_string(),
    )
}

/// Build a bounded, disk-backed CPU scope index for repeated timeline queries.
///
/// The index is sorted by scope start cycle and includes a prefix end-cycle
/// accelerator, so range queries can skip unrelated regions without reparsing
/// the trace. `max_intervals` caps in-memory construction and the resulting
/// sidecar; callers must surface the returned `truncated` flag.
pub fn build_cpu_timeline_index(
    source: &[u8],
    output: &std::path::Path,
    max_intervals: usize,
) -> Result<CpuTimelineIndexInfo, TimelineIndexError> {
    build_cpu_timeline_index_with_source_identity(
        source,
        output,
        max_intervals,
        SourceIdentity::from_bytes(source),
    )
}

/// Build a CPU timeline index using an identity calculated while the source was
/// read. This avoids a second full-file fingerprint pass for streaming callers.
pub fn build_cpu_timeline_index_with_source_identity(
    source: &[u8],
    output: &std::path::Path,
    max_intervals: usize,
    source_identity: SourceIdentity,
) -> Result<CpuTimelineIndexInfo, TimelineIndexError> {
    let mut reader = Reader::new(source);
    let header = read_header(&mut reader).map_err(trace_error_to_timeline_index_error)?;
    let decoded = read_packets(&mut reader).map_err(trace_error_to_timeline_index_error)?;
    let events = read_event_registry(&header, &decoded.streams)
        .map_err(trace_error_to_timeline_index_error)?;
    let decoded_importants = read_known_important_events(&header, &decoded.streams, &events)
        .map_err(trace_error_to_timeline_index_error)?;
    let cycle_frequency = decoded_importants
        .prologue
        .as_ref()
        .map(|prologue| prologue.cycle_frequency)
        .filter(|frequency| *frequency > 0);
    let mut index = CpuTimelineIndexBuilder::new(max_intervals)?;
    let _ = read_dashboard_events(
        &header,
        &decoded.streams,
        &events,
        &decoded_importants,
        decoded.summary.sync_count,
        DashboardDecodeOptions::cpu_timeline_only(),
        Some(&mut index),
    )
    .map_err(trace_error_to_timeline_index_error)?;
    index.finish(output, source_identity, cycle_frequency)
}

/// Internal projection mode for consumers that require only CPU scope data.
/// Keeping this at the dashboard-pass boundary ensures unrelated providers do
/// not see or allocate for their events during sidecar construction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DashboardDecodeScope {
    Full,
    CpuTimelineOnly,
}

#[derive(Clone, Copy, Debug)]
struct DashboardDecodeOptions {
    dashboard: DashboardOptions,
    scope: DashboardDecodeScope,
}

impl DashboardDecodeOptions {
    const fn full(dashboard: DashboardOptions) -> Self {
        Self {
            dashboard,
            scope: DashboardDecodeScope::Full,
        }
    }

    fn cpu_timeline_only() -> Self {
        Self {
            dashboard: DashboardOptions::default(),
            scope: DashboardDecodeScope::CpuTimelineOnly,
        }
    }
}

impl DashboardDecodeScope {
    fn includes_important_event(self, event: &EventTypeInfo) -> bool {
        matches!(self, Self::Full)
            || matches!(
                (event.logger.as_str(), event.event.as_str()),
                ("CpuProfiler", "EventSpec" | "MetadataSpec")
            )
    }

    fn includes_normal_event(self, event: &EventTypeInfo) -> bool {
        matches!(self, Self::Full)
            || matches!(
                (event.logger.as_str(), event.event.as_str()),
                ("CpuProfiler", "Metadata" | "EventBatchV3")
                    | ("MetadataStack", "ClearScope" | "SaveStack" | "RestoreStack")
            )
    }
}

fn trace_error_to_timeline_index_error(error: TraceError) -> TimelineIndexError {
    TimelineIndexError::Malformed(error.to_string())
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
    let mut thread_groups = ThreadGroupState::default();

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
                    let mut info = decode_thread_info(event, data, event_offset + 4)?;
                    if !thread_groups.stack.is_empty() {
                        info.groups = thread_groups.stack.clone();
                        info.active_group = thread_groups.stack.last().cloned();
                    }
                    decoded.thread_info.push(info);
                }
                ("$Trace", "ThreadGroupBegin") => {
                    let name = decode_thread_group_begin(event, data, event_offset + 4)?;
                    thread_groups.begin(name);
                }
                ("$Trace", "ThreadGroupEnd") => {
                    thread_groups.end();
                }
                _ => {}
            }
        }
    }

    decoded.thread_info.sort_by_key(|thread| thread.thread_id);
    decoded.thread_groups = thread_groups.dashboard();
    Ok(decoded)
}

/// Hot-path route for normal-stream dashboard events. Resolved once per UID at
/// registry build time so the dispatch loop does not string-compare every event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
enum DashboardEventKind {
    Unknown = 0,
    CpuProfilerMetadata,
    CpuProfilerEventBatchV3,
    CpuProfilerEndThread,
    MiscBeginFrame,
    MiscEndFrame,
    Misc,
    Cpu,
    GpuProfiler,
    Counters,
    StatsEventBatch2,
    CsvProfilerStat,
    TaskTrace,
    LoggingLogMessage,
    LoadTime,
    IoStore,
    PlatformFile,
    TraceThreadTiming,
    MemoryScope,
    MemoryCallstackSpec,
    MemoryAlloc,
    MemoryFree,
    LlmTagValue,
    MetadataStack,
    SlateTraceAddWidget,
}

fn derive_dashboard_event_kind(logger: &str, event: &str) -> DashboardEventKind {
    match (logger, event) {
        ("CpuProfiler", "Metadata") => DashboardEventKind::CpuProfilerMetadata,
        ("CpuProfiler", "EventBatchV3") => DashboardEventKind::CpuProfilerEventBatchV3,
        ("CpuProfiler", "EndThread") => DashboardEventKind::CpuProfilerEndThread,
        ("Misc", "BeginFrame") => DashboardEventKind::MiscBeginFrame,
        ("Misc", "EndFrame") => DashboardEventKind::MiscEndFrame,
        ("Misc", _) => DashboardEventKind::Misc,
        ("Cpu", _) => DashboardEventKind::Cpu,
        ("GpuProfiler", _) => DashboardEventKind::GpuProfiler,
        ("Counters", _) => DashboardEventKind::Counters,
        ("Stats", "EventBatch2") => DashboardEventKind::StatsEventBatch2,
        ("CsvProfiler", "BeginStat" | "EndStat" | "CustomStatInt" | "CustomStatFloat") => {
            DashboardEventKind::CsvProfilerStat
        }
        ("TaskTrace", _) => DashboardEventKind::TaskTrace,
        ("Logging", "LogMessage") => DashboardEventKind::LoggingLogMessage,
        ("LoadTime", _) => DashboardEventKind::LoadTime,
        ("IoStore", _) => DashboardEventKind::IoStore,
        ("PlatformFile", _) => DashboardEventKind::PlatformFile,
        ("$Trace", "ThreadTiming") => DashboardEventKind::TraceThreadTiming,
        ("Memory", "MemoryScope") => DashboardEventKind::MemoryScope,
        ("Memory", "CallstackSpec") => DashboardEventKind::MemoryCallstackSpec,
        (
            "Memory",
            "Alloc" | "AllocSystem" | "AllocVideo" | "ReallocAlloc" | "ReallocAllocSystem"
            | "ReallocAllocVideo",
        ) => DashboardEventKind::MemoryAlloc,
        (
            "Memory",
            "Free" | "FreeSystem" | "FreeVideo" | "ReallocFree" | "ReallocFreeSystem"
            | "ReallocFreeVideo",
        ) => DashboardEventKind::MemoryFree,
        ("LLM", "TagValue") => DashboardEventKind::LlmTagValue,
        ("MetadataStack", _) => DashboardEventKind::MetadataStack,
        ("SlateTrace", "AddWidget") => DashboardEventKind::SlateTraceAddWidget,
        _ => DashboardEventKind::Unknown,
    }
}

fn dashboard_event_kinds(events: &[EventTypeInfo]) -> Vec<DashboardEventKind> {
    let max_uid = events.iter().map(|event| event.uid).max().unwrap_or(0);
    let mut kinds = vec![DashboardEventKind::Unknown; usize::from(max_uid) + 1];
    for event in events {
        kinds[usize::from(event.uid)] = derive_dashboard_event_kind(&event.logger, &event.event);
    }
    kinds
}

fn read_dashboard_events(
    header: &TraceHeader,
    streams: &BTreeMap<u16, Vec<u8>>,
    events: &[EventTypeInfo],
    importants: &DecodedImportantEvents,
    sync_count: u64,
    decode_options: DashboardDecodeOptions,
    mut cpu_timeline_sink: Option<&mut dyn CpuTimelineSink>,
) -> Result<DecodedDashboardEvents, TraceError> {
    if header.protocol < 5 {
        return Ok(DecodedDashboardEvents::default());
    }
    let options = decode_options.dashboard;
    let decode_scope = decode_options.scope;

    let registry = events
        .iter()
        .map(|event| (event.uid, event))
        .collect::<BTreeMap<_, _>>();
    let event_kinds = dashboard_event_kinds(events);
    let mut decoded = DecodedDashboardEvents::default();
    let mut spec_by_id = BTreeMap::<u32, CpuScopeSpec>::new();
    let mut metadata_spec_by_id = BTreeMap::<u32, CpuMetadataSpec>::new();
    let mut metadata_by_id = BTreeMap::<u32, CpuMetadataRecord>::new();
    let mut metadata_scope_totals = FxHashMap::<u32, (u64, u64)>::default();
    let mut metadata_interval_state = CpuMetadataIntervalState::default();
    let mut metadata_stack_contexts = FxHashMap::<u16, CpuMetadataStackRuntimeState>::default();
    let mut cpu_batch_thread_states = FxHashMap::<u16, CpuBatchThreadState>::default();
    let mut scope_totals = FxHashMap::<u32, (u64, u64)>::default();
    let mut frame_scope_totals = FxHashMap::<u32, FxHashMap<u32, (u64, u64)>>::default();
    let mut frame_cycle_bounds = FxHashMap::<u32, (u64, u64)>::default();
    let mut thread_scope_totals = FxHashMap::<u16, FxHashMap<u32, (u64, u64)>>::default();
    let mut cpu_named_events = BTreeMap::<String, CpuNamedEventState>::new();
    let mut gpu_queues = BTreeMap::<u32, GpuQueueState>::new();
    let mut gpu_breadcrumb_specs = BTreeMap::<u32, GpuBreadcrumbSpec>::new();
    let mut gpu_breadcrumb_totals = BTreeMap::<u32, GpuBreadcrumbTotal>::new();
    let mut counter_specs = BTreeMap::<u16, CounterSpec>::new();
    let mut counter_states = BTreeMap::<u16, CounterState>::new();
    let mut unresolved_counter_samples = 0_u64;
    let mut stat_specs = BTreeMap::<u32, StatSpec>::new();
    let mut csv_categories = BTreeMap::<i32, CsvCategory>::new();
    let mut csv_stats = BTreeMap::<u64, CsvStat>::new();
    let mut load_time = LoadTimeState::default();
    let mut io_store = IoStoreState::default();
    let mut platform_file = PlatformFileProvider::default();
    let mut trace_thread_timing = BTreeMap::<u16, TraceThreadTimingSummary>::new();
    let mut cpu_end_threads = Vec::<CpuEndThreadSummary>::new();
    let mut memory = MemoryProvider::default();
    let mut callstacks = CallstackProvider::default();
    let mut modules = ModuleProvider::default();
    let mut metadata_stack = MetadataStackState::default();
    let mut slate_widgets = BTreeMap::<u64, SlateWidgetState>::new();
    let mut trace_channels = BTreeMap::<u32, TraceChannelState>::new();
    let mut stats_samples = crate::utrace_stats_batch::StatsSampleProvider::default();
    let mut csv_samples = crate::utrace_csv::CsvSampleProvider::default();
    let mut tasks = crate::utrace_tasks::TaskProvider::default();
    let mut bookmark_specs = BTreeMap::<u64, BookmarkSpec>::new();
    let mut bookmark_states = BTreeMap::<u64, BookmarkState>::new();
    let mut unresolved_bookmark_events = 0_u64;
    let mut region_state = RegionState::default();
    let mut log_categories = BTreeMap::<u64, LogCategoryRec>::new();
    let mut log_message_specs = BTreeMap::<u64, LogMessageSpecRec>::new();
    let mut log_message_states = BTreeMap::<u64, LogMessageState>::new();
    let mut unresolved_log_messages = 0_u64;
    let mut unmodeled_events = FxHashMap::<u16, GenericEventState>::default();
    let mut session: Option<SessionInfo> = None;
    let mut submission_latency_samples = Vec::<GpuSubmissionLatencySample>::new();
    let mut gpu_timeline_collector = options.gpu_timeline_frame.map(|frame_number| {
        GpuTimelineCollector::new(frame_number, options.gpu_timeline_limit.unwrap_or(500))
    });
    let cycle_frequency = importants
        .prologue
        .as_ref()
        .map(|prologue| prologue.cycle_frequency)
        .filter(|frequency| *frequency > 0);
    let prologue_start_cycle = importants
        .prologue
        .as_ref()
        .map(|prologue| prologue.start_cycle);

    for thread_id in [0_u16, 1_u16] {
        let Some(stream) = streams.get(&thread_id) else {
            continue;
        };
        for raw_event in read_protocol5_important_events(stream)? {
            let Some(event) = registry.get(&raw_event.uid).copied() else {
                continue;
            };
            if !decode_scope.includes_important_event(event) {
                continue;
            }
            if decode_status_for(event) == DecodeStatus::Raw {
                unmodeled_events.entry(event.uid).or_default().record(
                    event,
                    raw_event.data,
                    thread_id,
                )?;
                continue;
            }
            match (event.logger.as_str(), event.event.as_str()) {
                ("CpuProfiler", "EventSpec") => {
                    let spec = decode_cpu_event_spec(event, raw_event.data, raw_event.offset + 4)?;
                    spec_by_id.insert(spec.id, spec);
                }
                ("CpuProfiler", "MetadataSpec") => {
                    let spec =
                        decode_cpu_metadata_spec(event, raw_event.data, raw_event.offset + 4)?;
                    metadata_spec_by_id.insert(spec.spec_id, spec);
                }
                ("Counters", "Spec") => {
                    let spec = decode_counter_spec(event, raw_event.data, raw_event.offset + 4)?;
                    counter_specs.insert(spec.id, spec);
                }
                ("Stats", "Spec") => {
                    let spec = decode_stat_spec(event, raw_event.data, raw_event.offset + 4)?;
                    stat_specs.insert(spec.id, spec);
                }
                ("CsvProfiler", "RegisterCategory") => {
                    let category =
                        decode_csv_category(event, raw_event.data, raw_event.offset + 4)?;
                    csv_categories.insert(category.index, category);
                }
                ("CsvProfiler", "DefineDeclaredStat" | "DefineInlineStat") => {
                    let stat = decode_csv_stat(event, raw_event.data, raw_event.offset + 4)?;
                    csv_stats.insert(stat.stat_id, stat);
                }
                ("LoadTime", "ClassInfo") => {
                    decode_load_time_event(
                        event,
                        raw_event.data,
                        &mut load_time,
                        raw_event.offset + 4,
                    )?;
                }
                ("LoadTime", _) => {
                    decode_load_time_event(
                        event,
                        raw_event.data,
                        &mut load_time,
                        raw_event.offset + 4,
                    )?;
                }
                ("IoStore", _) => {
                    decode_io_store_event(
                        event,
                        raw_event.data,
                        &mut io_store,
                        raw_event.offset + 4,
                    )?;
                }
                ("PlatformFile", _) => {
                    decode_platform_file_event(
                        event,
                        raw_event.data,
                        &mut platform_file,
                        thread_id,
                        raw_event.offset + 4,
                    )?;
                }
                ("$Trace", "ThreadTiming") => {
                    let timing = decode_trace_thread_timing(
                        event,
                        raw_event.data,
                        raw_event.offset + 4,
                        thread_id,
                    )?;
                    trace_thread_timing.insert(timing.thread_id, timing);
                }
                ("Trace", "ChannelAnnounce") => {
                    let announce =
                        decode_trace_channel_announce(event, raw_event.data, raw_event.offset + 4)?;
                    trace_channels
                        .entry(announce.id)
                        .or_default()
                        .announce(announce);
                }
                ("Trace", "ChannelToggle") => {
                    let toggle =
                        decode_trace_channel_toggle(event, raw_event.data, raw_event.offset + 4)?;
                    trace_channels
                        .entry(toggle.id)
                        .or_default()
                        .toggle(toggle.is_enabled);
                }
                ("$Trace", "ThreadGroupBegin") | ("$Trace", "ThreadGroupEnd") => {
                    // Handled in read_known_important_events so ThreadInfo
                    // membership stays ordered with the group stack.
                }
                ("Misc", "BookmarkSpec") => {
                    let spec = decode_bookmark_spec(event, raw_event.data, raw_event.offset + 4)?;
                    bookmark_specs.insert(spec.bookmark_point, spec);
                }
                ("Misc", "BeginFrame") => {
                    decoded.frames.push(decode_frame_marker(
                        event,
                        raw_event.data,
                        raw_event.offset + 4,
                        thread_id,
                        FrameMarkerKind::Begin,
                    )?);
                }
                ("Misc", "EndFrame") => {
                    decoded.frames.push(decode_frame_marker(
                        event,
                        raw_event.data,
                        raw_event.offset + 4,
                        thread_id,
                        FrameMarkerKind::End,
                    )?);
                }
                ("GpuProfiler", "Init") => {
                    decoded.gpu.version = Some(read_u8_field(
                        event,
                        raw_event.data,
                        "Version",
                        raw_event.offset + 4,
                    )?);
                }
                ("GpuProfiler", "QueueSpec") => {
                    let queue = decode_gpu_queue_spec(event, raw_event.data, raw_event.offset + 4)?;
                    let queue_id = queue.queue_id;
                    gpu_queues.entry(queue_id).or_default().spec = Some(queue);
                }
                ("GpuProfiler", "EventBreadcrumbSpec") => {
                    let spec =
                        decode_gpu_breadcrumb_spec(event, raw_event.data, raw_event.offset + 4)?;
                    gpu_breadcrumb_specs.insert(spec.spec_id, spec);
                }
                ("Logging", "LogCategory") => {
                    let (pointer, category) =
                        decode_log_category(event, raw_event.data, raw_event.offset + 4)?;
                    log_categories.insert(pointer, category);
                }
                ("Logging", "LogMessageSpec") => {
                    let (log_point, spec) =
                        decode_log_message_spec(event, raw_event.data, raw_event.offset + 4)?;
                    log_message_specs.insert(log_point, spec);
                }
                ("Diagnostics", "Session2") => {
                    session = Some(decode_session(event, raw_event.data, raw_event.offset + 4)?);
                }
                ("Diagnostics", "ModuleInit") => {
                    let (format, shift) =
                        decode_module_init(event, raw_event.data, raw_event.offset + 4)?;
                    modules.record_init(format, shift);
                }
                ("Diagnostics", "ModuleLoad") => {
                    let (name, base, size, image_id) =
                        decode_module_load(event, raw_event.data, raw_event.offset + 4)?;
                    modules.record_load(name, base, size, image_id)?;
                }
                ("Diagnostics", "ModuleUnload") => {
                    let base = decode_module_unload(event, raw_event.data, raw_event.offset + 4)?;
                    modules.record_unload(base)?;
                }
                ("CpuProfiler", "EndThread") => {
                    cpu_end_threads.push(decode_cpu_end_thread(
                        event,
                        raw_event.data,
                        raw_event.offset + 4,
                        thread_id,
                    )?);
                }
                ("Memory", "Init") => {
                    memory.set_init(decode_memory_init(
                        event,
                        raw_event.data,
                        raw_event.offset + 4,
                    )?);
                }
                ("Memory", "TagSpec") => {
                    memory.record_tag(decode_memory_tag(
                        event,
                        raw_event.data,
                        raw_event.offset + 4,
                    )?);
                }
                ("Memory", "CallstackSpec") => {
                    callstacks.record(decode_callstack_spec(
                        event,
                        raw_event.data,
                        raw_event.offset + 4,
                    )?);
                }
                ("Memory", "MemoryScope") => {
                    memory.record_scope(decode_memory_scope(
                        event,
                        raw_event.data,
                        raw_event.offset + 4,
                    )?);
                }
                ("LLM", "TagsSpec") => {
                    memory.record_llm_tag(decode_llm_tag(
                        event,
                        raw_event.data,
                        raw_event.offset + 4,
                    )?);
                }
                ("LLM", "TrackerSpec") => {
                    memory.record_llm_tracker(decode_llm_tracker(
                        event,
                        raw_event.data,
                        raw_event.offset + 4,
                    )?);
                }
                ("LLM", "TagSetSpec") => {
                    memory.record_llm_tag_set(decode_llm_tag_set(
                        event,
                        raw_event.data,
                        raw_event.offset + 4,
                    )?);
                }
                ("MetadataStack", "ClearScope" | "SaveStack" | "RestoreStack") => {
                    decode_metadata_stack_event(
                        event,
                        raw_event.data,
                        &mut metadata_stack,
                        raw_event.offset + 4,
                    )?;
                }
                ("SlateTrace", "AddWidget") => {
                    let widget =
                        decode_slate_add_widget(event, raw_event.data, raw_event.offset + 4)?;
                    slate_widgets
                        .entry(widget.widget_id)
                        .or_default()
                        .record(widget.cycle);
                }
                _ => {}
            }
        }
    }

    let (dispatched_events, dispatch_summary) =
        crate::utrace_dispatch::dispatch_normal_events(streams, &registry, sync_count)?;
    decoded.dispatch = Some(dispatch_summary);

    for raw_event in &dispatched_events {
        let thread_id = raw_event.thread_id;
        let Some(event) = registry.get(&raw_event.uid).copied() else {
            continue;
        };
        if !decode_scope.includes_normal_event(event) {
            continue;
        }
        if decode_status_for(event) == DecodeStatus::Raw {
            unmodeled_events.entry(event.uid).or_default().record(
                event,
                &raw_event.data,
                thread_id,
            )?;
            continue;
        }
        let kind = event_kinds
            .get(usize::from(raw_event.uid))
            .copied()
            .unwrap_or(DashboardEventKind::Unknown);
        match kind {
            DashboardEventKind::CpuProfilerMetadata => {
                let mut record = decode_cpu_metadata_record(event, &raw_event.data, 0)?;
                enrich_cpu_metadata_record(&metadata_spec_by_id, &mut record);
                metadata_by_id.insert(record.metadata_id, record);
            }
            DashboardEventKind::CpuProfilerEventBatchV3 => {
                let Some(data) = read_aux_bytes(event, &raw_event.data, "Data", 0)? else {
                    continue;
                };
                let mut batch_state = CpuBatchDecodeState {
                    batches: &mut decoded.cpu.batches,
                    scope_totals: &mut scope_totals,
                    metadata_scope_totals: &mut metadata_scope_totals,
                    metadata_interval_state: &mut metadata_interval_state,
                    metadata_stack_context: metadata_stack_contexts.entry(thread_id).or_default(),
                    thread_state: cpu_batch_thread_states.entry(thread_id).or_default(),
                    batch_base_cycle: raw_event.scope_cycle.or(prologue_start_cycle),
                    frame_scope_totals: &mut frame_scope_totals,
                    frame_cycle_bounds: &mut frame_cycle_bounds,
                    thread_scope_totals: thread_scope_totals.entry(thread_id).or_default(),
                    timeline: cpu_timeline_sink.take(),
                    thread_id,
                    cycle_frequency,
                };
                decode_cpu_batch(&data, &spec_by_id, &metadata_by_id, &mut batch_state)?;
                cpu_timeline_sink = batch_state.timeline.take();
            }
            DashboardEventKind::MiscBeginFrame => {
                decoded.frames.push(decode_frame_marker(
                    event,
                    &raw_event.data,
                    0,
                    thread_id,
                    FrameMarkerKind::Begin,
                )?);
            }
            DashboardEventKind::MiscEndFrame => {
                decoded.frames.push(decode_frame_marker(
                    event,
                    &raw_event.data,
                    0,
                    thread_id,
                    FrameMarkerKind::End,
                )?);
            }
            DashboardEventKind::Cpu => {
                cpu_named_events
                    .entry(event.event.clone())
                    .or_default()
                    .record(event, &raw_event.data, thread_id)?;
            }
            DashboardEventKind::GpuProfiler => {
                let mut gpu_state = GpuNormalEventState {
                    specs: &gpu_breadcrumb_specs,
                    queues: &mut gpu_queues,
                    breadcrumb_totals: &mut gpu_breadcrumb_totals,
                    submission_latency_samples: &mut submission_latency_samples,
                    timeline: gpu_timeline_collector.as_mut(),
                };
                decode_gpu_normal_event(event, &raw_event.data, &mut gpu_state, 0)?;
            }
            DashboardEventKind::Counters => {
                decode_counter_value(
                    event,
                    &raw_event.data,
                    &counter_specs,
                    &mut counter_states,
                    &mut unresolved_counter_samples,
                    0,
                )?;
            }
            DashboardEventKind::StatsEventBatch2 => {
                stats_samples.record_batch(event, &raw_event.data, &stat_specs, 0)?;
            }
            DashboardEventKind::CsvProfilerStat => {
                csv_samples.record_event(event, &raw_event.data, thread_id, &csv_stats, 0)?;
            }
            DashboardEventKind::TaskTrace => {
                tasks.record_event(event, &raw_event.data, thread_id, 0)?;
            }
            DashboardEventKind::Misc => {
                decode_misc_annotation_event(
                    event,
                    &raw_event.data,
                    &bookmark_specs,
                    &mut bookmark_states,
                    &mut unresolved_bookmark_events,
                    &mut region_state,
                    0,
                )?;
            }
            DashboardEventKind::LoggingLogMessage => {
                decode_log_message(
                    event,
                    &raw_event.data,
                    &log_message_specs,
                    &mut log_message_states,
                    &mut unresolved_log_messages,
                    0,
                )?;
            }
            DashboardEventKind::LoadTime => {
                decode_load_time_event(event, &raw_event.data, &mut load_time, 0)?;
            }
            DashboardEventKind::IoStore => {
                decode_io_store_event(event, &raw_event.data, &mut io_store, 0)?;
            }
            DashboardEventKind::PlatformFile => {
                decode_platform_file_event(
                    event,
                    &raw_event.data,
                    &mut platform_file,
                    thread_id,
                    0,
                )?;
            }
            DashboardEventKind::TraceThreadTiming => {
                let timing = decode_trace_thread_timing(event, &raw_event.data, 0, thread_id)?;
                trace_thread_timing.insert(timing.thread_id, timing);
            }
            DashboardEventKind::CpuProfilerEndThread => {
                cpu_end_threads.push(decode_cpu_end_thread(event, &raw_event.data, 0, thread_id)?);
            }
            DashboardEventKind::MemoryScope => {
                memory.record_scope(decode_memory_scope(event, &raw_event.data, 0)?);
            }
            DashboardEventKind::MemoryCallstackSpec => {
                callstacks.record(decode_callstack_spec(event, &raw_event.data, 0)?);
            }
            DashboardEventKind::MemoryAlloc => {
                let init = memory.init().ok_or_else(|| {
                    TraceError::new(
                        TraceErrorKind::MalformedData,
                        0,
                        "Memory.Init",
                        "allocation event appeared before required Memory.Init",
                    )
                })?;
                memory.record_allocation(decode_memory_allocation(
                    event,
                    &raw_event.data,
                    init,
                    0,
                )?);
            }
            DashboardEventKind::MemoryFree => {
                memory.record_free(decode_memory_free(event, &raw_event.data, 0)?);
            }
            DashboardEventKind::LlmTagValue => {
                let sample = decode_llm_tag_values(event, &raw_event.data, 0)?;
                memory.record_llm_tag_values(
                    sample.tracker_id,
                    sample.cycle,
                    &sample.values,
                    sample.dropped_values,
                );
            }
            DashboardEventKind::MetadataStack => {
                decode_metadata_stack_event(event, &raw_event.data, &mut metadata_stack, 0)?;
                apply_metadata_stack_event_to_cpu_context(
                    event,
                    &raw_event.data,
                    metadata_stack_contexts.entry(thread_id).or_default(),
                    0,
                )?;
            }
            DashboardEventKind::SlateTraceAddWidget => {
                let widget = decode_slate_add_widget(event, &raw_event.data, 0)?;
                slate_widgets
                    .entry(widget.widget_id)
                    .or_default()
                    .record(widget.cycle);
            }
            DashboardEventKind::Unknown => {}
        }
    }

    decoded.cpu.batches.unterminated_scopes =
        decoded.cpu.batches.unterminated_scopes.saturating_add(
            cpu_batch_thread_states
                .values()
                .map(cpu_batch_thread_state_unterminated_scopes)
                .sum::<u64>(),
        );

    decoded.cpu.specs = spec_by_id.values().cloned().collect();
    let cpu_metadata_rendered_totals = metadata_interval_state.rendered_scope_totals.clone();
    decoded.cpu.metadata = cpu_metadata_dashboard(
        &metadata_spec_by_id,
        &metadata_by_id,
        metadata_scope_totals,
        metadata_interval_state,
        decoded
            .cpu
            .batches
            .metadata_scopes
            .saturating_add(decoded.cpu.batches.restored_metadata_scopes),
    );
    decoded.cpu.scopes = scope_summaries(scope_totals, &spec_by_id, cycle_frequency);
    decoded.cpu.named_events = cpu_named_event_summaries(cpu_named_events);
    decoded.cpu.end_threads = cpu_end_threads;
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
                groups: info.map(|thread| thread.groups.clone()).unwrap_or_default(),
                active_group: info.and_then(|thread| thread.active_group.clone()),
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
    let mut gpu_frames = gpu_frame_summaries(&gpu_queues);
    decoded.frame_correlation = frame_correlation_dashboard(
        &cpu_metadata_rendered_totals,
        frame_scope_totals,
        frame_cycle_bounds,
        &spec_by_id,
        &gpu_frames,
        cycle_frequency,
        options.max_frames.unwrap_or(120),
    );
    let (total_frame_count, frames_truncated) =
        cap_gpu_frame_summaries(&mut gpu_frames, options.max_frames.unwrap_or(120));
    decoded.gpu.total_frame_count = total_frame_count;
    decoded.gpu.frames_truncated = frames_truncated;
    decoded.gpu.frames = gpu_frames;
    decoded.gpu.queues = gpu_queue_summaries(gpu_queues);
    decoded.gpu.work = gpu_work_summary(&decoded.gpu.queues);
    decoded.gpu.breadcrumbs = gpu_breadcrumb_dashboard(
        &decoded.gpu.queues,
        &gpu_breadcrumb_specs,
        gpu_breadcrumb_totals,
    );
    decoded.gpu.submission_latency = gpu_submission_latency(submission_latency_samples);
    if let Some(collector) = gpu_timeline_collector {
        decoded.gpu.timeline = Some(collector.into_dashboard());
    }
    decoded.counters = counter_dashboard(counter_specs, counter_states, unresolved_counter_samples);
    decoded.stats = stats_dashboard(stat_specs);
    stats_samples.apply_to_dashboard(&mut decoded.stats);
    decoded.csv = csv_dashboard(csv_categories, csv_stats);
    csv_samples.apply_to_dashboard(&mut decoded.csv);
    decoded.loading = load_time.dashboard();
    decoded.io_store = io_store.dashboard();
    decoded.platform_file = platform_file.dashboard();
    decoded.trace_timing = trace_timing_dashboard(trace_thread_timing);
    decoded.memory = memory.dashboard();
    decoded.metadata_stack = metadata_stack.dashboard();
    decoded.slate = slate_dashboard(slate_widgets);
    decoded.channels = trace_channel_dashboard(trace_channels);
    decoded.thread_groups = importants.thread_groups.clone();
    decoded.tasks = tasks.dashboard();
    decoded.annotations = annotation_dashboard(
        bookmark_specs,
        bookmark_states,
        unresolved_bookmark_events,
        region_state,
    );
    apply_callstack_resolutions(
        &mut decoded.memory,
        &mut decoded.annotations.bookmarks,
        &mut callstacks,
    );
    let mut symbol_cache = crate::utrace_symbols::SymbolCache::default();
    let mut mapped_frame_cache = BTreeMap::<u64, MappedCallstackFrame>::new();
    decoded.callstacks = callstacks.dashboard_mapped(|address| {
        if let Some(mapped) = mapped_frame_cache.get(&address) {
            return Some(mapped.clone());
        }
        let mapped = crate::utrace_symbols::map_frame(&modules, address, None, &mut symbol_cache);
        mapped_frame_cache.insert(address, mapped.clone());
        Some(mapped)
    });
    decoded.modules = modules.dashboard();
    decoded.logging = log_dashboard(
        log_categories,
        log_message_specs,
        log_message_states,
        unresolved_log_messages,
    );
    decoded.unmodeled = unmodeled_trace_dashboard(unmodeled_events, &registry);
    decoded.session = session;
    decoded.frames.sort_by_key(|frame| frame.cycle);
    Ok(decoded)
}

#[derive(Clone, Debug, Default, PartialEq)]
struct GpuQueueSpec {
    queue_id: u32,
    name: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct GpuBreadcrumbSpec {
    spec_id: u32,
    name: String,
    name_format: Option<String>,
    field_names_bytes: usize,
    field_names: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct GpuOpenWork {
    gpu_timestamp_top: u64,
    cpu_timestamp: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct GpuOpenBreadcrumb {
    spec_id: u32,
    gpu_timestamp_top: u64,
    metadata_bytes: usize,
    decoded_metadata_bytes: usize,
    skipped_metadata_bytes: usize,
    decode_failed: bool,
    metadata_hex_prefix: String,
    metadata_strings: Vec<String>,
    metadata_values: Vec<MetadataValue>,
    rendered_name: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct GpuBreadcrumbTotal {
    count: u64,
    total_cycles: u64,
    metadata_events: u64,
    metadata_bytes: u64,
    decoded_metadata_bytes: u64,
    undecoded_metadata_bytes: u64,
    decode_failed_events: u64,
    metadata_hex_prefix: String,
    metadata_strings: BTreeSet<String>,
    sample: Option<GpuBreadcrumbMetadataSample>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct GpuFrameState {
    boundary_count: u64,
    work_count: u64,
    work_total_cycles: u64,
    breadcrumb_count: u64,
    breadcrumb_total_cycles: u64,
    wait_count: u64,
    wait_total_cycles: u64,
    draw_count: u64,
    primitive_count: u64,
    signal_fence_count: u64,
    wait_fence_count: u64,
    min_gpu_timestamp: Option<u64>,
    max_gpu_timestamp: Option<u64>,
    breadcrumb_totals: BTreeMap<String, (u64, u64)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct GpuQueueState {
    spec: Option<GpuQueueSpec>,
    open_work: Vec<GpuOpenWork>,
    open_breadcrumbs: Vec<GpuOpenBreadcrumb>,
    current_frame: Option<u32>,
    frames: BTreeMap<u32, GpuFrameState>,
    work_count: u64,
    work_total_cycles: u64,
    min_gpu_timestamp: Option<u64>,
    max_gpu_timestamp: Option<u64>,
    first_cpu_timestamp: Option<u64>,
    last_cpu_timestamp: Option<u64>,
    wait_count: u64,
    wait_total_cycles: u64,
    frame_boundary_count: u64,
    last_frame_number: Option<u32>,
    draw_count: u64,
    primitive_count: u64,
    signal_fence_count: u64,
    wait_fence_count: u64,
    breadcrumb_count: u64,
    breadcrumb_total_cycles: u64,
    breadcrumb_metadata_count: u64,
    breadcrumb_metadata_bytes: u64,
    breadcrumb_decoded_metadata_bytes: u64,
    breadcrumb_undecoded_metadata_bytes: u64,
    breadcrumb_decode_failed_events: u64,
    breadcrumb_metadata_hex_prefix: String,
    breadcrumb_metadata_strings: BTreeSet<String>,
    unmatched_breadcrumb_ends: u64,
    negative_breadcrumb_durations: u64,
    unmatched_work_ends: u64,
    negative_work_durations: u64,
}

fn decode_gpu_queue_spec(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
) -> Result<GpuQueueSpec, TraceError> {
    let aux = parse_protocol5_aux(data, event_data_size(event), base_offset)?;
    let name = optional_aux_text(event, &aux, "TypeString")?.filter(|name| !name.is_empty());
    Ok(GpuQueueSpec {
        queue_id: read_u32_field(event, data, "QueueId", base_offset)?,
        name,
    })
}

fn decode_gpu_breadcrumb_spec(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
) -> Result<GpuBreadcrumbSpec, TraceError> {
    let aux = parse_protocol5_aux(data, event_data_size(event), base_offset)?;
    let spec_id = read_u32_field(event, data, "SpecId", base_offset)?;
    let static_name = optional_aux_text(event, &aux, "StaticName")?.unwrap_or_default();
    let name_format = optional_aux_text(event, &aux, "NameFormat")?.unwrap_or_default();
    let field_names_bytes = aux_bytes_len(event, &aux, "FieldNames");
    let field_names = read_aux_bytes(event, data, "FieldNames", base_offset)?
        .map(|bytes| decode_metadata_field_names(&bytes))
        .unwrap_or_default();
    let (name, name_format) = normalize_gpu_breadcrumb_name(static_name, name_format);
    Ok(GpuBreadcrumbSpec {
        spec_id,
        name,
        name_format,
        field_names_bytes,
        field_names,
    })
}

fn normalize_gpu_breadcrumb_name(
    mut name: String,
    mut name_format: String,
) -> (String, Option<String>) {
    if name == name_format {
        name.clear();
    }

    let common_prefix = name
        .chars()
        .zip(name_format.chars())
        .take_while(|(left, right)| left == right)
        .count();
    if common_prefix > 1 {
        name_format = name_format.chars().skip(common_prefix).collect();
    }

    if name.is_empty() {
        if let Some(index) = name_format.find(['%', ' ', '(', '=']) {
            name = name_format[..index].to_owned();
            name_format = name_format[index..].to_owned();
        } else if !name_format.is_empty() {
            name = std::mem::take(&mut name_format);
        } else {
            name = "Unknown".to_owned();
        }
    }

    let name_format = if name_format.is_empty() {
        None
    } else {
        Some(name_format)
    };
    (name, name_format)
}

struct GpuNormalEventState<'a> {
    specs: &'a BTreeMap<u32, GpuBreadcrumbSpec>,
    queues: &'a mut BTreeMap<u32, GpuQueueState>,
    breadcrumb_totals: &'a mut BTreeMap<u32, GpuBreadcrumbTotal>,
    submission_latency_samples: &'a mut Vec<GpuSubmissionLatencySample>,
    timeline: Option<&'a mut GpuTimelineCollector>,
}

fn decode_gpu_normal_event(
    event: &EventTypeInfo,
    data: &[u8],
    state: &mut GpuNormalEventState<'_>,
    base_offset: u64,
) -> Result<(), TraceError> {
    let specs = state.specs;
    let queues = &mut *state.queues;
    let breadcrumb_totals = &mut *state.breadcrumb_totals;
    let submission_latency_samples = &mut *state.submission_latency_samples;
    let mut timeline = state.timeline.as_deref_mut();
    match event.event.as_str() {
        "EventFrameBoundary" => {
            let queue_id = read_u32_field(event, data, "QueueId", base_offset)?;
            let frame_number = read_u32_field(event, data, "FrameNumber", base_offset)?;
            let queue = queues.entry(queue_id).or_default();
            queue.current_frame = Some(frame_number);
            queue.frame_boundary_count += 1;
            queue.last_frame_number = Some(frame_number);
            queue.frames.entry(frame_number).or_default().boundary_count += 1;
        }
        "EventBeginBreadcrumb" => {
            let spec_id = read_u32_field(event, data, "SpecId", base_offset)?;
            let queue_id = read_u32_field(event, data, "QueueId", base_offset)?;
            let gpu_timestamp_top = read_u64_field(event, data, "GPUTimestampTOP", base_offset)?;
            // Insights ignores events whose timestamp could not be determined.
            if gpu_timestamp_top == 0 {
                return Ok(());
            }
            let metadata = read_aux_bytes(event, data, "Metadata", base_offset)?;
            let metadata_bytes = metadata.as_ref().map_or(0, Vec::len);
            let metadata_report = metadata
                .as_ref()
                .map(|bytes| decode_cbor_report(bytes))
                .unwrap_or_default();
            let metadata_hex_prefix = metadata
                .as_ref()
                .map(|bytes| hex_prefix(bytes, 32))
                .unwrap_or_default();
            let mut metadata_strings = metadata_report
                .values
                .iter()
                .flat_map(metadata_value_strings)
                .collect::<Vec<_>>();
            metadata_strings.sort();
            metadata_strings.dedup();
            let spec = specs.get(&spec_id);
            let rendered_name = spec.and_then(|spec| {
                render_metadata_name_parts(
                    &spec.name,
                    spec.name_format.as_deref(),
                    &metadata_report.values,
                )
            });
            let queue = queues.entry(queue_id).or_default();
            queue.open_breadcrumbs.push(GpuOpenBreadcrumb {
                spec_id,
                gpu_timestamp_top,
                metadata_bytes,
                decoded_metadata_bytes: metadata_report.consumed_bytes,
                skipped_metadata_bytes: metadata_report.skipped_bytes,
                decode_failed: metadata_report.failed_reads > 0,
                metadata_hex_prefix,
                metadata_strings,
                metadata_values: metadata_report.values,
                rendered_name,
            });
            update_min_max(
                &mut queue.min_gpu_timestamp,
                &mut queue.max_gpu_timestamp,
                gpu_timestamp_top,
            );
        }
        "EventEndBreadcrumb" => {
            let queue_id = read_u32_field(event, data, "QueueId", base_offset)?;
            let gpu_timestamp_bop = read_u64_field(event, data, "GPUTimestampBOP", base_offset)?;
            let queue = queues.entry(queue_id).or_default();
            // Insights ignores events whose timestamp could not be determined
            // before touching the open stack, leaving the begin unterminated.
            if gpu_timestamp_bop == 0 {
                return Ok(());
            }
            update_min_max(
                &mut queue.min_gpu_timestamp,
                &mut queue.max_gpu_timestamp,
                gpu_timestamp_bop,
            );
            let Some(begin) = queue.open_breadcrumbs.pop() else {
                queue.unmatched_breadcrumb_ends += 1;
                return Ok(());
            };
            if gpu_timestamp_bop < begin.gpu_timestamp_top {
                queue.negative_breadcrumb_durations += 1;
                // Insights still closes the interval; count the anomaly but do not
                // leave the begin open (which would inflate unterminated counts).
                let duration = 0;
                queue.breadcrumb_count += 1;
                record_gpu_frame_breadcrumb(
                    queue,
                    begin.gpu_timestamp_top,
                    gpu_timestamp_bop,
                    duration,
                    begin.rendered_name.as_deref(),
                );
                if let Some(timeline) = timeline.as_mut() {
                    let name = begin
                        .rendered_name
                        .clone()
                        .or_else(|| specs.get(&begin.spec_id).map(|spec| spec.name.clone()))
                        .unwrap_or_else(|| format!("#{}", begin.spec_id));
                    timeline.record(
                        queue.current_frame,
                        queue_id,
                        GpuTimelineIntervalKind::Breadcrumb,
                        Some(begin.spec_id),
                        name,
                        begin.gpu_timestamp_top,
                        gpu_timestamp_bop,
                        duration,
                    );
                }
                let total = breadcrumb_totals.entry(begin.spec_id).or_default();
                total.count += 1;
                if begin.metadata_bytes > 0 {
                    total.metadata_events += 1;
                    total.metadata_bytes = total
                        .metadata_bytes
                        .saturating_add(u64::try_from(begin.metadata_bytes).unwrap());
                }
                return Ok(());
            }
            let duration = gpu_timestamp_bop - begin.gpu_timestamp_top;
            queue.breadcrumb_count += 1;
            queue.breadcrumb_total_cycles = queue.breadcrumb_total_cycles.saturating_add(duration);
            record_gpu_frame_breadcrumb(
                queue,
                begin.gpu_timestamp_top,
                gpu_timestamp_bop,
                duration,
                begin.rendered_name.as_deref(),
            );
            if let Some(timeline) = timeline.as_mut() {
                let name = begin
                    .rendered_name
                    .clone()
                    .or_else(|| specs.get(&begin.spec_id).map(|spec| spec.name.clone()))
                    .unwrap_or_else(|| format!("#{}", begin.spec_id));
                timeline.record(
                    queue.current_frame,
                    queue_id,
                    GpuTimelineIntervalKind::Breadcrumb,
                    Some(begin.spec_id),
                    name,
                    begin.gpu_timestamp_top,
                    gpu_timestamp_bop,
                    duration,
                );
            }
            if begin.metadata_bytes > 0 {
                queue.breadcrumb_metadata_count += 1;
                queue.breadcrumb_metadata_bytes = queue
                    .breadcrumb_metadata_bytes
                    .saturating_add(u64::try_from(begin.metadata_bytes).unwrap());
                queue.breadcrumb_decoded_metadata_bytes = queue
                    .breadcrumb_decoded_metadata_bytes
                    .saturating_add(u64::try_from(begin.decoded_metadata_bytes).unwrap());
                queue.breadcrumb_undecoded_metadata_bytes = queue
                    .breadcrumb_undecoded_metadata_bytes
                    .saturating_add(u64::try_from(begin.skipped_metadata_bytes).unwrap());
                if begin.decode_failed {
                    queue.breadcrumb_decode_failed_events += 1;
                }
                if queue.breadcrumb_metadata_hex_prefix.is_empty() {
                    queue.breadcrumb_metadata_hex_prefix = begin.metadata_hex_prefix.clone();
                }
                queue
                    .breadcrumb_metadata_strings
                    .extend(begin.metadata_strings.iter().cloned());
            }
            let total = breadcrumb_totals.entry(begin.spec_id).or_default();
            total.count += 1;
            total.total_cycles = total.total_cycles.saturating_add(duration);
            if begin.metadata_bytes > 0 {
                total.metadata_events += 1;
                total.metadata_bytes = total
                    .metadata_bytes
                    .saturating_add(u64::try_from(begin.metadata_bytes).unwrap());
                total.decoded_metadata_bytes = total
                    .decoded_metadata_bytes
                    .saturating_add(u64::try_from(begin.decoded_metadata_bytes).unwrap());
                total.undecoded_metadata_bytes = total
                    .undecoded_metadata_bytes
                    .saturating_add(u64::try_from(begin.skipped_metadata_bytes).unwrap());
                if begin.decode_failed {
                    total.decode_failed_events += 1;
                }
                if total.metadata_hex_prefix.is_empty() {
                    total.metadata_hex_prefix = begin.metadata_hex_prefix;
                }
                total.metadata_strings.extend(begin.metadata_strings);
                if total.sample.is_none() {
                    if let Some(spec) = specs.get(&begin.spec_id) {
                        total.sample = Some(GpuBreadcrumbMetadataSample {
                            spec_id: begin.spec_id,
                            name: spec.name.clone(),
                            rendered_name: begin.rendered_name,
                            fields: metadata_sample_fields(
                                &spec.field_names,
                                &begin.metadata_values,
                            ),
                        });
                    }
                }
            }
        }
        "EventBeginWork" => {
            let queue_id = read_u32_field(event, data, "QueueId", base_offset)?;
            let gpu_timestamp_top = read_u64_field(event, data, "GPUTimestampTOP", base_offset)?;
            let cpu_timestamp = read_u64_field(event, data, "CPUTimestamp", base_offset)?;
            let queue = queues.entry(queue_id).or_default();
            queue.open_work.push(GpuOpenWork {
                gpu_timestamp_top,
                cpu_timestamp,
            });
            if gpu_timestamp_top != 0 && cpu_timestamp != 0 && submission_latency_samples.len() < 64
            {
                let delay_cycles = submission_delay_cycles(gpu_timestamp_top, cpu_timestamp);
                submission_latency_samples.push(GpuSubmissionLatencySample {
                    queue_id,
                    gpu_timestamp_top,
                    cpu_submit_timestamp: cpu_timestamp,
                    delay_cycles,
                });
            }
            update_min_max(
                &mut queue.min_gpu_timestamp,
                &mut queue.max_gpu_timestamp,
                gpu_timestamp_top,
            );
            update_first_last(
                &mut queue.first_cpu_timestamp,
                &mut queue.last_cpu_timestamp,
                cpu_timestamp,
            );
        }
        "EventEndWork" => {
            let queue_id = read_u32_field(event, data, "QueueId", base_offset)?;
            let gpu_timestamp_bop = read_u64_field(event, data, "GPUTimestampBOP", base_offset)?;
            let queue = queues.entry(queue_id).or_default();
            update_min_max(
                &mut queue.min_gpu_timestamp,
                &mut queue.max_gpu_timestamp,
                gpu_timestamp_bop,
            );
            let Some(begin) = queue.open_work.pop() else {
                queue.unmatched_work_ends += 1;
                return Ok(());
            };
            if gpu_timestamp_bop < begin.gpu_timestamp_top {
                queue.negative_work_durations += 1;
                return Ok(());
            }
            queue.work_count += 1;
            let duration = gpu_timestamp_bop - begin.gpu_timestamp_top;
            queue.work_total_cycles = queue.work_total_cycles.saturating_add(duration);
            record_gpu_frame_work(queue, begin.gpu_timestamp_top, gpu_timestamp_bop, duration);
            if let Some(timeline) = timeline.as_mut() {
                timeline.record(
                    queue.current_frame,
                    queue_id,
                    GpuTimelineIntervalKind::Work,
                    None,
                    "Work".to_owned(),
                    begin.gpu_timestamp_top,
                    gpu_timestamp_bop,
                    duration,
                );
            }
        }
        "EventWait" => {
            let queue_id = read_u32_field(event, data, "QueueId", base_offset)?;
            let start_time = read_u64_field(event, data, "StartTime", base_offset)?;
            let end_time = read_u64_field(event, data, "EndTime", base_offset)?;
            let queue = queues.entry(queue_id).or_default();
            update_min_max(
                &mut queue.min_gpu_timestamp,
                &mut queue.max_gpu_timestamp,
                start_time,
            );
            update_min_max(
                &mut queue.min_gpu_timestamp,
                &mut queue.max_gpu_timestamp,
                end_time,
            );
            if end_time >= start_time {
                queue.wait_count += 1;
                let duration = end_time - start_time;
                queue.wait_total_cycles = queue.wait_total_cycles.saturating_add(duration);
                record_gpu_frame_wait(queue, start_time, end_time, duration);
            }
        }
        "EventStats" => {
            let queue_id = read_u32_field(event, data, "QueueId", base_offset)?;
            let queue = queues.entry(queue_id).or_default();
            let draw_count = u64::from(read_u32_field(event, data, "NumDraws", base_offset)?);
            let primitive_count =
                u64::from(read_u32_field(event, data, "NumPrimitives", base_offset)?);
            queue.draw_count = queue.draw_count.saturating_add(draw_count);
            queue.primitive_count = queue.primitive_count.saturating_add(primitive_count);
            record_gpu_frame_stats(queue, draw_count, primitive_count);
        }
        "SignalFence" => {
            let queue_id = read_u32_field(event, data, "QueueId", base_offset)?;
            let cpu_timestamp = read_u64_field(event, data, "CPUTimestamp", base_offset)?;
            let queue = queues.entry(queue_id).or_default();
            queue.signal_fence_count += 1;
            record_gpu_frame_signal_fence(queue);
            update_first_last(
                &mut queue.first_cpu_timestamp,
                &mut queue.last_cpu_timestamp,
                cpu_timestamp,
            );
        }
        "WaitFence" => {
            let queue_id = read_u32_field(event, data, "QueueId", base_offset)?;
            let cpu_timestamp = read_u64_field(event, data, "CPUTimestamp", base_offset)?;
            let queue = queues.entry(queue_id).or_default();
            queue.wait_fence_count += 1;
            record_gpu_frame_wait_fence(queue);
            update_first_last(
                &mut queue.first_cpu_timestamp,
                &mut queue.last_cpu_timestamp,
                cpu_timestamp,
            );
        }
        _ => {}
    }
    Ok(())
}

fn current_gpu_frame(queue: &mut GpuQueueState) -> Option<&mut GpuFrameState> {
    let frame_number = queue.current_frame?;
    Some(queue.frames.entry(frame_number).or_default())
}

fn record_gpu_frame_work(queue: &mut GpuQueueState, start: u64, end: u64, duration: u64) {
    if let Some(frame) = current_gpu_frame(queue) {
        frame.work_count += 1;
        frame.work_total_cycles = frame.work_total_cycles.saturating_add(duration);
        update_min_max(
            &mut frame.min_gpu_timestamp,
            &mut frame.max_gpu_timestamp,
            start,
        );
        update_min_max(
            &mut frame.min_gpu_timestamp,
            &mut frame.max_gpu_timestamp,
            end,
        );
    }
}

fn record_gpu_frame_breadcrumb(
    queue: &mut GpuQueueState,
    start: u64,
    end: u64,
    duration: u64,
    rendered_name: Option<&str>,
) {
    if let Some(frame) = current_gpu_frame(queue) {
        frame.breadcrumb_count += 1;
        frame.breadcrumb_total_cycles = frame.breadcrumb_total_cycles.saturating_add(duration);
        if let Some(rendered_name) = rendered_name.filter(|name| !name.is_empty()) {
            let total = frame
                .breadcrumb_totals
                .entry(rendered_name.to_owned())
                .or_insert((0, 0));
            total.0 += 1;
            total.1 = total.1.saturating_add(duration);
        }
        update_min_max(
            &mut frame.min_gpu_timestamp,
            &mut frame.max_gpu_timestamp,
            start,
        );
        update_min_max(
            &mut frame.min_gpu_timestamp,
            &mut frame.max_gpu_timestamp,
            end,
        );
    }
}

fn record_gpu_frame_wait(queue: &mut GpuQueueState, start: u64, end: u64, duration: u64) {
    if let Some(frame) = current_gpu_frame(queue) {
        frame.wait_count += 1;
        frame.wait_total_cycles = frame.wait_total_cycles.saturating_add(duration);
        update_min_max(
            &mut frame.min_gpu_timestamp,
            &mut frame.max_gpu_timestamp,
            start,
        );
        update_min_max(
            &mut frame.min_gpu_timestamp,
            &mut frame.max_gpu_timestamp,
            end,
        );
    }
}

fn record_gpu_frame_stats(queue: &mut GpuQueueState, draw_count: u64, primitive_count: u64) {
    if let Some(frame) = current_gpu_frame(queue) {
        frame.draw_count = frame.draw_count.saturating_add(draw_count);
        frame.primitive_count = frame.primitive_count.saturating_add(primitive_count);
    }
}

fn record_gpu_frame_signal_fence(queue: &mut GpuQueueState) {
    if let Some(frame) = current_gpu_frame(queue) {
        frame.signal_fence_count += 1;
    }
}

fn record_gpu_frame_wait_fence(queue: &mut GpuQueueState) {
    if let Some(frame) = current_gpu_frame(queue) {
        frame.wait_fence_count += 1;
    }
}

fn gpu_frame_summaries(queues: &BTreeMap<u32, GpuQueueState>) -> Vec<GpuFrameSummary> {
    let mut summaries = queues
        .iter()
        .flat_map(|(&queue_id, queue)| {
            queue
                .frames
                .iter()
                .map(move |(&frame_number, frame)| GpuFrameSummary {
                    queue_id,
                    frame_number,
                    boundary_count: frame.boundary_count,
                    work_count: frame.work_count,
                    work_total_cycles: frame.work_total_cycles,
                    breadcrumb_count: frame.breadcrumb_count,
                    breadcrumb_total_cycles: frame.breadcrumb_total_cycles,
                    wait_count: frame.wait_count,
                    wait_total_cycles: frame.wait_total_cycles,
                    draw_count: frame.draw_count,
                    primitive_count: frame.primitive_count,
                    signal_fence_count: frame.signal_fence_count,
                    wait_fence_count: frame.wait_fence_count,
                    min_gpu_timestamp: frame.min_gpu_timestamp,
                    max_gpu_timestamp: frame.max_gpu_timestamp,
                    top_breadcrumbs: gpu_frame_top_breadcrumbs(&frame.breadcrumb_totals),
                })
        })
        .collect::<Vec<_>>();
    summaries.sort_by(|left, right| {
        left.queue_id
            .cmp(&right.queue_id)
            .then_with(|| left.frame_number.cmp(&right.frame_number))
    });
    summaries
}

fn cap_gpu_frame_summaries(summaries: &mut Vec<GpuFrameSummary>, max_frames: usize) -> (u64, bool) {
    let total_frame_count = u64::try_from(summaries.len()).unwrap_or(u64::MAX);
    let truncated = summaries.len() > max_frames;
    summaries.truncate(max_frames);
    (total_frame_count, truncated)
}

fn gpu_frame_top_breadcrumbs(
    totals: &BTreeMap<String, (u64, u64)>,
) -> Vec<GpuFrameBreadcrumbSummary> {
    let mut summaries = totals
        .iter()
        .map(|(name, &(count, total_cycles))| GpuFrameBreadcrumbSummary {
            name: name.clone(),
            count,
            total_cycles,
        })
        .collect::<Vec<_>>();
    summaries.sort_by(|left, right| {
        right
            .total_cycles
            .cmp(&left.total_cycles)
            .then_with(|| right.count.cmp(&left.count))
            .then_with(|| left.name.cmp(&right.name))
    });
    summaries.truncate(8);
    summaries
}

fn gpu_queue_summaries(queues: BTreeMap<u32, GpuQueueState>) -> Vec<GpuQueueSummary> {
    let mut summaries = queues
        .into_iter()
        .map(|(queue_id, state)| {
            let spec = state.spec.unwrap_or(GpuQueueSpec {
                queue_id,
                name: None,
            });
            let queue_id = spec.queue_id;
            GpuQueueSummary {
                queue_id,
                gpu: ((queue_id >> 8) & 0xff) as u8,
                index: ((queue_id >> 16) & 0xff) as u8,
                queue_type: (queue_id & 0xff) as u8,
                name: spec.name,
                work_count: state.work_count,
                work_total_cycles: state.work_total_cycles,
                min_gpu_timestamp: state.min_gpu_timestamp,
                max_gpu_timestamp: state.max_gpu_timestamp,
                first_cpu_timestamp: state.first_cpu_timestamp,
                last_cpu_timestamp: state.last_cpu_timestamp,
                wait_count: state.wait_count,
                wait_total_cycles: state.wait_total_cycles,
                frame_boundary_count: state.frame_boundary_count,
                last_frame_number: state.last_frame_number,
                draw_count: state.draw_count,
                primitive_count: state.primitive_count,
                signal_fence_count: state.signal_fence_count,
                wait_fence_count: state.wait_fence_count,
                breadcrumb_count: state.breadcrumb_count,
                breadcrumb_total_cycles: state.breadcrumb_total_cycles,
                breadcrumb_metadata_count: state.breadcrumb_metadata_count,
                breadcrumb_metadata_bytes: state.breadcrumb_metadata_bytes,
                breadcrumb_metadata_hex_prefix: state.breadcrumb_metadata_hex_prefix,
                breadcrumb_metadata_strings: state
                    .breadcrumb_metadata_strings
                    .into_iter()
                    .collect(),
                unmatched_breadcrumb_ends: state.unmatched_breadcrumb_ends,
                negative_breadcrumb_durations: state.negative_breadcrumb_durations,
                unterminated_breadcrumbs: u64::try_from(state.open_breadcrumbs.len()).unwrap(),
                unmatched_work_ends: state.unmatched_work_ends,
                negative_work_durations: state.negative_work_durations,
                unterminated_work: u64::try_from(state.open_work.len()).unwrap(),
            }
        })
        .collect::<Vec<_>>();
    summaries.sort_by(|left, right| {
        right
            .work_total_cycles
            .cmp(&left.work_total_cycles)
            .then_with(|| left.queue_id.cmp(&right.queue_id))
    });
    summaries
}

fn gpu_breadcrumb_dashboard(
    queues: &[GpuQueueSummary],
    specs: &BTreeMap<u32, GpuBreadcrumbSpec>,
    totals: BTreeMap<u32, GpuBreadcrumbTotal>,
) -> GpuBreadcrumbDashboard {
    let mut top = totals
        .iter()
        .map(|(&spec_id, total)| {
            let name = specs
                .get(&spec_id)
                .map(|spec| spec.name.clone())
                .unwrap_or_else(|| format!("#{spec_id}"));
            GpuBreadcrumbSummary {
                spec_id,
                name,
                count: total.count,
                total_cycles: total.total_cycles,
                metadata_events: total.metadata_events,
                metadata_bytes: total.metadata_bytes,
            }
        })
        .collect::<Vec<_>>();
    top.sort_by(|left, right| {
        right
            .total_cycles
            .cmp(&left.total_cycles)
            .then_with(|| right.count.cmp(&left.count))
            .then_with(|| left.spec_id.cmp(&right.spec_id))
    });
    top.truncate(40);

    let mut field_names = specs
        .values()
        .flat_map(|spec| spec.field_names.iter().cloned())
        .collect::<Vec<_>>();
    field_names.sort();
    field_names.dedup();
    let metadata_bytes = queues
        .iter()
        .map(|queue| queue.breadcrumb_metadata_bytes)
        .sum();
    let metadata_hex_prefix = queues
        .iter()
        .find_map(|queue| {
            (!queue.breadcrumb_metadata_hex_prefix.is_empty())
                .then(|| queue.breadcrumb_metadata_hex_prefix.clone())
        })
        .unwrap_or_default();
    let metadata_strings = queues
        .iter()
        .flat_map(|queue| queue.breadcrumb_metadata_strings.iter().cloned())
        .collect::<Vec<_>>();
    let metadata_samples = totals
        .values()
        .filter_map(|total| total.sample.clone())
        .take(40)
        .collect::<Vec<_>>();

    GpuBreadcrumbDashboard {
        specs: u64::try_from(specs.len()).unwrap(),
        specs_with_name_format: u64::try_from(
            specs
                .values()
                .filter(|spec| spec.name_format.is_some())
                .count(),
        )
        .unwrap(),
        field_names_bytes: specs
            .values()
            .map(|spec| u64::try_from(spec.field_names_bytes).unwrap())
            .sum(),
        field_names,
        intervals: queues.iter().map(|queue| queue.breadcrumb_count).sum(),
        total_cycles: queues
            .iter()
            .map(|queue| queue.breadcrumb_total_cycles)
            .sum(),
        metadata_events: queues
            .iter()
            .map(|queue| queue.breadcrumb_metadata_count)
            .sum(),
        metadata_bytes,
        metadata_hex_prefix,
        metadata_strings,
        decoded_metadata_bytes: totals
            .values()
            .map(|total| total.decoded_metadata_bytes)
            .sum(),
        undecoded_metadata_bytes: totals
            .values()
            .map(|total| total.undecoded_metadata_bytes)
            .sum(),
        decode_failed_events: totals
            .values()
            .map(|total| total.decode_failed_events)
            .sum(),
        metadata_samples,
        unmatched_ends: queues
            .iter()
            .map(|queue| queue.unmatched_breadcrumb_ends)
            .sum(),
        negative_durations: queues
            .iter()
            .map(|queue| queue.negative_breadcrumb_durations)
            .sum(),
        unterminated_scopes: queues
            .iter()
            .map(|queue| queue.unterminated_breadcrumbs)
            .sum(),
        top,
    }
}

fn gpu_work_summary(queues: &[GpuQueueSummary]) -> GpuWorkSummary {
    GpuWorkSummary {
        queues: u64::try_from(queues.len()).unwrap(),
        intervals: queues.iter().map(|queue| queue.work_count).sum(),
        total_cycles: queues.iter().map(|queue| queue.work_total_cycles).sum(),
        unmatched_ends: queues.iter().map(|queue| queue.unmatched_work_ends).sum(),
        negative_durations: queues
            .iter()
            .map(|queue| queue.negative_work_durations)
            .sum(),
        unterminated_scopes: queues.iter().map(|queue| queue.unterminated_work).sum(),
    }
}

fn frame_correlation_dashboard(
    cpu_metadata_scope_totals: &BTreeMap<(u32, String), (u64, u64)>,
    frame_scope_totals: FxHashMap<u32, FxHashMap<u32, (u64, u64)>>,
    frame_cycle_bounds: FxHashMap<u32, (u64, u64)>,
    specs: &BTreeMap<u32, CpuScopeSpec>,
    gpu_frames: &[GpuFrameSummary],
    cycle_frequency: Option<u64>,
    max_frames: usize,
) -> FrameCorrelationDashboard {
    let mut frames = BTreeMap::<u32, CorrelatedFrameSummary>::new();
    for ((_, rendered_name), &(count, total_cycles)) in cpu_metadata_scope_totals {
        let Some(frame_number) = parse_rendered_frame_number(rendered_name) else {
            continue;
        };
        let frame = frames
            .entry(frame_number)
            .or_insert_with(|| CorrelatedFrameSummary {
                frame_number,
                cpu_metadata_count: 0,
                cpu_metadata_cycles: 0,
                cpu_metadata_seconds: None,
                cpu_begin_cycle: None,
                cpu_end_cycle: None,
                top_cpu_scopes: Vec::new(),
                gpu_queue_count: 0,
                gpu_work_count: 0,
                gpu_work_cycles: 0,
                gpu_breadcrumb_count: 0,
                gpu_breadcrumb_cycles: 0,
                top_gpu_breadcrumbs: Vec::new(),
            });
        frame.cpu_metadata_count = frame.cpu_metadata_count.saturating_add(count);
        frame.cpu_metadata_cycles = frame.cpu_metadata_cycles.saturating_add(total_cycles);
        frame.cpu_metadata_seconds =
            cycle_frequency.map(|frequency| frame.cpu_metadata_cycles as f64 / frequency as f64);
    }
    for (frame_number, totals) in frame_scope_totals {
        let frame = frames
            .entry(frame_number)
            .or_insert_with(|| CorrelatedFrameSummary {
                frame_number,
                cpu_metadata_count: 0,
                cpu_metadata_cycles: 0,
                cpu_metadata_seconds: None,
                cpu_begin_cycle: None,
                cpu_end_cycle: None,
                top_cpu_scopes: Vec::new(),
                gpu_queue_count: 0,
                gpu_work_count: 0,
                gpu_work_cycles: 0,
                gpu_breadcrumb_count: 0,
                gpu_breadcrumb_cycles: 0,
                top_gpu_breadcrumbs: Vec::new(),
            });
        frame.top_cpu_scopes = scope_summaries(totals, specs, cycle_frequency)
            .into_iter()
            .take(5)
            .collect();
    }
    let mut breadcrumb_totals = BTreeMap::<u32, BTreeMap<String, (u64, u64)>>::new();
    for gpu_frame in gpu_frames {
        let frame =
            frames
                .entry(gpu_frame.frame_number)
                .or_insert_with(|| CorrelatedFrameSummary {
                    frame_number: gpu_frame.frame_number,
                    cpu_metadata_count: 0,
                    cpu_metadata_cycles: 0,
                    cpu_metadata_seconds: None,
                    cpu_begin_cycle: None,
                    cpu_end_cycle: None,
                    top_cpu_scopes: Vec::new(),
                    gpu_queue_count: 0,
                    gpu_work_count: 0,
                    gpu_work_cycles: 0,
                    gpu_breadcrumb_count: 0,
                    gpu_breadcrumb_cycles: 0,
                    top_gpu_breadcrumbs: Vec::new(),
                });
        frame.gpu_queue_count += 1;
        frame.gpu_work_count = frame.gpu_work_count.saturating_add(gpu_frame.work_count);
        frame.gpu_work_cycles = frame
            .gpu_work_cycles
            .saturating_add(gpu_frame.work_total_cycles);
        frame.gpu_breadcrumb_count = frame
            .gpu_breadcrumb_count
            .saturating_add(gpu_frame.breadcrumb_count);
        frame.gpu_breadcrumb_cycles = frame
            .gpu_breadcrumb_cycles
            .saturating_add(gpu_frame.breadcrumb_total_cycles);
        let totals = breadcrumb_totals.entry(gpu_frame.frame_number).or_default();
        for breadcrumb in &gpu_frame.top_breadcrumbs {
            if parse_rendered_frame_number(&breadcrumb.name).is_some() {
                continue;
            }
            let total = totals.entry(breadcrumb.name.clone()).or_insert((0, 0));
            total.0 = total.0.saturating_add(breadcrumb.count);
            total.1 = total.1.saturating_add(breadcrumb.total_cycles);
        }
    }
    for (frame_number, (begin_cycle, end_cycle)) in frame_cycle_bounds {
        let frame = frames
            .entry(frame_number)
            .or_insert_with(|| CorrelatedFrameSummary {
                frame_number,
                cpu_metadata_count: 0,
                cpu_metadata_cycles: 0,
                cpu_metadata_seconds: None,
                cpu_begin_cycle: None,
                cpu_end_cycle: None,
                top_cpu_scopes: Vec::new(),
                gpu_queue_count: 0,
                gpu_work_count: 0,
                gpu_work_cycles: 0,
                gpu_breadcrumb_count: 0,
                gpu_breadcrumb_cycles: 0,
                top_gpu_breadcrumbs: Vec::new(),
            });
        frame.cpu_begin_cycle = Some(begin_cycle);
        frame.cpu_end_cycle = Some(end_cycle);
    }
    let mut frames = frames.into_values().collect::<Vec<_>>();
    for frame in &mut frames {
        if let Some(totals) = breadcrumb_totals.get(&frame.frame_number) {
            frame.top_gpu_breadcrumbs = gpu_frame_top_breadcrumbs(totals);
        }
    }
    frames.sort_by_key(|frame| frame.frame_number);
    let total_frame_count = u64::try_from(frames.len()).unwrap_or(u64::MAX);
    let truncated = frames.len() > max_frames;
    frames.truncate(max_frames);
    FrameCorrelationDashboard {
        total_frame_count,
        truncated,
        frames,
    }
}

fn parse_rendered_frame_number(name: &str) -> Option<u32> {
    let suffix = name.strip_prefix("Frame ")?;
    suffix
        .split(|character: char| !character.is_ascii_digit())
        .next()
        .filter(|value| !value.is_empty())?
        .parse()
        .ok()
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CounterSpec {
    id: u16,
    name: String,
    kind: CounterKind,
    display_hint: CounterDisplayHint,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct CounterState {
    samples: u64,
    int_samples: u64,
    float_samples: u64,
    first_cycle: Option<u64>,
    last_cycle: Option<u64>,
    min: Option<f64>,
    max: Option<f64>,
    latest: Option<f64>,
    sample_points: Vec<CounterSamplePoint>,
}

fn decode_counter_spec(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
) -> Result<CounterSpec, TraceError> {
    let aux = parse_protocol5_aux(data, event_data_size(event), base_offset)?;
    let name = read_aux_string(event, &aux, "Name")?;
    Ok(CounterSpec {
        id: read_u16_field(event, data, "Id", base_offset)?,
        kind: counter_kind(read_u8_field(event, data, "Type", base_offset)?),
        display_hint: counter_display_hint(read_u8_field(event, data, "DisplayHint", base_offset)?),
        name,
    })
}

fn decode_counter_value(
    event: &EventTypeInfo,
    data: &[u8],
    specs: &BTreeMap<u16, CounterSpec>,
    states: &mut BTreeMap<u16, CounterState>,
    unresolved_samples: &mut u64,
    base_offset: u64,
) -> Result<(), TraceError> {
    match event.event.as_str() {
        "SetValueInt" => {
            let counter_id = read_u16_field(event, data, "CounterId", base_offset)?;
            let cycle = read_u64_field(event, data, "Cycle", base_offset)?;
            let value = read_i64_field(event, data, "Value", base_offset)? as f64;
            if !specs.contains_key(&counter_id) {
                *unresolved_samples += 1;
                return Ok(());
            }
            states
                .entry(counter_id)
                .or_default()
                .record(cycle, value, CounterValueKind::Int);
        }
        "SetValueFloat" => {
            let counter_id = read_u16_field(event, data, "CounterId", base_offset)?;
            let cycle = read_u64_field(event, data, "Cycle", base_offset)?;
            let value = read_f64_field(event, data, "Value", base_offset)?;
            if !specs.contains_key(&counter_id) {
                *unresolved_samples += 1;
                return Ok(());
            }
            states
                .entry(counter_id)
                .or_default()
                .record(cycle, value, CounterValueKind::Float);
        }
        _ => {}
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CounterValueKind {
    Int,
    Float,
}

impl CounterState {
    fn record(&mut self, cycle: u64, value: f64, kind: CounterValueKind) {
        self.samples += 1;
        match kind {
            CounterValueKind::Int => self.int_samples += 1,
            CounterValueKind::Float => self.float_samples += 1,
        }
        if self.first_cycle.is_none() {
            self.first_cycle = Some(cycle);
        }
        self.last_cycle = Some(cycle);
        self.min = Some(self.min.map_or(value, |current| current.min(value)));
        self.max = Some(self.max.map_or(value, |current| current.max(value)));
        self.latest = Some(value);
        if self.sample_points.len() < 40 {
            self.sample_points.push(CounterSamplePoint { cycle, value });
        }
    }
}

fn counter_dashboard(
    specs: BTreeMap<u16, CounterSpec>,
    states: BTreeMap<u16, CounterState>,
    unresolved_samples: u64,
) -> CounterDashboard {
    let mut counters = specs
        .into_iter()
        .map(|(id, spec)| {
            let state = states.get(&id).cloned().unwrap_or_default();
            CounterSummary {
                id,
                name: spec.name,
                kind: spec.kind,
                display_hint: spec.display_hint,
                samples: state.samples,
                first_cycle: state.first_cycle,
                last_cycle: state.last_cycle,
                min: state.min,
                max: state.max,
                latest: state.latest,
                sample_points: state.sample_points,
            }
        })
        .collect::<Vec<_>>();
    counters.sort_by(|left, right| {
        right
            .samples
            .cmp(&left.samples)
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.id.cmp(&right.id))
    });

    CounterDashboard {
        specs: u64::try_from(counters.len()).unwrap(),
        samples: counters.iter().map(|counter| counter.samples).sum(),
        int_samples: states.values().map(|state| state.int_samples).sum(),
        float_samples: states.values().map(|state| state.float_samples).sum(),
        unresolved_samples,
        counters,
    }
}

fn counter_kind(raw: u8) -> CounterKind {
    match raw {
        0 => CounterKind::Int,
        1 => CounterKind::Float,
        _ => CounterKind::Unknown,
    }
}

fn counter_display_hint(raw: u8) -> CounterDisplayHint {
    match raw {
        0 => CounterDisplayHint::None,
        1 => CounterDisplayHint::Memory,
        _ => CounterDisplayHint::Unknown,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct StatSpec {
    id: u32,
    name: String,
    description: String,
    group: String,
    is_floating_point: bool,
    is_memory: bool,
    should_clear_every_frame: bool,
}

fn decode_stat_spec(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
) -> Result<StatSpec, TraceError> {
    let aux = parse_protocol5_aux(data, event_data_size(event), base_offset)?;
    Ok(StatSpec {
        id: read_u32_field(event, data, "Id", base_offset)?,
        is_floating_point: read_u8_field(event, data, "IsFloatingPoint", base_offset)? != 0,
        is_memory: read_u8_field(event, data, "IsMemory", base_offset)? != 0,
        should_clear_every_frame: read_u8_field(event, data, "ShouldClearEveryFrame", base_offset)?
            != 0,
        name: read_aux_string(event, &aux, "Name")?,
        description: read_aux_string(event, &aux, "Description")?,
        group: read_aux_string(event, &aux, "Group")?,
    })
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct StatGroupTotals {
    specs: u64,
    floating_point_specs: u64,
    memory_specs: u64,
    clear_every_frame_specs: u64,
}

impl StatGroupTotals {
    fn record(&mut self, spec: &StatSpec) {
        self.specs += 1;
        if spec.is_floating_point {
            self.floating_point_specs += 1;
        }
        if spec.is_memory {
            self.memory_specs += 1;
        }
        if spec.should_clear_every_frame {
            self.clear_every_frame_specs += 1;
        }
    }
}

fn stats_dashboard(specs: BTreeMap<u32, StatSpec>) -> StatsDashboard {
    let mut group_totals = BTreeMap::<String, StatGroupTotals>::new();
    for spec in specs.values() {
        group_totals
            .entry(spec.group.clone())
            .or_default()
            .record(spec);
    }

    let mut groups = group_totals
        .into_iter()
        .map(|(name, totals)| StatGroupSummary {
            name,
            specs: totals.specs,
            floating_point_specs: totals.floating_point_specs,
            memory_specs: totals.memory_specs,
            clear_every_frame_specs: totals.clear_every_frame_specs,
        })
        .collect::<Vec<_>>();
    groups.sort_by(|left, right| {
        right
            .specs
            .cmp(&left.specs)
            .then_with(|| left.name.cmp(&right.name))
    });

    let mut stats = specs
        .into_values()
        .map(|spec| StatSpecSummary {
            id: spec.id,
            name: spec.name,
            description: spec.description,
            group: spec.group,
            is_floating_point: spec.is_floating_point,
            is_memory: spec.is_memory,
            should_clear_every_frame: spec.should_clear_every_frame,
        })
        .collect::<Vec<_>>();
    stats.sort_by(|left, right| {
        left.group
            .cmp(&right.group)
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.id.cmp(&right.id))
    });

    StatsDashboard {
        specs: u64::try_from(stats.len()).unwrap(),
        floating_point_specs: u64::try_from(
            stats.iter().filter(|stat| stat.is_floating_point).count(),
        )
        .unwrap(),
        memory_specs: u64::try_from(stats.iter().filter(|stat| stat.is_memory).count()).unwrap(),
        clear_every_frame_specs: u64::try_from(
            stats
                .iter()
                .filter(|stat| stat.should_clear_every_frame)
                .count(),
        )
        .unwrap(),
        sample_events: 0,
        unresolved_samples: 0,
        sample_state_overflow: 0,
        malformed_batches: 0,
        groups,
        stats,
        samples: Vec::new(),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CsvCategory {
    index: i32,
    name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CsvStat {
    stat_id: u64,
    category_index: i32,
    name: String,
    kind: CsvStatKind,
}

fn decode_csv_category(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
) -> Result<CsvCategory, TraceError> {
    let aux = parse_protocol5_aux(data, event_data_size(event), base_offset)?;
    Ok(CsvCategory {
        index: read_i32_field(event, data, "Index", base_offset)?,
        name: read_aux_string(event, &aux, "Name")?,
    })
}

fn decode_csv_stat(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
) -> Result<CsvStat, TraceError> {
    let aux = parse_protocol5_aux(data, event_data_size(event), base_offset)?;
    let kind = match event.event.as_str() {
        "DefineInlineStat" => CsvStatKind::Inline,
        _ => CsvStatKind::Declared,
    };
    Ok(CsvStat {
        stat_id: read_u64_field(event, data, "StatId", base_offset)?,
        category_index: read_i32_field(event, data, "CategoryIndex", base_offset)?,
        name: read_aux_string(event, &aux, "Name")?,
        kind,
    })
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct CsvCategoryTotals {
    stats: u64,
    declared_stats: u64,
    inline_stats: u64,
}

impl CsvCategoryTotals {
    fn record(&mut self, kind: CsvStatKind) {
        self.stats += 1;
        match kind {
            CsvStatKind::Declared => self.declared_stats += 1,
            CsvStatKind::Inline => self.inline_stats += 1,
        }
    }
}

fn csv_dashboard(
    categories: BTreeMap<i32, CsvCategory>,
    stats: BTreeMap<u64, CsvStat>,
) -> CsvDashboard {
    let mut totals = BTreeMap::<i32, CsvCategoryTotals>::new();
    let mut unresolved_stats = 0_u64;
    for stat in stats.values() {
        if categories.contains_key(&stat.category_index) {
            totals
                .entry(stat.category_index)
                .or_default()
                .record(stat.kind);
        } else {
            unresolved_stats += 1;
        }
    }

    let mut top_categories = categories
        .values()
        .map(|category| {
            let totals = totals.get(&category.index).cloned().unwrap_or_default();
            CsvCategorySummary {
                index: category.index,
                name: category.name.clone(),
                stats: totals.stats,
                declared_stats: totals.declared_stats,
                inline_stats: totals.inline_stats,
            }
        })
        .collect::<Vec<_>>();
    top_categories.sort_by(|left, right| {
        right
            .stats
            .cmp(&left.stats)
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.index.cmp(&right.index))
    });

    let mut stat_defs = stats
        .into_values()
        .map(|stat| CsvStatSummary {
            stat_id: stat.stat_id,
            name: stat.name,
            category_index: stat.category_index,
            category: categories
                .get(&stat.category_index)
                .map(|category| category.name.clone()),
            kind: stat.kind,
        })
        .collect::<Vec<_>>();
    stat_defs.sort_by(|left, right| {
        left.category_index
            .cmp(&right.category_index)
            .then_with(|| left.name.cmp(&right.name))
            .then_with(|| left.stat_id.cmp(&right.stat_id))
    });

    CsvDashboard {
        categories: u64::try_from(categories.len()).unwrap(),
        stats: u64::try_from(stat_defs.len()).unwrap(),
        declared_stats: u64::try_from(
            stat_defs
                .iter()
                .filter(|stat| stat.kind == CsvStatKind::Declared)
                .count(),
        )
        .unwrap(),
        inline_stats: u64::try_from(
            stat_defs
                .iter()
                .filter(|stat| stat.kind == CsvStatKind::Inline)
                .count(),
        )
        .unwrap(),
        unresolved_stats,
        sample_events: 0,
        begin_events: 0,
        end_events: 0,
        unmatched_ends: 0,
        custom_int_samples: 0,
        custom_float_samples: 0,
        open_begins: 0,
        sample_unresolved_stats: 0,
        top_categories,
        stat_defs,
        duration_samples: Vec::new(),
        value_samples: Vec::new(),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LoadTimeClassInfo {
    class: u64,
    name: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct LoadTimeState {
    classes: BTreeMap<u64, String>,
    packages: BTreeMap<u64, LoadTimePackageSummary>,
    open_requests: BTreeMap<u64, u64>,
    requests: LoadTimeRequestDashboard,
    async_loading: LoadTimeAsyncLoadingSummary,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LoadTimePackageInfo {
    async_package: u64,
    total_header_size: u32,
    import_count: u32,
    export_count: u32,
    name: String,
    priority: Option<i32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LoadTimeRequestEvent {
    cycle: u64,
    request_id: u64,
}

fn decode_load_time_class_info(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
) -> Result<LoadTimeClassInfo, TraceError> {
    let aux = parse_protocol5_aux(data, event_data_size(event), base_offset)?;
    Ok(LoadTimeClassInfo {
        class: read_pointer_field(event, data, "Class", base_offset)?,
        name: read_aux_string(event, &aux, "Name")?,
    })
}

fn decode_load_time_package_summary(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
) -> Result<LoadTimePackageInfo, TraceError> {
    let aux = parse_protocol5_aux(data, event_data_size(event), base_offset)?;
    Ok(LoadTimePackageInfo {
        async_package: read_pointer_field(event, data, "AsyncPackage", base_offset)?,
        total_header_size: read_u32_field(event, data, "TotalHeaderSize", base_offset)?,
        import_count: read_u32_field(event, data, "ImportCount", base_offset)?,
        export_count: read_u32_field(event, data, "ExportCount", base_offset)?,
        name: read_aux_text(event, &aux, "Name")?,
        priority: read_optional_i32_field(event, data, "Priority", base_offset)?,
    })
}

fn decode_load_time_request_event(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
) -> Result<LoadTimeRequestEvent, TraceError> {
    Ok(LoadTimeRequestEvent {
        cycle: read_u64_field(event, data, "Cycle", base_offset)?,
        request_id: read_u64_field(event, data, "RequestId", base_offset)?,
    })
}

fn decode_load_time_cycle(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
) -> Result<u64, TraceError> {
    read_u64_field(event, data, "Cycle", base_offset)
}

fn decode_load_time_event(
    event: &EventTypeInfo,
    data: &[u8],
    state: &mut LoadTimeState,
    base_offset: u64,
) -> Result<(), TraceError> {
    match event.event.as_str() {
        "ClassInfo" => {
            let class_info = decode_load_time_class_info(event, data, base_offset)?;
            state.classes.insert(class_info.class, class_info.name);
        }
        "PackageSummary" => {
            let package = decode_load_time_package_summary(event, data, base_offset)?;
            state.packages.insert(
                package.async_package,
                LoadTimePackageSummary {
                    async_package: package.async_package,
                    name: package.name,
                    total_header_size: package.total_header_size,
                    import_count: package.import_count,
                    export_count: package.export_count,
                    priority: package.priority,
                },
            );
        }
        "BeginRequest" => {
            let request = decode_load_time_request_event(event, data, base_offset)?;
            state.requests.begun += 1;
            state
                .open_requests
                .insert(request.request_id, request.cycle);
        }
        "EndRequest" => {
            let request = decode_load_time_request_event(event, data, base_offset)?;
            state.requests.ended += 1;
            let Some(start_cycle) = state.open_requests.remove(&request.request_id) else {
                state.requests.unmatched_ends += 1;
                return Ok(());
            };
            let duration = request.cycle.saturating_sub(start_cycle);
            state.requests.completed += 1;
            state.requests.total_cycles = state.requests.total_cycles.saturating_add(duration);
            if state.requests.samples.len() < 40 {
                state.requests.samples.push(LoadTimeRequestSummary {
                    request_id: request.request_id,
                    start_cycle,
                    end_cycle: request.cycle,
                    duration_cycles: duration,
                });
            }
        }
        "StartAsyncLoading" | "SuspendAsyncLoading" | "ResumeAsyncLoading" => {
            let cycle = decode_load_time_cycle(event, data, base_offset)?;
            match event.event.as_str() {
                "StartAsyncLoading" => state.async_loading.starts += 1,
                "SuspendAsyncLoading" => state.async_loading.suspends += 1,
                "ResumeAsyncLoading" => state.async_loading.resumes += 1,
                _ => unreachable!("matched async loading events"),
            }
            state.async_loading.first_cycle = Some(
                state
                    .async_loading
                    .first_cycle
                    .map_or(cycle, |first| first.min(cycle)),
            );
            state.async_loading.last_cycle = Some(
                state
                    .async_loading
                    .last_cycle
                    .map_or(cycle, |last| last.max(cycle)),
            );
        }
        _ => {}
    }
    Ok(())
}

impl LoadTimeState {
    fn dashboard(mut self) -> LoadingDashboard {
        self.requests.open = u64::try_from(self.open_requests.len()).unwrap();
        let mut classes = self
            .classes
            .into_iter()
            .map(|(class, name)| LoadTimeClassSummary { class, name })
            .collect::<Vec<_>>();
        classes.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.class.cmp(&right.class))
        });

        let package_count = u64::try_from(self.packages.len()).unwrap();
        let mut packages = self.packages.into_values().collect::<Vec<_>>();
        packages.sort_by(|left, right| {
            left.name
                .cmp(&right.name)
                .then_with(|| left.async_package.cmp(&right.async_package))
        });
        packages.truncate(80);

        LoadingDashboard {
            class_count: u64::try_from(classes.len()).unwrap(),
            classes,
            package_count,
            packages,
            requests: self.requests,
            async_loading: self.async_loading,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
struct IoStoreState {
    backends: BTreeMap<u64, IoStoreBackendState>,
    requests: BTreeMap<u64, IoStoreRequestState>,
    requests_created: u64,
    requests_started: u64,
    requests_completed: u64,
    requests_failed: u64,
    requests_unresolved: u64,
    bytes_requested: u64,
    bytes_completed: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct IoStoreBackendState {
    name: String,
    starts: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct IoStoreRequestState {
    request_handle: u64,
    batch_handle: u64,
    chunk_id_hash: u32,
    chunk_type: u8,
    offset: u64,
    size: u64,
    backend_handle: Option<u64>,
    create_cycle: u64,
    start_cycle: Option<u64>,
    complete_cycle: Option<u64>,
    completed_size: Option<u64>,
    status: IoStoreRequestStatus,
}

fn decode_io_store_event(
    event: &EventTypeInfo,
    data: &[u8],
    state: &mut IoStoreState,
    base_offset: u64,
) -> Result<(), TraceError> {
    match event.event.as_str() {
        "BackendName" => {
            let aux = parse_protocol5_aux(data, event_data_size(event), base_offset)?;
            let backend_handle = read_u64_field(event, data, "BackendHandle", base_offset)?;
            let name = read_aux_text(event, &aux, "Name")?;
            state.backends.entry(backend_handle).or_default().name = name;
        }
        "RequestCreate" => {
            let request = IoStoreRequestState {
                create_cycle: read_u64_field(event, data, "Cycle", base_offset)?,
                request_handle: read_u64_field(event, data, "RequestHandle", base_offset)?,
                batch_handle: read_u64_field(event, data, "BatchHandle", base_offset)?,
                chunk_id_hash: read_u32_field(event, data, "ChunkIdHash", base_offset)?,
                chunk_type: read_u8_field(event, data, "ChunkType", base_offset)?,
                offset: read_u64_field(event, data, "Offset", base_offset)?,
                size: read_u64_field(event, data, "Size", base_offset)?,
                backend_handle: None,
                start_cycle: None,
                complete_cycle: None,
                completed_size: None,
                status: IoStoreRequestStatus::Created,
            };
            state.requests_created += 1;
            state.bytes_requested = state.bytes_requested.saturating_add(request.size);
            state.requests.insert(request.request_handle, request);
        }
        "RequestStarted" => {
            let cycle = read_u64_field(event, data, "Cycle", base_offset)?;
            let request_handle = read_u64_field(event, data, "RequestHandle", base_offset)?;
            let backend_handle = read_u64_field(event, data, "BackendHandle", base_offset)?;
            state.requests_started += 1;
            state.backends.entry(backend_handle).or_default().starts += 1;
            if let Some(request) = state.requests.get_mut(&request_handle) {
                request.backend_handle = Some(backend_handle);
                request.start_cycle = Some(cycle);
                request.status = IoStoreRequestStatus::Started;
            }
        }
        "RequestCompleted" => {
            let cycle = read_u64_field(event, data, "Cycle", base_offset)?;
            let request_handle = read_u64_field(event, data, "RequestHandle", base_offset)?;
            let size = read_u64_field(event, data, "Size", base_offset)?;
            state.requests_completed += 1;
            state.bytes_completed = state.bytes_completed.saturating_add(size);
            if let Some(request) = state.requests.get_mut(&request_handle) {
                request.complete_cycle = Some(cycle);
                request.completed_size = Some(size);
                request.status = IoStoreRequestStatus::Completed;
            }
        }
        "RequestFailed" => {
            let _cycle = read_u64_field(event, data, "Cycle", base_offset)?;
            let request_handle = read_u64_field(event, data, "RequestHandle", base_offset)?;
            state.requests_failed += 1;
            if let Some(request) = state.requests.get_mut(&request_handle) {
                request.status = IoStoreRequestStatus::Failed;
            }
        }
        "RequestUnresolved" => {
            let _cycle = read_u64_field(event, data, "Cycle", base_offset)?;
            let request_handle = read_u64_field(event, data, "RequestHandle", base_offset)?;
            state.requests_unresolved += 1;
            if let Some(request) = state.requests.get_mut(&request_handle) {
                request.status = IoStoreRequestStatus::Unresolved;
            }
        }
        _ => {}
    }
    Ok(())
}

impl IoStoreState {
    fn dashboard(self) -> IoStoreDashboard {
        let backend_names = self
            .backends
            .iter()
            .map(|(handle, backend)| (*handle, backend.name.clone()))
            .collect::<BTreeMap<_, _>>();
        let mut backends = self
            .backends
            .iter()
            .map(|(backend_handle, backend)| IoStoreBackendSummary {
                backend_handle: *backend_handle,
                name: backend.name.clone(),
                starts: backend.starts,
            })
            .collect::<Vec<_>>();
        backends.sort_by(|left, right| {
            right
                .starts
                .cmp(&left.starts)
                .then_with(|| left.name.cmp(&right.name))
                .then_with(|| left.backend_handle.cmp(&right.backend_handle))
        });

        let mut request_samples = self
            .requests
            .into_values()
            .map(|request| IoStoreRequestSummary {
                backend_name: request
                    .backend_handle
                    .and_then(|handle| backend_names.get(&handle).cloned())
                    .filter(|name| !name.is_empty()),
                request_handle: request.request_handle,
                batch_handle: request.batch_handle,
                chunk_id_hash: request.chunk_id_hash,
                chunk_type: request.chunk_type,
                offset: request.offset,
                size: request.size,
                backend_handle: request.backend_handle,
                create_cycle: request.create_cycle,
                start_cycle: request.start_cycle,
                complete_cycle: request.complete_cycle,
                completed_size: request.completed_size,
                status: request.status,
            })
            .collect::<Vec<_>>();
        request_samples.sort_by(|left, right| left.create_cycle.cmp(&right.create_cycle));
        request_samples.truncate(40);

        IoStoreDashboard {
            backend_count: u64::try_from(backends.len()).unwrap(),
            backends,
            requests_created: self.requests_created,
            requests_started: self.requests_started,
            requests_completed: self.requests_completed,
            requests_failed: self.requests_failed,
            requests_unresolved: self.requests_unresolved,
            bytes_requested: self.bytes_requested,
            bytes_completed: self.bytes_completed,
            request_samples,
        }
    }
}

fn decode_platform_file_event(
    event: &EventTypeInfo,
    data: &[u8],
    state: &mut PlatformFileProvider,
    thread_id: u16,
    base_offset: u64,
) -> Result<(), TraceError> {
    match event.event.as_str() {
        "BeginOpen" => {
            let aux = parse_protocol5_aux(data, event_data_size(event), base_offset)?;
            let cycle = read_u64_field(event, data, "Cycle", base_offset)?;
            let path = read_aux_text(event, &aux, "Path")?;
            state.begin_open(thread_id, cycle, path);
        }
        "EndOpen" => {
            let cycle = read_u64_field(event, data, "Cycle", base_offset)?;
            let file_handle = read_u64_field(event, data, "FileHandle", base_offset)?;
            state.end_open(thread_id, cycle, file_handle);
        }
        "BeginReOpen" => {
            let cycle = read_u64_field(event, data, "Cycle", base_offset)?;
            let old_file_handle = read_u64_field(event, data, "OldFileHandle", base_offset)?;
            state.begin_reopen(thread_id, cycle, old_file_handle);
        }
        "EndReOpen" => {
            let cycle = read_u64_field(event, data, "Cycle", base_offset)?;
            let new_file_handle = read_u64_field(event, data, "NewFileHandle", base_offset)?;
            state.end_reopen(thread_id, cycle, new_file_handle);
        }
        "BeginClose" => {
            let cycle = read_u64_field(event, data, "Cycle", base_offset)?;
            let file_handle = read_u64_field(event, data, "FileHandle", base_offset)?;
            state.begin_close(thread_id, cycle, file_handle);
        }
        "EndClose" => {
            let cycle = read_u64_field(event, data, "Cycle", base_offset)?;
            state.end_close(thread_id, cycle);
        }
        "BeginRead" => {
            let cycle = read_u64_field(event, data, "Cycle", base_offset)?;
            let read_handle = read_u64_field(event, data, "ReadHandle", base_offset)?;
            let file_handle = read_u64_field(event, data, "FileHandle", base_offset)?;
            let offset = read_u64_field(event, data, "Offset", base_offset)?;
            let size = read_u64_field(event, data, "Size", base_offset)?;
            state.begin_read(thread_id, cycle, read_handle, file_handle, offset, size);
        }
        "EndRead" => {
            let cycle = read_u64_field(event, data, "Cycle", base_offset)?;
            let read_handle = read_u64_field(event, data, "ReadHandle", base_offset)?;
            let size_read = read_u64_field(event, data, "SizeRead", base_offset)?;
            state.end_read(cycle, read_handle, size_read);
        }
        "BeginWrite" => {
            let cycle = read_u64_field(event, data, "Cycle", base_offset)?;
            let write_handle = read_u64_field(event, data, "WriteHandle", base_offset)?;
            let file_handle = read_u64_field(event, data, "FileHandle", base_offset)?;
            let offset = read_u64_field(event, data, "Offset", base_offset)?;
            let size = read_u64_field(event, data, "Size", base_offset)?;
            state.begin_write(thread_id, cycle, write_handle, file_handle, offset, size);
        }
        "EndWrite" => {
            let cycle = read_u64_field(event, data, "Cycle", base_offset)?;
            let write_handle = read_u64_field(event, data, "WriteHandle", base_offset)?;
            let size_written = read_u64_field(event, data, "SizeWritten", base_offset)?;
            state.end_write(cycle, write_handle, size_written);
        }
        _ => {}
    }
    Ok(())
}

fn decode_trace_thread_timing(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
    thread_id: u16,
) -> Result<TraceThreadTimingSummary, TraceError> {
    Ok(TraceThreadTimingSummary {
        thread_id,
        base_timestamp: read_u64_field(event, data, "BaseTimestamp", base_offset)?,
    })
}

fn trace_timing_dashboard(
    threads: BTreeMap<u16, TraceThreadTimingSummary>,
) -> TraceTimingDashboard {
    let threads = threads.into_values().collect::<Vec<_>>();
    TraceTimingDashboard {
        thread_count: u64::try_from(threads.len()).unwrap(),
        threads,
    }
}

fn decode_cpu_end_thread(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
    thread_id: u16,
) -> Result<CpuEndThreadSummary, TraceError> {
    Ok(CpuEndThreadSummary {
        thread_id,
        cycle: read_u64_field(event, data, "Cycle", base_offset)?,
    })
}

fn decode_memory_init(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
) -> Result<MemoryInit, TraceError> {
    let init = MemoryInit {
        page_size: read_u64_field(event, data, "PageSize", base_offset)?,
        marker_period: read_u32_field(event, data, "MarkerPeriod", base_offset)?,
        version: read_u8_field(event, data, "Version", base_offset)?,
        min_alignment: read_u8_field(event, data, "MinAlignment", base_offset)?,
        size_shift: read_u8_field(event, data, "SizeShift", base_offset)?,
    };
    if !(1..=2).contains(&init.version) {
        return Err(TraceError::new(
            TraceErrorKind::UnsupportedFormat,
            base_offset,
            "Memory.Init.Version",
            format!(
                "unsupported Memory trace version {}; supported versions are 1 through 2",
                init.version
            ),
        ));
    }
    if init.size_shift >= 64 {
        return Err(TraceError::new(
            TraceErrorKind::MalformedData,
            base_offset,
            "Memory.Init.SizeShift",
            format!("size shift {} must be less than 64", init.size_shift),
        ));
    }
    Ok(init)
}

fn decode_memory_tag(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
) -> Result<MemoryTag, TraceError> {
    let aux = parse_protocol5_aux(data, event_data_size(event), base_offset)?;
    Ok(MemoryTag {
        tag: read_i32_field(event, data, "Tag", base_offset)?,
        parent: read_i32_field(event, data, "Parent", base_offset)?,
        display: read_aux_string(event, &aux, "Display")?,
    })
}

fn decode_memory_scope(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
) -> Result<i32, TraceError> {
    read_i32_field(event, data, "Tag", base_offset)
}

fn decode_memory_allocation(
    event: &EventTypeInfo,
    data: &[u8],
    init: MemoryInit,
    base_offset: u64,
) -> Result<MemoryAllocation, TraceError> {
    let root_heap = match event.event.as_str() {
        "AllocSystem" | "ReallocAllocSystem" => 0,
        "AllocVideo" | "ReallocAllocVideo" => 1,
        "Alloc" | "ReallocAlloc" => read_u8_field(event, data, "RootHeap", base_offset)?,
        _ => unreachable!("memory allocation event was prefiltered"),
    };
    let packed_size = u64::from(read_u32_field(event, data, "Size", base_offset)?);
    let size_lower = u64::from(read_u8_field(
        event,
        data,
        "AlignmentPow2_SizeLower",
        base_offset,
    )?) & ((1_u64 << init.size_shift) - 1);
    Ok(MemoryAllocation {
        address: read_u64_field(event, data, "Address", base_offset)?,
        size: (packed_size << init.size_shift) | size_lower,
        root_heap,
        callstack_id: read_u32_field(event, data, "CallstackId", base_offset)?,
        kind: if event.event.starts_with("Realloc") {
            MemoryAllocationKind::ReallocAlloc
        } else {
            MemoryAllocationKind::Alloc
        },
    })
}

fn decode_memory_free(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
) -> Result<MemoryFree, TraceError> {
    let root_heap = match event.event.as_str() {
        "FreeSystem" | "ReallocFreeSystem" => 0,
        "FreeVideo" | "ReallocFreeVideo" => 1,
        "Free" | "ReallocFree" => read_u8_field(event, data, "RootHeap", base_offset)?,
        _ => unreachable!("memory free event was prefiltered"),
    };
    Ok(MemoryFree {
        address: read_u64_field(event, data, "Address", base_offset)?,
        root_heap,
        is_realloc: event.event.starts_with("Realloc"),
    })
}

const MAX_LLM_VALUES_PER_EVENT: usize = 4_096;

#[derive(Clone, Debug, Eq, PartialEq)]
struct LlmTagValueSample {
    tracker_id: u8,
    cycle: u64,
    values: Vec<(i64, i64)>,
    dropped_values: u64,
}

fn decode_llm_tag(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
) -> Result<LlmTag, TraceError> {
    let aux = parse_protocol5_aux(data, event_data_size(event), base_offset)?;
    Ok(LlmTag {
        tag: read_llm_tag_id_field(event, data, "TagId", base_offset)?,
        parent: read_llm_tag_id_field(event, data, "ParentId", base_offset)?,
        tag_set: read_u8_field(event, data, "TagSetId", base_offset)?,
        name: read_aux_string(event, &aux, "Name")?,
    })
}

fn decode_llm_tracker(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
) -> Result<LlmTracker, TraceError> {
    let aux = parse_protocol5_aux(data, event_data_size(event), base_offset)?;
    Ok(LlmTracker {
        tracker_id: read_u8_field(event, data, "TrackerId", base_offset)?,
        name: read_aux_string(event, &aux, "Name")?,
    })
}

fn decode_llm_tag_set(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
) -> Result<LlmTagSet, TraceError> {
    let aux = parse_protocol5_aux(data, event_data_size(event), base_offset)?;
    Ok(LlmTagSet {
        tag_set: read_u8_field(event, data, "TagSetId", base_offset)?,
        name: read_aux_string(event, &aux, "Name")?,
    })
}

fn decode_llm_tag_values(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
) -> Result<LlmTagValueSample, TraceError> {
    let aux = parse_protocol5_aux(data, event_data_size(event), base_offset)?;
    let tags = read_required_aux_bytes(event, &aux, "Tags")?;
    let values = read_required_aux_bytes(event, &aux, "Values")?;
    if values.len() % size_of::<i64>() != 0 {
        return Err(TraceError::new(
            TraceErrorKind::MalformedData,
            base_offset,
            "LLM.TagValue.Values",
            format!(
                "{} byte Values array is not a whole number of i64 values",
                values.len()
            ),
        ));
    }

    let count = values.len() / size_of::<i64>();
    let tag_width = if tags.len() == count * size_of::<i64>() {
        size_of::<i64>()
    } else if tags.len() == count * size_of::<u32>() {
        size_of::<u32>()
    } else {
        return Err(TraceError::new(
            TraceErrorKind::MalformedData,
            base_offset,
            "LLM.TagValue.Tags",
            format!(
                "{} byte Tags array does not match {count} i64 values",
                tags.len()
            ),
        ));
    };

    let retained_count = count.min(MAX_LLM_VALUES_PER_EVENT);
    let mut decoded = Vec::with_capacity(retained_count);
    for index in 0..retained_count {
        let tag_offset = index * tag_width;
        let tag = match tag_width {
            4 => i64::from(u32::from_le_bytes(
                tags[tag_offset..tag_offset + 4]
                    .try_into()
                    .expect("LLM tag width was validated"),
            )),
            8 => i64::from_le_bytes(
                tags[tag_offset..tag_offset + 8]
                    .try_into()
                    .expect("LLM tag width was validated"),
            ),
            _ => unreachable!("LLM tag width is constrained to 4 or 8"),
        };
        let value_offset = index * size_of::<i64>();
        let value = i64::from_le_bytes(
            values[value_offset..value_offset + size_of::<i64>()]
                .try_into()
                .expect("LLM value width was validated"),
        );
        decoded.push((tag, value));
    }

    Ok(LlmTagValueSample {
        tracker_id: read_u8_field(event, data, "TrackerId", base_offset)?,
        cycle: read_u64_field(event, data, "Cycle", base_offset)?,
        values: decoded,
        dropped_values: u64::try_from(count - retained_count).unwrap_or(u64::MAX),
    })
}

fn read_llm_tag_id_field(
    event: &EventTypeInfo,
    data: &[u8],
    name: &str,
    base_offset: u64,
) -> Result<i64, TraceError> {
    let field = find_field(event, name)?;
    match field.size {
        4 => Ok(i64::from(read_u32_field(event, data, name, base_offset)?)),
        8 => read_i64_field(event, data, name, base_offset),
        size => Err(TraceError::new(
            TraceErrorKind::MalformedData,
            base_offset + u64::from(field.offset),
            format!("{}.{}", event.event, name),
            format!("expected 4 or 8 byte LLM tag id, got {size}"),
        )),
    }
}

pub(crate) fn read_required_aux_bytes<'a>(
    event: &EventTypeInfo,
    aux: &'a BTreeMap<u8, Vec<u8>>,
    name: &str,
) -> Result<&'a [u8], TraceError> {
    let index = event
        .fields
        .iter()
        .position(|field| field.name == name)
        .ok_or_else(|| {
            TraceError::new(
                TraceErrorKind::MalformedData,
                0,
                format!("{}.{}", event.event, name),
                "declared event is missing required aux array field",
            )
        })?;
    aux.get(&(index as u8)).map(Vec::as_slice).ok_or_else(|| {
        TraceError::new(
            TraceErrorKind::MalformedData,
            0,
            format!("{}.{}", event.event, name),
            "event payload is missing required aux array",
        )
    })
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct MetadataStackState {
    clear_scope_count: u64,
    saved_stacks: BTreeMap<u32, u64>,
    restored_stacks: BTreeMap<u32, u64>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct CpuMetadataStackRuntimeState {
    active: Vec<CpuMetadataStackEntry>,
    saved_stacks: BTreeMap<u32, Vec<u32>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CpuMetadataStackEntry {
    metadata_id: u32,
    restored: bool,
}

fn decode_metadata_stack_event(
    event: &EventTypeInfo,
    data: &[u8],
    state: &mut MetadataStackState,
    base_offset: u64,
) -> Result<(), TraceError> {
    match event.event.as_str() {
        "ClearScope" => state.clear_scope_count += 1,
        "SaveStack" => {
            let id = read_u32_field(event, data, "Id", base_offset)?;
            *state.saved_stacks.entry(id).or_default() += 1;
        }
        "RestoreStack" => {
            let id = read_u32_field(event, data, "Id", base_offset)?;
            *state.restored_stacks.entry(id).or_default() += 1;
        }
        _ => {}
    }
    Ok(())
}

fn apply_metadata_stack_event_to_cpu_context(
    event: &EventTypeInfo,
    data: &[u8],
    state: &mut CpuMetadataStackRuntimeState,
    base_offset: u64,
) -> Result<(), TraceError> {
    match event.event.as_str() {
        "ClearScope" => state.active.clear(),
        "SaveStack" => {
            let id = read_u32_field(event, data, "Id", base_offset)?;
            state.saved_stacks.insert(
                id,
                state.active.iter().map(|entry| entry.metadata_id).collect(),
            );
        }
        "RestoreStack" => {
            let id = read_u32_field(event, data, "Id", base_offset)?;
            state.active = state
                .saved_stacks
                .get(&id)
                .into_iter()
                .flatten()
                .map(|&metadata_id| CpuMetadataStackEntry {
                    metadata_id,
                    restored: true,
                })
                .collect();
        }
        _ => {}
    }
    Ok(())
}

impl CpuMetadataStackRuntimeState {
    fn enter_inline(&mut self, metadata_id: u32) {
        self.active.push(CpuMetadataStackEntry {
            metadata_id,
            restored: false,
        });
    }

    fn leave_inline(&mut self, metadata_id: u32) {
        let Some(index) = self
            .active
            .iter()
            .rposition(|entry| entry.metadata_id == metadata_id && !entry.restored)
        else {
            return;
        };
        self.active.remove(index);
    }

    fn restored_metadata_id(&self) -> Option<u32> {
        self.active
            .iter()
            .rev()
            .find(|entry| entry.restored)
            .map(|entry| entry.metadata_id)
    }

    fn active_frame_number(&self, metadata: &BTreeMap<u32, CpuMetadataRecord>) -> Option<u32> {
        self.active.iter().rev().find_map(|entry| {
            metadata
                .get(&entry.metadata_id)
                .and_then(|record| record.rendered_name.as_deref())
                .and_then(parse_rendered_frame_number)
        })
    }
}

impl MetadataStackState {
    fn dashboard(self) -> MetadataStackDashboard {
        let mut saved_stacks = self
            .saved_stacks
            .iter()
            .map(|(&id, &count)| MetadataSavedStackSummary { id, count })
            .collect::<Vec<_>>();
        saved_stacks.sort_by(|left, right| {
            right
                .count
                .cmp(&left.count)
                .then_with(|| left.id.cmp(&right.id))
        });
        let mut restored_stacks = self
            .restored_stacks
            .iter()
            .map(|(&id, &count)| MetadataRestoredStackSummary {
                id,
                count,
                saved: self.saved_stacks.contains_key(&id),
            })
            .collect::<Vec<_>>();
        restored_stacks.sort_by(|left, right| {
            right
                .saved
                .cmp(&left.saved)
                .then_with(|| right.count.cmp(&left.count))
                .then_with(|| left.id.cmp(&right.id))
        });
        let ids = self
            .saved_stacks
            .keys()
            .chain(self.restored_stacks.keys())
            .copied()
            .collect::<BTreeSet<_>>();
        let stack_ids = ids
            .iter()
            .map(|id| MetadataStackIdSummary {
                id: *id,
                saves: self.saved_stacks.get(id).copied().unwrap_or_default(),
                restores: self.restored_stacks.get(id).copied().unwrap_or_default(),
            })
            .collect::<Vec<_>>();
        let restored_stack_count = restored_stacks.iter().map(|stack| stack.count).sum();
        let unmatched_restore_count = restored_stacks
            .iter()
            .filter(|stack| !stack.saved)
            .map(|stack| stack.count)
            .sum();
        MetadataStackDashboard {
            clear_scope_count: self.clear_scope_count,
            saved_stack_count: saved_stacks.iter().map(|stack| stack.count).sum(),
            restored_stack_count,
            unmatched_restore_count,
            saved_stacks,
            restored_stacks,
            stack_ids,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct SlateWidgetEvent {
    cycle: u64,
    widget_id: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct SlateWidgetState {
    count: u64,
    first_cycle: Option<u64>,
    last_cycle: Option<u64>,
}

impl SlateWidgetState {
    fn record(&mut self, cycle: u64) {
        self.count += 1;
        if self.first_cycle.is_none() {
            self.first_cycle = Some(cycle);
        }
        self.last_cycle = Some(cycle);
    }
}

fn decode_slate_add_widget(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
) -> Result<SlateWidgetEvent, TraceError> {
    Ok(SlateWidgetEvent {
        cycle: read_u64_field(event, data, "Cycle", base_offset)?,
        widget_id: read_u64_field(event, data, "WidgetId", base_offset)?,
    })
}

fn slate_dashboard(widgets: BTreeMap<u64, SlateWidgetState>) -> SlateDashboard {
    let mut widgets = widgets
        .into_iter()
        .map(|(widget_id, state)| SlateWidgetSummary {
            widget_id,
            count: state.count,
            first_cycle: state.first_cycle,
            last_cycle: state.last_cycle,
        })
        .collect::<Vec<_>>();
    widgets.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.widget_id.cmp(&right.widget_id))
    });
    SlateDashboard {
        added_widgets: widgets.iter().map(|widget| widget.count).sum(),
        widgets,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TraceChannelAnnounce {
    id: u32,
    name: String,
    is_enabled: bool,
    read_only: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TraceChannelToggle {
    id: u32,
    is_enabled: bool,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct TraceChannelState {
    name: Option<String>,
    is_enabled: bool,
    read_only: bool,
    toggle_count: u64,
}

impl TraceChannelState {
    fn announce(&mut self, announce: TraceChannelAnnounce) {
        self.name = Some(announce.name);
        self.is_enabled = announce.is_enabled;
        self.read_only = announce.read_only;
    }

    fn toggle(&mut self, is_enabled: bool) {
        self.is_enabled = is_enabled;
        self.toggle_count += 1;
    }
}

fn decode_trace_channel_announce(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
) -> Result<TraceChannelAnnounce, TraceError> {
    let aux = parse_protocol5_aux(data, event_data_size(event), base_offset)?;
    Ok(TraceChannelAnnounce {
        id: read_u32_field(event, data, "Id", base_offset)?,
        is_enabled: read_u8_field(event, data, "IsEnabled", base_offset)? != 0,
        read_only: read_u8_field(event, data, "ReadOnly", base_offset)? != 0,
        name: read_aux_string(event, &aux, "Name")?,
    })
}

fn decode_trace_channel_toggle(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
) -> Result<TraceChannelToggle, TraceError> {
    Ok(TraceChannelToggle {
        id: read_u32_field(event, data, "Id", base_offset)?,
        is_enabled: read_u8_field(event, data, "IsEnabled", base_offset)? != 0,
    })
}

fn trace_channel_dashboard(channels: BTreeMap<u32, TraceChannelState>) -> TraceChannelDashboard {
    let mut summaries = channels
        .into_iter()
        .map(|(id, state)| TraceChannelSummary {
            id,
            name: state.name,
            is_enabled: state.is_enabled,
            read_only: state.read_only,
            toggle_count: state.toggle_count,
        })
        .collect::<Vec<_>>();
    summaries.sort_by(|left, right| {
        left.name
            .as_deref()
            .unwrap_or("")
            .cmp(right.name.as_deref().unwrap_or(""))
            .then_with(|| left.id.cmp(&right.id))
    });

    TraceChannelDashboard {
        count: u64::try_from(summaries.len()).unwrap(),
        enabled: u64::try_from(
            summaries
                .iter()
                .filter(|channel| channel.is_enabled)
                .count(),
        )
        .unwrap(),
        read_only: u64::try_from(summaries.iter().filter(|channel| channel.read_only).count())
            .unwrap(),
        toggles: summaries.iter().map(|channel| channel.toggle_count).sum(),
        channels: summaries,
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct ThreadGroupState {
    groups: BTreeMap<String, ThreadGroupCounts>,
    stack: Vec<String>,
    begin_events: u64,
    end_events: u64,
    unmatched_ends: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct ThreadGroupCounts {
    begin_count: u64,
    end_count: u64,
}

impl ThreadGroupState {
    fn begin(&mut self, name: String) {
        self.begin_events += 1;
        self.groups.entry(name.clone()).or_default().begin_count += 1;
        self.stack.push(name);
    }

    fn end(&mut self) {
        self.end_events += 1;
        let Some(name) = self.stack.pop() else {
            self.unmatched_ends += 1;
            return;
        };
        self.groups.entry(name).or_default().end_count += 1;
    }

    fn dashboard(self) -> ThreadGroupDashboard {
        let mut groups = self
            .groups
            .into_iter()
            .map(|(name, counts)| ThreadGroupSummary {
                name,
                begin_count: counts.begin_count,
                end_count: counts.end_count,
                balanced: counts.begin_count == counts.end_count,
            })
            .collect::<Vec<_>>();
        groups.sort_by(|left, right| {
            right
                .begin_count
                .cmp(&left.begin_count)
                .then_with(|| left.name.cmp(&right.name))
        });

        ThreadGroupDashboard {
            begin_events: self.begin_events,
            end_events: self.end_events,
            unmatched_ends: self.unmatched_ends,
            unclosed_groups: u64::try_from(self.stack.len()).unwrap(),
            groups,
        }
    }
}

fn decode_thread_group_begin(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
) -> Result<String, TraceError> {
    let aux = parse_protocol5_aux(data, event_data_size(event), base_offset)?;
    read_aux_string(event, &aux, "Name")
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct BookmarkSpec {
    bookmark_point: u64,
    format_string: String,
    file: Option<String>,
    line: Option<i32>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct BookmarkState {
    count: u64,
    format_args_bytes: u64,
    sample_args: Vec<String>,
    sample_message: Option<String>,
    first_cycle: Option<u64>,
    last_cycle: Option<u64>,
    callstack_count: u64,
    callstack_samples: Vec<BookmarkCallstackSample>,
}

const MAX_BOOKMARK_CALLSTACK_SAMPLES: usize = 8;

#[derive(Clone, Debug, Eq, PartialEq)]
struct OpenRegion {
    name: String,
    category: Option<String>,
    cycle: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct RegionTotal {
    count: u64,
    total_cycles: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct RegionState {
    begin_events: u64,
    end_events: u64,
    completed: u64,
    unmatched_ends: u64,
    with_id_begin_events: u64,
    with_id_end_events: u64,
    open_by_name: BTreeMap<String, Vec<OpenRegion>>,
    open_by_id: BTreeMap<u64, OpenRegion>,
    totals: BTreeMap<(String, Option<String>), RegionTotal>,
}

fn decode_bookmark_spec(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
) -> Result<BookmarkSpec, TraceError> {
    let aux = parse_protocol5_aux(data, event_data_size(event), base_offset)?;
    Ok(BookmarkSpec {
        bookmark_point: read_pointer_field(event, data, "BookmarkPoint", base_offset)?,
        format_string: optional_aux_text(event, &aux, "FormatString")?.unwrap_or_default(),
        file: optional_aux_text(event, &aux, "FileName")?.filter(|file| !file.is_empty()),
        line: Some(read_i32_field(event, data, "Line", base_offset)?).filter(|line| *line != 0),
    })
}

fn decode_misc_annotation_event(
    event: &EventTypeInfo,
    data: &[u8],
    bookmark_specs: &BTreeMap<u64, BookmarkSpec>,
    bookmark_states: &mut BTreeMap<u64, BookmarkState>,
    unresolved_bookmark_events: &mut u64,
    region_state: &mut RegionState,
    base_offset: u64,
) -> Result<(), TraceError> {
    match event.event.as_str() {
        "Bookmark" => {
            let bookmark_point = read_pointer_field(event, data, "BookmarkPoint", base_offset)?;
            let cycle = read_u64_field(event, data, "Cycle", base_offset)?;
            let callstack_id = read_u32_field(event, data, "CallstackId", base_offset)?;
            if !bookmark_specs.contains_key(&bookmark_point) {
                *unresolved_bookmark_events += 1;
            }
            let state = bookmark_states.entry(bookmark_point).or_default();
            state.count += 1;
            if let Some(format_args) = read_aux_bytes(event, data, "FormatArgs", base_offset)? {
                state.format_args_bytes = state
                    .format_args_bytes
                    .saturating_add(u64::try_from(format_args.len()).unwrap());
                if state.sample_message.is_none() {
                    let format_string = bookmark_specs
                        .get(&bookmark_point)
                        .map(|spec| spec.format_string.as_str())
                        .unwrap_or("");
                    if let Some(message) = render_format_message(format_string, &format_args) {
                        state.sample_message = Some(message);
                    }
                    let decoded = crate::utrace_format_args::decode_format_args(&format_args);
                    if !decoded.args.is_empty() {
                        state.sample_args = format_arg_display_strings(&decoded.args);
                    }
                }
            }
            if state.first_cycle.is_none() {
                state.first_cycle = Some(cycle);
            }
            state.last_cycle = Some(cycle);
            if callstack_id != 0 {
                state.callstack_count += 1;
                if state.callstack_samples.len() < MAX_BOOKMARK_CALLSTACK_SAMPLES {
                    state.callstack_samples.push(BookmarkCallstackSample {
                        cycle,
                        callstack_id,
                        callstack: CallstackResolution::Missing,
                    });
                }
            }
        }
        "RegionBegin" => {
            let aux = parse_protocol5_aux(data, event_data_size(event), base_offset)?;
            let cycle = read_u64_field(event, data, "Cycle", base_offset)?;
            let name = optional_aux_text(event, &aux, "RegionName")?.unwrap_or_default();
            let category =
                optional_aux_text(event, &aux, "Category")?.filter(|category| !category.is_empty());
            region_state.begin_events += 1;
            region_state
                .open_by_name
                .entry(name.clone())
                .or_default()
                .push(OpenRegion {
                    name,
                    category,
                    cycle,
                });
        }
        "RegionBeginWithId" => {
            let aux = parse_protocol5_aux(data, event_data_size(event), base_offset)?;
            let cycle_and_id = read_u64_field(event, data, "CycleAndId", base_offset)?;
            let name = optional_aux_text(event, &aux, "RegionName")?.unwrap_or_default();
            let category =
                optional_aux_text(event, &aux, "Category")?.filter(|category| !category.is_empty());
            region_state.begin_events += 1;
            region_state.with_id_begin_events += 1;
            region_state.open_by_id.insert(
                cycle_and_id,
                OpenRegion {
                    name,
                    category,
                    cycle: cycle_and_id,
                },
            );
        }
        "RegionEnd" => {
            let aux = parse_protocol5_aux(data, event_data_size(event), base_offset)?;
            let cycle = read_u64_field(event, data, "Cycle", base_offset)?;
            let name = optional_aux_text(event, &aux, "RegionName")?.unwrap_or_default();
            region_state.end_events += 1;
            let Some(stack) = region_state.open_by_name.get_mut(&name) else {
                region_state.unmatched_ends += 1;
                return Ok(());
            };
            let Some(open) = stack.pop() else {
                region_state.unmatched_ends += 1;
                return Ok(());
            };
            record_region(region_state, open, cycle);
        }
        "RegionEndWithId" => {
            let cycle = read_u64_field(event, data, "Cycle", base_offset)?;
            let region_id = read_u64_field(event, data, "RegionId", base_offset)?;
            region_state.end_events += 1;
            region_state.with_id_end_events += 1;
            let Some(open) = region_state.open_by_id.remove(&region_id) else {
                region_state.unmatched_ends += 1;
                return Ok(());
            };
            record_region(region_state, open, cycle);
        }
        _ => {}
    }
    Ok(())
}

fn record_region(region_state: &mut RegionState, open: OpenRegion, end_cycle: u64) {
    let total = region_state
        .totals
        .entry((open.name, open.category))
        .or_default();
    total.count += 1;
    total.total_cycles = total
        .total_cycles
        .saturating_add(end_cycle.saturating_sub(open.cycle));
    region_state.completed += 1;
}

fn apply_callstack_resolutions(
    memory: &mut MemoryDashboard,
    bookmarks: &mut BookmarkDashboard,
    callstacks: &mut CallstackProvider,
) {
    for sample in &mut memory.allocs.samples {
        let resolution = callstacks.resolve(CallstackId(sample.callstack_id));
        sample.callstack = resolution;
        if matches!(
            resolution,
            CallstackResolution::Missing | CallstackResolution::CatalogTruncated
        ) {
            callstacks.note_unresolved_reference();
        }
    }
    for bookmark in &mut bookmarks.bookmarks {
        for sample in &mut bookmark.callstack_samples {
            let resolution = callstacks.resolve(CallstackId(sample.callstack_id));
            sample.callstack = resolution;
            if matches!(
                resolution,
                CallstackResolution::Missing | CallstackResolution::CatalogTruncated
            ) {
                callstacks.note_unresolved_reference();
            }
        }
    }
}

fn annotation_dashboard(
    bookmark_specs: BTreeMap<u64, BookmarkSpec>,
    bookmark_states: BTreeMap<u64, BookmarkState>,
    unresolved_bookmark_events: u64,
    region_state: RegionState,
) -> AnnotationDashboard {
    AnnotationDashboard {
        bookmarks: bookmark_dashboard(bookmark_specs, bookmark_states, unresolved_bookmark_events),
        regions: region_dashboard(region_state),
    }
}

fn bookmark_dashboard(
    specs: BTreeMap<u64, BookmarkSpec>,
    states: BTreeMap<u64, BookmarkState>,
    unresolved_events: u64,
) -> BookmarkDashboard {
    let mut bookmarks = specs
        .into_iter()
        .map(|(bookmark_point, spec)| {
            let state = states.get(&bookmark_point).cloned().unwrap_or_default();
            BookmarkSummary {
                bookmark_point,
                format_string: spec.format_string,
                file: spec.file,
                line: spec.line,
                count: state.count,
                format_args_bytes: state.format_args_bytes,
                sample_args: state.sample_args,
                sample_message: state.sample_message,
                first_cycle: state.first_cycle,
                last_cycle: state.last_cycle,
                callstack_count: state.callstack_count,
                callstack_samples: state.callstack_samples,
            }
        })
        .collect::<Vec<_>>();
    bookmarks.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.format_string.cmp(&right.format_string))
            .then_with(|| left.bookmark_point.cmp(&right.bookmark_point))
    });

    BookmarkDashboard {
        specs: u64::try_from(bookmarks.len()).unwrap(),
        events: bookmarks.iter().map(|bookmark| bookmark.count).sum(),
        format_args_bytes: bookmarks
            .iter()
            .map(|bookmark| bookmark.format_args_bytes)
            .sum(),
        unresolved_events,
        bookmarks,
    }
}

fn region_dashboard(state: RegionState) -> RegionDashboard {
    let unterminated = state
        .open_by_name
        .values()
        .map(|regions| u64::try_from(regions.len()).unwrap())
        .sum::<u64>()
        + u64::try_from(state.open_by_id.len()).unwrap();
    let mut regions = state
        .totals
        .into_iter()
        .map(|((name, category), total)| RegionSummary {
            name,
            category,
            count: total.count,
            total_cycles: total.total_cycles,
        })
        .collect::<Vec<_>>();
    regions.sort_by(|left, right| {
        right
            .total_cycles
            .cmp(&left.total_cycles)
            .then_with(|| right.count.cmp(&left.count))
            .then_with(|| left.name.cmp(&right.name))
    });

    RegionDashboard {
        begin_events: state.begin_events,
        end_events: state.end_events,
        completed: state.completed,
        unmatched_ends: state.unmatched_ends,
        unterminated,
        with_id_begin_events: state.with_id_begin_events,
        with_id_end_events: state.with_id_end_events,
        regions,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LogCategoryRec {
    name: String,
    default_verbosity: u8,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LogMessageSpecRec {
    category_pointer: u64,
    line: Option<i32>,
    verbosity: u8,
    file: Option<String>,
    format_string: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct LogMessageState {
    count: u64,
    format_args_bytes: u64,
    sample_args: Vec<String>,
    sample_message: Option<String>,
    first_cycle: Option<u64>,
    last_cycle: Option<u64>,
}

fn decode_log_category(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
) -> Result<(u64, LogCategoryRec), TraceError> {
    let aux = parse_protocol5_aux(data, event_data_size(event), base_offset)?;
    let pointer = read_pointer_field(event, data, "CategoryPointer", base_offset)?;
    let category = LogCategoryRec {
        name: optional_aux_text(event, &aux, "Name")?.unwrap_or_default(),
        default_verbosity: read_u8_field(event, data, "DefaultVerbosity", base_offset)?,
    };
    Ok((pointer, category))
}

fn decode_log_message_spec(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
) -> Result<(u64, LogMessageSpecRec), TraceError> {
    let aux = parse_protocol5_aux(data, event_data_size(event), base_offset)?;
    let log_point = read_pointer_field(event, data, "LogPoint", base_offset)?;
    let spec = LogMessageSpecRec {
        category_pointer: read_pointer_field(event, data, "CategoryPointer", base_offset)?,
        line: Some(read_i32_field(event, data, "Line", base_offset)?).filter(|line| *line != 0),
        verbosity: read_u8_field(event, data, "Verbosity", base_offset)?,
        file: optional_aux_text(event, &aux, "FileName")?.filter(|file| !file.is_empty()),
        format_string: optional_aux_text(event, &aux, "FormatString")?.unwrap_or_default(),
    };
    Ok((log_point, spec))
}

fn decode_log_message(
    event: &EventTypeInfo,
    data: &[u8],
    specs: &BTreeMap<u64, LogMessageSpecRec>,
    states: &mut BTreeMap<u64, LogMessageState>,
    unresolved_messages: &mut u64,
    base_offset: u64,
) -> Result<(), TraceError> {
    let log_point = read_pointer_field(event, data, "LogPoint", base_offset)?;
    let cycle = read_u64_field(event, data, "Cycle", base_offset)?;
    if !specs.contains_key(&log_point) {
        *unresolved_messages += 1;
    }
    let state = states.entry(log_point).or_default();
    state.count += 1;
    if let Some(format_args) = read_aux_bytes(event, data, "FormatArgs", base_offset)? {
        state.format_args_bytes = state
            .format_args_bytes
            .saturating_add(u64::try_from(format_args.len()).unwrap());
        if state.sample_args.is_empty() && state.sample_message.is_none() {
            let format_string = specs
                .get(&log_point)
                .map(|spec| spec.format_string.as_str())
                .unwrap_or("%s");
            if let Some(message) = render_format_message(format_string, &format_args) {
                state.sample_message = Some(message);
            }
            let decoded = crate::utrace_format_args::decode_format_args(&format_args);
            if !decoded.args.is_empty() {
                state.sample_args = format_arg_display_strings(&decoded.args);
            } else {
                // Preserve prior heuristic sample-arg extraction for non-typed blobs.
                state.sample_args = decode_log_format_args(&format_args);
            }
        }
    }
    if state.first_cycle.is_none() {
        state.first_cycle = Some(cycle);
    }
    state.last_cycle = Some(cycle);
    Ok(())
}

fn decode_log_format_args(bytes: &[u8]) -> Vec<String> {
    let strings = extract_utf16_strings(bytes);
    if !strings.is_empty() {
        return strings;
    }
    extract_ascii_strings(bytes)
}

fn decode_metadata_field_names(bytes: &[u8]) -> Vec<String> {
    let strings = decode_cbor_text_strings(bytes);
    if !strings.is_empty() {
        return strings;
    }
    let strings = extract_utf16_strings(bytes);
    if !strings.is_empty() {
        return strings;
    }
    extract_ascii_strings(bytes)
}

fn decode_cbor_text_strings(bytes: &[u8]) -> Vec<String> {
    let mut strings = decode_cbor_report(bytes)
        .values
        .into_iter()
        .flat_map(|value| metadata_value_strings(&value))
        .collect::<Vec<_>>();
    if !strings.is_empty() {
        return strings;
    }

    let mut index = 0;
    while index < bytes.len() {
        let Some((text_start, text_len, next_index)) = cbor_text_span(bytes, index) else {
            index += 1;
            continue;
        };
        let text_end = text_start + text_len;
        if let Ok(text) = std::str::from_utf8(&bytes[text_start..text_end]) {
            if !text.is_empty() {
                strings.push(text.to_owned());
            }
        }
        index = next_index.max(index + 1);
    }
    strings
}

fn decode_cbor_report(bytes: &[u8]) -> CborDecodeReport {
    const MAX_VALUES: usize = 64;

    let mut values = Vec::new();
    let mut consumed_bytes = 0;
    let mut skipped_bytes = 0;
    let mut failed_reads = 0;
    let mut cursor = 0;
    while cursor < bytes.len() && values.len() < MAX_VALUES {
        let mut reader = CborReader::new(bytes, cursor);
        match reader.read_value(0) {
            Some(value) if reader.cursor > cursor => {
                consumed_bytes += reader.cursor - cursor;
                values.push(value);
                cursor = reader.cursor;
            }
            _ => {
                failed_reads += 1;
                skipped_bytes += 1;
                cursor += 1;
            }
        }
    }
    if cursor < bytes.len() {
        skipped_bytes += bytes.len() - cursor;
    }
    CborDecodeReport {
        values,
        consumed_bytes,
        skipped_bytes,
        failed_reads,
    }
}

fn metadata_value_strings(value: &MetadataValue) -> Vec<String> {
    match value {
        MetadataValue::Text { value } if !value.is_empty() => vec![value.clone()],
        MetadataValue::Array { values } => values.iter().flat_map(metadata_value_strings).collect(),
        MetadataValue::Map { entries } => entries
            .iter()
            .flat_map(|entry| {
                metadata_value_strings(&entry.key)
                    .into_iter()
                    .chain(metadata_value_strings(&entry.value))
            })
            .collect(),
        _ => Vec::new(),
    }
}

struct CborReader<'a> {
    bytes: &'a [u8],
    cursor: usize,
}

impl<'a> CborReader<'a> {
    const MAX_DEPTH: usize = 8;
    const MAX_CONTAINER_ITEMS: usize = 64;

    const fn new(bytes: &'a [u8], cursor: usize) -> Self {
        Self { bytes, cursor }
    }

    fn checked_container_capacity(len: usize) -> Option<usize> {
        (len <= Self::MAX_CONTAINER_ITEMS).then_some(len)
    }

    fn read_value(&mut self, depth: usize) -> Option<MetadataValue> {
        if depth > Self::MAX_DEPTH {
            return None;
        }
        let initial = *self.bytes.get(self.cursor)?;
        self.cursor += 1;
        let major = initial >> 5;
        let additional = initial & 0x1f;

        match major {
            0 => self
                .read_argument(additional)
                .map(|value| MetadataValue::Unsigned { value }),
            1 => self.read_argument(additional).and_then(|value| {
                let signed = -1_i128 - i128::from(value);
                i64::try_from(signed)
                    .ok()
                    .map(|value| MetadataValue::Signed { value })
            }),
            2 => {
                let len = usize::try_from(self.read_argument(additional)?).ok()?;
                let start = self.cursor;
                let end = start.checked_add(len)?;
                if end > self.bytes.len() {
                    return None;
                }
                self.cursor = end;
                Some(MetadataValue::Bytes {
                    byte_len: len,
                    hex_prefix: hex_prefix(&self.bytes[start..end], 32),
                })
            }
            3 => {
                let len = usize::try_from(self.read_argument(additional)?).ok()?;
                let start = self.cursor;
                let end = start.checked_add(len)?;
                if end > self.bytes.len() {
                    return None;
                }
                self.cursor = end;
                std::str::from_utf8(&self.bytes[start..end])
                    .ok()
                    .map(|value| MetadataValue::Text {
                        value: value.to_owned(),
                    })
            }
            4 => {
                let len = usize::try_from(self.read_argument(additional)?).ok()?;
                let capacity = Self::checked_container_capacity(len)?;
                let mut values = Vec::with_capacity(capacity);
                for _ in 0..len {
                    values.push(self.read_value(depth + 1)?);
                }
                Some(MetadataValue::Array { values })
            }
            5 => {
                let len = usize::try_from(self.read_argument(additional)?).ok()?;
                let capacity = Self::checked_container_capacity(len)?;
                let mut entries = Vec::with_capacity(capacity);
                for _ in 0..len {
                    entries.push(MetadataMapEntry {
                        key: self.read_value(depth + 1)?,
                        value: self.read_value(depth + 1)?,
                    });
                }
                Some(MetadataValue::Map { entries })
            }
            7 => match additional {
                20 => Some(MetadataValue::Bool { value: false }),
                21 => Some(MetadataValue::Bool { value: true }),
                22 | 23 => Some(MetadataValue::Null),
                26 => {
                    let raw = self.read_exact(4)?;
                    Some(MetadataValue::Float {
                        value: f64::from(f32::from_bits(u32::from_be_bytes(raw.try_into().ok()?))),
                    })
                }
                27 => {
                    let raw = self.read_exact(8)?;
                    Some(MetadataValue::Float {
                        value: f64::from_bits(u64::from_be_bytes(raw.try_into().ok()?)),
                    })
                }
                _ => Some(MetadataValue::Unknown {
                    kind: "simple",
                    byte_len: 1,
                }),
            },
            _ => None,
        }
    }

    fn read_argument(&mut self, additional: u8) -> Option<u64> {
        match additional {
            0..=23 => Some(u64::from(additional)),
            24 => Some(u64::from(*self.read_exact(1)?.first()?)),
            25 => Some(u64::from(u16::from_be_bytes(
                self.read_exact(2)?.try_into().ok()?,
            ))),
            26 => Some(u64::from(u32::from_be_bytes(
                self.read_exact(4)?.try_into().ok()?,
            ))),
            27 => Some(u64::from_be_bytes(self.read_exact(8)?.try_into().ok()?)),
            _ => None,
        }
    }

    fn read_exact(&mut self, len: usize) -> Option<&'a [u8]> {
        let end = self.cursor.checked_add(len)?;
        let bytes = self.bytes.get(self.cursor..end)?;
        self.cursor = end;
        Some(bytes)
    }
}

fn cbor_text_span(bytes: &[u8], index: usize) -> Option<(usize, usize, usize)> {
    let byte = *bytes.get(index)?;
    if byte >> 5 != 3 {
        return None;
    }
    let additional = byte & 0x1f;
    let (length, text_start) = match additional {
        0..=23 => (usize::from(additional), index + 1),
        24 => (usize::from(*bytes.get(index + 1)?), index + 2),
        25 => {
            let raw = bytes.get(index + 1..index + 3)?;
            (
                usize::from(u16::from_be_bytes(raw.try_into().ok()?)),
                index + 3,
            )
        }
        _ => return None,
    };
    let text_end = text_start.checked_add(length)?;
    (text_end <= bytes.len()).then_some((text_start, length, text_end))
}

fn extract_utf16_strings(bytes: &[u8]) -> Vec<String> {
    let mut strings = Vec::new();
    let mut index = 0;
    while index + 1 < bytes.len() {
        let start = index;
        let mut units = Vec::new();
        while index + 1 < bytes.len() {
            let unit = u16::from_le_bytes([bytes[index], bytes[index + 1]]);
            if unit == 0 {
                index += 2;
                break;
            }
            if !(0x20..=0x7e).contains(&unit) && unit != 0x09 {
                break;
            }
            units.push(unit);
            index += 2;
        }
        if units.len() >= 4 {
            if let Ok(value) = String::from_utf16(&units) {
                strings.push(value);
            }
        }
        index = (index.max(start + 1)).min(bytes.len());
    }
    strings
}

fn extract_ascii_strings(bytes: &[u8]) -> Vec<String> {
    let mut strings = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        while index < bytes.len() && !(0x20..=0x7e).contains(&bytes[index]) {
            index += 1;
        }
        let start = index;
        while index < bytes.len() && (0x20..=0x7e).contains(&bytes[index]) {
            index += 1;
        }
        if index.saturating_sub(start) >= 4 {
            strings.push(String::from_utf8_lossy(&bytes[start..index]).into_owned());
        }
    }
    strings
}

fn render_log_sample(format_string: &str, args: &[String]) -> Option<String> {
    if args.is_empty() {
        return None;
    }
    let mut rendered = format_string.to_owned();
    for arg in args {
        if let Some(index) = rendered.find("%s") {
            rendered.replace_range(index..index + 2, arg);
        } else {
            return None;
        }
    }
    Some(rendered)
}

fn decode_session(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
) -> Result<SessionInfo, TraceError> {
    let aux = parse_protocol5_aux(data, event_data_size(event), base_offset)?;
    let instance_id = read_aux_bytes(event, data, "InstanceId", base_offset)?
        .filter(|bytes| !bytes.is_empty())
        .map(|bytes| format_guid_bytes(&bytes));
    let vfs_paths = optional_aux_text(event, &aux, "VFSPaths")?
        .map(|paths| {
            paths
                .split(';')
                .filter(|path| !path.is_empty())
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(SessionInfo {
        platform: optional_aux_text(event, &aux, "Platform")?.unwrap_or_default(),
        app_name: optional_aux_text(event, &aux, "AppName")?.unwrap_or_default(),
        project_name: optional_aux_text(event, &aux, "ProjectName")?.unwrap_or_default(),
        command_line: optional_aux_text(event, &aux, "CommandLine")?.unwrap_or_default(),
        branch: optional_aux_text(event, &aux, "Branch")?.unwrap_or_default(),
        build_version: optional_aux_text(event, &aux, "BuildVersion")?.unwrap_or_default(),
        changelist: read_u32_field(event, data, "Changelist", base_offset)?,
        configuration: build_configuration(read_u8_field(
            event,
            data,
            "ConfigurationType",
            base_offset,
        )?),
        target_type: build_target_type(read_u8_field(event, data, "TargetType", base_offset)?),
        instance_id,
        vfs_paths,
    })
}

fn log_dashboard(
    categories: BTreeMap<u64, LogCategoryRec>,
    specs: BTreeMap<u64, LogMessageSpecRec>,
    states: BTreeMap<u64, LogMessageState>,
    unresolved_messages: u64,
) -> LogDashboard {
    let mut category_totals = BTreeMap::<u64, (u64, u64)>::new();
    let mut verbosity_totals = BTreeMap::<u8, (u64, u64)>::new();
    let mut specs_with_unknown_category = 0_u64;
    let mut top_messages = Vec::with_capacity(specs.len());

    for (log_point, spec) in &specs {
        let state = states.get(log_point).cloned().unwrap_or_default();
        let category = categories.get(&spec.category_pointer);
        if category.is_none() {
            specs_with_unknown_category += 1;
        }
        let category_total = category_totals.entry(spec.category_pointer).or_default();
        category_total.0 += 1;
        category_total.1 += state.count;
        let verbosity_total = verbosity_totals.entry(spec.verbosity & 0x0f).or_default();
        verbosity_total.0 += 1;
        verbosity_total.1 += state.count;
        top_messages.push(LogMessageSummary {
            log_point: *log_point,
            category: category.map(|category| category.name.clone()),
            verbosity: log_verbosity(spec.verbosity),
            format_string: spec.format_string.clone(),
            file: spec.file.clone(),
            line: spec.line,
            count: state.count,
            format_args_bytes: state.format_args_bytes,
            sample_args: state.sample_args.clone(),
            sample_message: state
                .sample_message
                .clone()
                .or_else(|| render_log_sample(&spec.format_string, &state.sample_args)),
            first_cycle: state.first_cycle,
            last_cycle: state.last_cycle,
        });
    }

    let mut top_categories = categories
        .iter()
        .map(|(pointer, category)| {
            let (message_specs, messages) = category_totals.get(pointer).copied().unwrap_or((0, 0));
            LogCategorySummary {
                name: category.name.clone(),
                default_verbosity: log_verbosity(category.default_verbosity),
                message_specs,
                messages,
            }
        })
        .collect::<Vec<_>>();
    top_categories.sort_by(|left, right| {
        right
            .messages
            .cmp(&left.messages)
            .then_with(|| right.message_specs.cmp(&left.message_specs))
            .then_with(|| left.name.cmp(&right.name))
    });
    top_categories.truncate(50);

    top_messages.sort_by(|left, right| {
        right
            .count
            .cmp(&left.count)
            .then_with(|| left.format_string.cmp(&right.format_string))
            .then_with(|| left.log_point.cmp(&right.log_point))
    });
    top_messages.truncate(50);

    let verbosity = verbosity_totals
        .into_iter()
        .map(|(raw, (message_specs, messages))| LogVerbosityCount {
            verbosity: log_verbosity(raw),
            message_specs,
            messages,
        })
        .collect();

    LogDashboard {
        categories: u64::try_from(categories.len()).unwrap(),
        message_specs: u64::try_from(specs.len()).unwrap(),
        messages: states.values().map(|state| state.count).sum(),
        format_args_bytes: states.values().map(|state| state.format_args_bytes).sum(),
        unresolved_messages,
        specs_with_unknown_category,
        verbosity,
        top_categories,
        top_messages,
    }
}

fn format_guid_bytes(bytes: &[u8]) -> String {
    if bytes.len() != 16 {
        return hex_prefix(bytes, bytes.len());
    }
    let a = u32::from_le_bytes(bytes[0..4].try_into().expect("length checked"));
    let b = u16::from_le_bytes(bytes[4..6].try_into().expect("length checked"));
    let c = u16::from_le_bytes(bytes[6..8].try_into().expect("length checked"));
    format!(
        "{a:08x}-{b:04x}-{c:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    )
}

fn log_verbosity(raw: u8) -> LogVerbosity {
    match raw & 0x0f {
        0 => LogVerbosity::NoLogging,
        1 => LogVerbosity::Fatal,
        2 => LogVerbosity::Error,
        3 => LogVerbosity::Warning,
        4 => LogVerbosity::Display,
        5 => LogVerbosity::Log,
        6 => LogVerbosity::Verbose,
        7 => LogVerbosity::VeryVerbose,
        _ => LogVerbosity::Unknown,
    }
}

fn build_configuration(raw: u8) -> BuildConfiguration {
    match raw {
        1 => BuildConfiguration::Debug,
        2 => BuildConfiguration::DebugGame,
        3 => BuildConfiguration::Development,
        4 => BuildConfiguration::Shipping,
        5 => BuildConfiguration::Test,
        _ => BuildConfiguration::Unknown,
    }
}

fn build_target_type(raw: u8) -> BuildTargetType {
    match raw {
        1 => BuildTargetType::Game,
        2 => BuildTargetType::Server,
        3 => BuildTargetType::Client,
        4 => BuildTargetType::Editor,
        5 => BuildTargetType::Program,
        _ => BuildTargetType::Unknown,
    }
}

fn update_min_max(minimum: &mut Option<u64>, maximum: &mut Option<u64>, value: u64) {
    *minimum = Some(minimum.map_or(value, |current| current.min(value)));
    *maximum = Some(maximum.map_or(value, |current| current.max(value)));
}

fn update_first_last(first: &mut Option<u64>, last: &mut Option<u64>, value: u64) {
    if first.is_none() {
        *first = Some(value);
    }
    *last = Some(value);
}

fn scope_summaries(
    totals: FxHashMap<u32, (u64, u64)>,
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

#[derive(Clone, Debug, Default, PartialEq)]
struct CpuNamedEventState {
    observed_count: u64,
    sample: Option<EventSample>,
}

impl CpuNamedEventState {
    fn record(
        &mut self,
        event: &EventTypeInfo,
        data: &[u8],
        thread_id: u16,
    ) -> Result<(), TraceError> {
        self.observed_count += 1;
        if self.sample.is_none() {
            self.sample = Some(decode_event_sample(
                event,
                &RawSample {
                    thread_id,
                    data: data.to_vec(),
                },
            )?);
        }
        Ok(())
    }
}

fn cpu_named_event_summaries(
    events: BTreeMap<String, CpuNamedEventState>,
) -> Vec<CpuNamedEventSummary> {
    let mut summaries = events
        .into_iter()
        .map(|(event, state)| CpuNamedEventSummary {
            event,
            observed_count: state.observed_count,
            sample: state.sample,
        })
        .collect::<Vec<_>>();
    summaries.sort_by(|left, right| {
        right
            .observed_count
            .cmp(&left.observed_count)
            .then_with(|| left.event.cmp(&right.event))
    });
    summaries
}

#[derive(Clone, Debug, Default, PartialEq)]
struct GenericEventState {
    observed_count: u64,
    sample: Option<EventSample>,
}

impl GenericEventState {
    fn record(
        &mut self,
        event: &EventTypeInfo,
        data: &[u8],
        thread_id: u16,
    ) -> Result<(), TraceError> {
        self.observed_count += 1;
        if self.sample.is_none() {
            self.sample = Some(decode_event_sample(
                event,
                &RawSample {
                    thread_id,
                    data: data.to_vec(),
                },
            )?);
        }
        Ok(())
    }
}

fn unmodeled_trace_dashboard(
    events: FxHashMap<u16, GenericEventState>,
    registry: &BTreeMap<u16, &EventTypeInfo>,
) -> UnmodeledTraceDashboard {
    let mut summaries = events
        .into_iter()
        .filter_map(|(uid, state)| {
            let event = registry.get(&uid).copied()?;
            Some(UnmodeledTraceEventSummary {
                logger: event.logger.clone(),
                event: event.event.clone(),
                observed_count: state.observed_count,
                sample: state.sample,
            })
        })
        .collect::<Vec<_>>();
    summaries.sort_by(|left, right| {
        right
            .observed_count
            .cmp(&left.observed_count)
            .then_with(|| left.logger.cmp(&right.logger))
            .then_with(|| left.event.cmp(&right.event))
    });
    let event_types = u64::try_from(summaries.len()).unwrap();
    let observed_events = summaries.iter().map(|event| event.observed_count).sum();
    summaries.truncate(80);
    UnmodeledTraceDashboard {
        event_types,
        observed_events,
        events: summaries,
    }
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
    scope_cycle: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ParsedNormalEvent {
    pub(super) uid: u16,
    pub(super) offset: usize,
    pub(super) total_end: usize,
    pub(super) data_start: usize,
    pub(super) data_end: usize,
    pub(super) has_aux: bool,
}

fn read_protocol5_normal_events(
    stream: &[u8],
    registry: &BTreeMap<u16, &EventTypeInfo>,
) -> Result<Vec<OwnedRawEvent>, TraceError> {
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
            scope_cycle,
        });
    }
    Ok(events)
}

pub(super) fn decode_known_scope_cycle(uid: u16, data: &[u8]) -> Option<u64> {
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

/// Lookup used by normal-stream event framing. Accepts both owned registry maps
/// (`BTreeMap<u16, EventTypeInfo>`) and borrowed views (`BTreeMap<u16, &EventTypeInfo>`).
pub(super) trait EventTypeRegistry {
    fn lookup(&self, uid: u16) -> Option<&EventTypeInfo>;
}

impl EventTypeRegistry for BTreeMap<u16, EventTypeInfo> {
    #[inline]
    fn lookup(&self, uid: u16) -> Option<&EventTypeInfo> {
        self.get(&uid)
    }
}

impl EventTypeRegistry for BTreeMap<u16, &EventTypeInfo> {
    #[inline]
    fn lookup(&self, uid: u16) -> Option<&EventTypeInfo> {
        self.get(&uid).copied()
    }
}

pub(super) fn parse_protocol5_normal_event(
    reader: &mut Reader<'_>,
    registry: &impl EventTypeRegistry,
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
        let Some(event) = registry.lookup(uid) else {
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

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct CpuMetadataSpec {
    spec_id: u32,
    name: String,
    name_format: Option<String>,
    field_names_bytes: usize,
    field_names: Vec<String>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct CborDecodeReport {
    values: Vec<MetadataValue>,
    consumed_bytes: usize,
    skipped_bytes: usize,
    failed_reads: u64,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct CpuMetadataRecord {
    metadata_id: u32,
    spec_id: u32,
    name: String,
    rendered_name: Option<String>,
    metadata_bytes: usize,
    decoded_metadata_bytes: usize,
    skipped_metadata_bytes: usize,
    decode_failed: bool,
    values: Vec<MetadataValue>,
    strings: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CpuStackEntryKind {
    PlainSpec(u32),
    Metadata {
        metadata_id: u32,
        spec_id: Option<u32>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CpuStackEntry {
    kind: CpuStackEntryKind,
    start_cycle: u64,
    accumulated_cycles: u64,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
struct CpuBatchThreadState {
    last_cycle: u64,
    stack: Vec<CpuStackEntry>,
    active_coroutine_id: Option<u64>,
    coroutine_stacks: BTreeMap<u64, Vec<CpuStackEntry>>,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct CpuMetadataIntervalState {
    rendered_scope_totals: BTreeMap<(u32, String), (u64, u64)>,
    samples: Vec<CpuMetadataIntervalSample>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CpuMetadataIntervalRecord {
    metadata_id: u32,
    spec_id: u32,
    start_cycle: u64,
    end_cycle: u64,
    duration: u64,
    attribution: CpuMetadataAttribution,
}

struct CpuBatchDecodeState<'a, 'timeline> {
    batches: &'a mut CpuBatchSummary,
    scope_totals: &'a mut FxHashMap<u32, (u64, u64)>,
    metadata_scope_totals: &'a mut FxHashMap<u32, (u64, u64)>,
    metadata_interval_state: &'a mut CpuMetadataIntervalState,
    metadata_stack_context: &'a mut CpuMetadataStackRuntimeState,
    thread_state: &'a mut CpuBatchThreadState,
    batch_base_cycle: Option<u64>,
    frame_scope_totals: &'a mut FxHashMap<u32, FxHashMap<u32, (u64, u64)>>,
    frame_cycle_bounds: &'a mut FxHashMap<u32, (u64, u64)>,
    thread_scope_totals: &'a mut FxHashMap<u32, (u64, u64)>,
    timeline: Option<&'timeline mut dyn CpuTimelineSink>,
    thread_id: u16,
    cycle_frequency: Option<u64>,
}

#[derive(Clone, Debug)]
struct GpuTimelineCollector {
    frame_number: u32,
    limit: usize,
    begin_timestamp: Option<u64>,
    end_timestamp: Option<u64>,
    interval_count: u64,
    truncated: bool,
    intervals: Vec<GpuTimelineInterval>,
}

impl GpuTimelineCollector {
    fn new(frame_number: u32, limit: usize) -> Self {
        Self {
            frame_number,
            limit,
            begin_timestamp: None,
            end_timestamp: None,
            interval_count: 0,
            truncated: false,
            intervals: Vec::new(),
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn record(
        &mut self,
        active_frame: Option<u32>,
        queue_id: u32,
        kind: GpuTimelineIntervalKind,
        spec_id: Option<u32>,
        name: String,
        start_timestamp: u64,
        end_timestamp: u64,
        duration: u64,
    ) {
        if active_frame != Some(self.frame_number) {
            return;
        }
        self.begin_timestamp = Some(
            self.begin_timestamp
                .map_or(start_timestamp, |begin| begin.min(start_timestamp)),
        );
        self.end_timestamp = Some(
            self.end_timestamp
                .map_or(end_timestamp, |end| end.max(end_timestamp)),
        );
        self.interval_count = self.interval_count.saturating_add(1);
        if self.intervals.len() >= self.limit {
            self.truncated = true;
            return;
        }
        self.intervals.push(GpuTimelineInterval {
            queue_id,
            kind,
            spec_id,
            name,
            start_timestamp,
            end_timestamp,
            duration,
        });
    }

    fn into_dashboard(self) -> GpuTimelineDashboard {
        let begin_timestamp = self.begin_timestamp.unwrap_or(0);
        GpuTimelineDashboard {
            frame_number: self.frame_number,
            begin_timestamp,
            end_timestamp: self.end_timestamp.unwrap_or(begin_timestamp),
            interval_count: self.interval_count,
            truncated: self.truncated,
            intervals: self.intervals,
        }
    }
}

#[derive(Clone, Debug)]
struct CpuTimelineCollector {
    frame_number: u32,
    limit: usize,
    begin_cycle: Option<u64>,
    end_cycle: Option<u64>,
    interval_count: u64,
    truncated: bool,
    intervals: Vec<CpuTimelineInterval>,
}

impl CpuTimelineCollector {
    fn new(frame_number: u32, limit: usize) -> Self {
        Self {
            frame_number,
            limit: limit.max(1),
            begin_cycle: None,
            end_cycle: None,
            interval_count: 0,
            truncated: false,
            intervals: Vec::new(),
        }
    }

    fn into_dashboard(self, cycle_frequency: Option<u64>) -> CpuTimelineDashboard {
        let begin_cycle = self.begin_cycle.unwrap_or(0);
        let end_cycle = self.end_cycle.unwrap_or(begin_cycle);
        CpuTimelineDashboard {
            frame_number: self.frame_number,
            begin_cycle,
            end_cycle,
            duration_seconds: cycle_frequency
                .map(|frequency| end_cycle.saturating_sub(begin_cycle) as f64 / frequency as f64),
            interval_count: self.interval_count,
            truncated: self.truncated,
            intervals: self.intervals,
        }
    }
}

impl CpuTimelineSink for CpuTimelineCollector {
    fn note(
        &mut self,
        start_cycle: u64,
        end_cycle: u64,
        active_frame: Option<u32>,
    ) -> SinkAppetite {
        if active_frame != Some(self.frame_number) {
            return SinkAppetite::Full;
        }
        self.begin_cycle = Some(
            self.begin_cycle
                .map_or(start_cycle, |begin| begin.min(start_cycle)),
        );
        self.end_cycle = Some(self.end_cycle.map_or(end_cycle, |end| end.max(end_cycle)));
        self.interval_count = self.interval_count.saturating_add(1);
        if self.intervals.len() >= self.limit {
            self.truncated = true;
            SinkAppetite::Full
        } else {
            SinkAppetite::WantsRecord
        }
    }

    fn record(&mut self, interval: CpuTimelineInterval, active_frame: Option<u32>) {
        debug_assert_eq!(active_frame, Some(self.frame_number));
        self.intervals.push(interval);
    }
}

struct CpuTimelineFanout<'a> {
    collector: &'a mut CpuTimelineCollector,
    index: &'a mut CpuTimelineIndexBuilder,
    collector_wants_record: bool,
    index_wants_record: bool,
}

impl<'a> CpuTimelineFanout<'a> {
    fn new(
        collector: &'a mut CpuTimelineCollector,
        index: &'a mut CpuTimelineIndexBuilder,
    ) -> Self {
        Self {
            collector,
            index,
            collector_wants_record: false,
            index_wants_record: false,
        }
    }
}

impl CpuTimelineSink for CpuTimelineFanout<'_> {
    fn note(
        &mut self,
        start_cycle: u64,
        end_cycle: u64,
        active_frame: Option<u32>,
    ) -> SinkAppetite {
        self.collector_wants_record =
            self.collector.note(start_cycle, end_cycle, active_frame) == SinkAppetite::WantsRecord;
        self.index_wants_record =
            self.index.note(start_cycle, end_cycle, active_frame) == SinkAppetite::WantsRecord;
        if self.collector_wants_record || self.index_wants_record {
            SinkAppetite::WantsRecord
        } else {
            SinkAppetite::Full
        }
    }

    fn record(&mut self, interval: CpuTimelineInterval, active_frame: Option<u32>) {
        match (self.collector_wants_record, self.index_wants_record) {
            (true, true) => {
                self.collector.record(interval.clone(), active_frame);
                self.index.record(interval, active_frame);
            }
            (true, false) => self.collector.record(interval, active_frame),
            (false, true) => self.index.record(interval, active_frame),
            (false, false) => debug_assert!(false, "record called without a sink appetite"),
        }
    }
}

enum CpuTimelineSinks<'a> {
    Collector(&'a mut CpuTimelineCollector),
    Index(&'a mut CpuTimelineIndexBuilder),
    Both(CpuTimelineFanout<'a>),
}

impl CpuTimelineSink for CpuTimelineSinks<'_> {
    fn note(
        &mut self,
        start_cycle: u64,
        end_cycle: u64,
        active_frame: Option<u32>,
    ) -> SinkAppetite {
        match self {
            Self::Collector(collector) => collector.note(start_cycle, end_cycle, active_frame),
            Self::Index(index) => index.note(start_cycle, end_cycle, active_frame),
            Self::Both(fanout) => fanout.note(start_cycle, end_cycle, active_frame),
        }
    }

    fn record(&mut self, interval: CpuTimelineInterval, active_frame: Option<u32>) {
        match self {
            Self::Collector(collector) => collector.record(interval, active_frame),
            Self::Index(index) => index.record(interval, active_frame),
            Self::Both(fanout) => fanout.record(interval, active_frame),
        }
    }
}

fn gpu_submission_latency(
    mut samples: Vec<GpuSubmissionLatencySample>,
) -> Option<GpuSubmissionLatency> {
    if samples.is_empty() {
        return None;
    }
    let sample_count = u64::try_from(samples.len()).unwrap_or(u64::MAX);
    let mut delays = samples
        .iter()
        .map(|sample| sample.delay_cycles)
        .collect::<Vec<_>>();
    delays.sort_unstable();
    let median_delay_cycles = delays[delays.len() / 2];
    let min_delay_cycles = *delays.first().unwrap();
    let max_delay_cycles = *delays.last().unwrap();
    if samples.len() > 16 {
        samples.truncate(16);
    }
    Some(GpuSubmissionLatency {
        sample_count,
        median_delay_cycles,
        min_delay_cycles,
        max_delay_cycles,
        samples,
    })
}

fn submission_delay_cycles(gpu_timestamp_top: u64, cpu_submit_timestamp: u64) -> i128 {
    i128::from(gpu_timestamp_top) - i128::from(cpu_submit_timestamp)
}

fn decode_cpu_metadata_spec(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
) -> Result<CpuMetadataSpec, TraceError> {
    let aux = parse_protocol5_aux(data, event_data_size(event), base_offset)?;
    let spec_id = read_u32_field(event, data, "Id", base_offset)?;
    let name = read_aux_string(event, &aux, "Name")?;
    let name_format = optional_aux_text(event, &aux, "NameFormat")?.unwrap_or_default();
    let field_names_bytes = aux_bytes_len(event, &aux, "FieldNames");
    let field_names = read_aux_bytes(event, data, "FieldNames", base_offset)?
        .map(|bytes| decode_metadata_field_names(&bytes))
        .unwrap_or_default();
    let (name, name_format) = normalize_gpu_breadcrumb_name(name, name_format);
    Ok(CpuMetadataSpec {
        spec_id,
        name,
        name_format,
        field_names_bytes,
        field_names,
    })
}

fn decode_cpu_metadata_record(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
) -> Result<CpuMetadataRecord, TraceError> {
    let aux = parse_protocol5_aux(data, event_data_size(event), base_offset)?;
    let metadata = read_aux_bytes(event, data, "Metadata", base_offset)?.unwrap_or_default();
    let report = decode_cbor_report(&metadata);
    let mut strings = report
        .values
        .iter()
        .flat_map(metadata_value_strings)
        .collect::<Vec<_>>();
    strings.sort();
    strings.dedup();
    Ok(CpuMetadataRecord {
        metadata_id: read_u32_field(event, data, "Id", base_offset)?,
        spec_id: read_u32_field(event, data, "SpecId", base_offset)?,
        name: String::new(),
        rendered_name: None,
        metadata_bytes: aux_bytes_len(event, &aux, "Metadata"),
        decoded_metadata_bytes: report.consumed_bytes,
        skipped_metadata_bytes: report.skipped_bytes,
        decode_failed: report.failed_reads > 0,
        values: report.values,
        strings,
    })
}

fn enrich_cpu_metadata_record(
    specs: &BTreeMap<u32, CpuMetadataSpec>,
    record: &mut CpuMetadataRecord,
) {
    let spec = specs.get(&record.spec_id);
    record.name = spec
        .map(|spec| spec.name.clone())
        .unwrap_or_else(|| format!("#{}", record.spec_id));
    record.rendered_name = spec.and_then(|spec| render_metadata_name(spec, &record.values));
}

fn cpu_metadata_dashboard(
    specs: &BTreeMap<u32, CpuMetadataSpec>,
    records: &BTreeMap<u32, CpuMetadataRecord>,
    totals: FxHashMap<u32, (u64, u64)>,
    interval_state: CpuMetadataIntervalState,
    total_metadata_scopes: u64,
) -> CpuMetadataDashboard {
    let resolved_scopes = totals.values().map(|(count, _)| *count).sum();
    let mut top = totals
        .iter()
        .map(|(&spec_id, &(count, total_cycles))| {
            let name = specs
                .get(&spec_id)
                .map(|spec| spec.name.clone())
                .unwrap_or_else(|| format!("#{spec_id}"));
            CpuMetadataScopeSummary {
                spec_id,
                name,
                count,
                total_cycles,
            }
        })
        .collect::<Vec<_>>();
    top.sort_by(|left, right| {
        right
            .total_cycles
            .cmp(&left.total_cycles)
            .then_with(|| right.count.cmp(&left.count))
            .then_with(|| left.spec_id.cmp(&right.spec_id))
    });
    top.truncate(40);

    let mut field_names = specs
        .values()
        .flat_map(|spec| spec.field_names.iter().cloned())
        .collect::<Vec<_>>();
    field_names.sort();
    field_names.dedup();

    let mut strings = records
        .values()
        .flat_map(|record| record.strings.iter().cloned())
        .collect::<Vec<_>>();
    strings.sort();
    strings.dedup();
    strings.truncate(40);

    let spec_summaries = cpu_metadata_spec_summaries(specs, records, &totals);
    let samples = spec_summaries
        .iter()
        .filter_map(|summary| summary.sample.clone())
        .collect();
    let rendered_scopes =
        cpu_metadata_rendered_scope_summaries(specs, interval_state.rendered_scope_totals);

    CpuMetadataDashboard {
        specs: u64::try_from(specs.len()).unwrap(),
        specs_with_name_format: u64::try_from(
            specs
                .values()
                .filter(|spec| spec.name_format.is_some())
                .count(),
        )
        .unwrap(),
        field_names_bytes: specs
            .values()
            .map(|spec| u64::try_from(spec.field_names_bytes).unwrap())
            .sum(),
        field_names,
        records: u64::try_from(records.len()).unwrap(),
        metadata_bytes: records
            .values()
            .map(|record| u64::try_from(record.metadata_bytes).unwrap())
            .sum(),
        scopes: total_metadata_scopes,
        resolved_scopes,
        unresolved_scopes: total_metadata_scopes.saturating_sub(resolved_scopes),
        decoded_records: records
            .values()
            .filter(|record| !record.values.is_empty())
            .count()
            .try_into()
            .unwrap(),
        decoded_values: records
            .values()
            .map(|record| u64::try_from(record.values.len()).unwrap())
            .sum(),
        decoded_metadata_bytes: records
            .values()
            .map(|record| u64::try_from(record.decoded_metadata_bytes).unwrap())
            .sum(),
        undecoded_records: records
            .values()
            .filter(|record| record.values.is_empty() && record.metadata_bytes > 0)
            .count()
            .try_into()
            .unwrap(),
        decode_failed_records: records
            .values()
            .filter(|record| record.decode_failed)
            .count()
            .try_into()
            .unwrap(),
        undecoded_metadata_bytes: records
            .values()
            .map(|record| u64::try_from(record.skipped_metadata_bytes).unwrap())
            .sum(),
        strings,
        samples,
        spec_summaries,
        rendered_scopes,
        interval_samples: interval_state.samples,
        top,
    }
}

fn cpu_metadata_rendered_scope_summaries(
    specs: &BTreeMap<u32, CpuMetadataSpec>,
    totals: BTreeMap<(u32, String), (u64, u64)>,
) -> Vec<CpuMetadataRenderedScopeSummary> {
    let mut summaries = totals
        .into_iter()
        .map(
            |((spec_id, rendered_name), (count, total_cycles))| CpuMetadataRenderedScopeSummary {
                spec_id,
                name: specs
                    .get(&spec_id)
                    .map(|spec| spec.name.clone())
                    .unwrap_or_else(|| format!("#{spec_id}")),
                rendered_name,
                count,
                total_cycles,
            },
        )
        .collect::<Vec<_>>();
    summaries.sort_by(|left, right| {
        right
            .total_cycles
            .cmp(&left.total_cycles)
            .then_with(|| right.count.cmp(&left.count))
            .then_with(|| left.spec_id.cmp(&right.spec_id))
            .then_with(|| left.rendered_name.cmp(&right.rendered_name))
    });
    summaries.truncate(40);
    summaries
}

#[derive(Clone, Debug, Default, PartialEq)]
struct CpuMetadataSpecState {
    records: u64,
    metadata_bytes: u64,
    decoded_records: u64,
    decoded_values: u64,
    decoded_metadata_bytes: u64,
    undecoded_records: u64,
    decode_failed_records: u64,
    undecoded_metadata_bytes: u64,
    scopes: u64,
    total_cycles: u64,
    strings: BTreeSet<String>,
    rendered_names: BTreeSet<String>,
    sample: Option<CpuMetadataSample>,
}

fn cpu_metadata_spec_summaries(
    specs: &BTreeMap<u32, CpuMetadataSpec>,
    records: &BTreeMap<u32, CpuMetadataRecord>,
    totals: &FxHashMap<u32, (u64, u64)>,
) -> Vec<CpuMetadataSpecSummary> {
    let mut states = BTreeMap::<u32, CpuMetadataSpecState>::new();

    for record in records.values() {
        let spec = specs.get(&record.spec_id);
        let state = states.entry(record.spec_id).or_default();
        state.records += 1;
        state.metadata_bytes = state
            .metadata_bytes
            .saturating_add(u64::try_from(record.metadata_bytes).unwrap());
        state.decoded_values = state
            .decoded_values
            .saturating_add(u64::try_from(record.values.len()).unwrap());
        state.decoded_metadata_bytes = state
            .decoded_metadata_bytes
            .saturating_add(u64::try_from(record.decoded_metadata_bytes).unwrap());
        state.undecoded_metadata_bytes = state
            .undecoded_metadata_bytes
            .saturating_add(u64::try_from(record.skipped_metadata_bytes).unwrap());
        if record.values.is_empty() && record.metadata_bytes > 0 {
            state.undecoded_records += 1;
        }
        if record.decode_failed {
            state.decode_failed_records += 1;
        }
        if !record.values.is_empty() {
            state.decoded_records += 1;
            if state.sample.is_none() {
                state.sample = Some(cpu_metadata_sample(spec, record));
            }
            if let Some(rendered_name) = record.rendered_name.clone() {
                state.rendered_names.insert(rendered_name);
            }
        }
        state.strings.extend(record.strings.iter().cloned());
    }

    for (&spec_id, &(scopes, total_cycles)) in totals {
        let state = states.entry(spec_id).or_default();
        state.scopes = scopes;
        state.total_cycles = total_cycles;
    }

    let mut summaries = states
        .into_iter()
        .map(|(spec_id, state)| {
            let mut strings = state.strings.into_iter().collect::<Vec<_>>();
            strings.truncate(8);
            let mut rendered_names = state.rendered_names.into_iter().collect::<Vec<_>>();
            rendered_names.truncate(8);
            CpuMetadataSpecSummary {
                spec_id,
                name: specs
                    .get(&spec_id)
                    .map(|spec| spec.name.clone())
                    .unwrap_or_else(|| format!("#{spec_id}")),
                records: state.records,
                metadata_bytes: state.metadata_bytes,
                decoded_records: state.decoded_records,
                decoded_values: state.decoded_values,
                decoded_metadata_bytes: state.decoded_metadata_bytes,
                undecoded_records: state.undecoded_records,
                decode_failed_records: state.decode_failed_records,
                undecoded_metadata_bytes: state.undecoded_metadata_bytes,
                scopes: state.scopes,
                total_cycles: state.total_cycles,
                strings,
                rendered_names,
                sample: state.sample,
            }
        })
        .collect::<Vec<_>>();

    summaries.sort_by(|left, right| {
        right
            .total_cycles
            .cmp(&left.total_cycles)
            .then_with(|| right.scopes.cmp(&left.scopes))
            .then_with(|| right.records.cmp(&left.records))
            .then_with(|| right.metadata_bytes.cmp(&left.metadata_bytes))
            .then_with(|| left.spec_id.cmp(&right.spec_id))
    });
    summaries.truncate(40);
    summaries
}

fn cpu_metadata_sample(
    spec: Option<&CpuMetadataSpec>,
    record: &CpuMetadataRecord,
) -> CpuMetadataSample {
    CpuMetadataSample {
        metadata_id: record.metadata_id,
        spec_id: record.spec_id,
        name: if record.name.is_empty() {
            spec.map(|spec| spec.name.clone())
                .unwrap_or_else(|| format!("#{}", record.spec_id))
        } else {
            record.name.clone()
        },
        rendered_name: record
            .rendered_name
            .clone()
            .or_else(|| spec.and_then(|spec| render_metadata_name(spec, &record.values))),
        fields: metadata_sample_fields(
            spec.map(|spec| spec.field_names.as_slice()).unwrap_or(&[]),
            &record.values,
        ),
    }
}

fn render_metadata_name(spec: &CpuMetadataSpec, values: &[MetadataValue]) -> Option<String> {
    render_metadata_name_parts(&spec.name, spec.name_format.as_deref(), values)
}

fn render_metadata_name_parts(
    name: &str,
    name_format: Option<&str>,
    values: &[MetadataValue],
) -> Option<String> {
    let format = name_format?;
    let flattened = flattened_metadata_values(values);
    let rendered_suffix = render_metadata_format(format, &flattened)?;
    let rendered = if rendered_suffix.starts_with([' ', '(', '[', '{', ':', '=', '-', '/']) {
        format!("{name}{rendered_suffix}")
    } else if name.is_empty() || name == "Unknown" {
        rendered_suffix
    } else {
        format!("{name} {rendered_suffix}")
    };
    (!rendered.is_empty()).then_some(rendered)
}

fn render_metadata_format(format: &str, values: &[&MetadataValue]) -> Option<String> {
    let mut rendered = String::new();
    let mut chars = format.char_indices().peekable();
    let mut value_index = 0;
    let mut replaced = false;
    let mut unresolved_placeholder = false;

    while let Some((_, character)) = chars.next() {
        if character != '%' {
            rendered.push(character);
            continue;
        }
        let Some(&(start, next)) = chars.peek() else {
            rendered.push('%');
            continue;
        };
        if next == '%' {
            chars.next();
            rendered.push('%');
            continue;
        }

        let mut end = start;
        let mut conversion = None;
        while let Some(&(index, candidate)) = chars.peek() {
            chars.next();
            end = index + candidate.len_utf8();
            if matches!(
                candidate,
                'd' | 'i' | 'u' | 'x' | 'X' | 'f' | 'F' | 'g' | 'G' | 'e' | 'E' | 's' | 'S'
            ) {
                conversion = Some(candidate);
                break;
            }
        }

        let Some(conversion) = conversion else {
            rendered.push('%');
            rendered.push_str(&format[start..end]);
            continue;
        };
        let Some(value) = values.get(value_index) else {
            unresolved_placeholder = true;
            continue;
        };
        value_index += 1;
        if let Some(text) = metadata_value_for_format(value, conversion) {
            rendered.push_str(&text);
            replaced = true;
        } else {
            unresolved_placeholder = true;
        }
    }

    (replaced && !unresolved_placeholder).then_some(rendered)
}

fn metadata_value_for_format(value: &MetadataValue, conversion: char) -> Option<String> {
    match conversion {
        's' | 'S' => Some(metadata_value_display(value)),
        'd' | 'i' | 'u' => metadata_value_integer(value).map(|value| value.to_string()),
        'x' => metadata_value_integer(value).map(|value| format!("{value:x}")),
        'X' => metadata_value_integer(value).map(|value| format!("{value:X}")),
        'f' | 'F' | 'g' | 'G' | 'e' | 'E' => metadata_value_float(value).map(|value| {
            if value.fract() == 0.0 {
                format!("{value:.1}")
            } else {
                value.to_string()
            }
        }),
        _ => None,
    }
}

fn metadata_value_display(value: &MetadataValue) -> String {
    match value {
        MetadataValue::Null => "null".to_owned(),
        MetadataValue::Bool { value } => value.to_string(),
        MetadataValue::Unsigned { value } => value.to_string(),
        MetadataValue::Signed { value } => value.to_string(),
        MetadataValue::Float { value } => value.to_string(),
        MetadataValue::Text { value } => value.clone(),
        MetadataValue::Bytes {
            byte_len,
            hex_prefix,
        } => format!("0x{hex_prefix} ({byte_len} bytes)"),
        MetadataValue::Array { values } => values
            .iter()
            .map(metadata_value_display)
            .collect::<Vec<_>>()
            .join(", "),
        MetadataValue::Map { entries } => entries
            .iter()
            .map(|entry| {
                format!(
                    "{}={}",
                    metadata_value_display(&entry.key),
                    metadata_value_display(&entry.value)
                )
            })
            .collect::<Vec<_>>()
            .join(", "),
        MetadataValue::Unknown { kind, byte_len } => format!("{kind} ({byte_len} bytes)"),
    }
}

fn metadata_value_integer(value: &MetadataValue) -> Option<i128> {
    match value {
        MetadataValue::Unsigned { value } => Some(i128::from(*value)),
        MetadataValue::Signed { value } => Some(i128::from(*value)),
        MetadataValue::Bool { value } => Some(i128::from(u8::from(*value))),
        _ => None,
    }
}

fn metadata_value_float(value: &MetadataValue) -> Option<f64> {
    match value {
        MetadataValue::Float { value } => Some(*value),
        MetadataValue::Unsigned { value } => Some(*value as f64),
        MetadataValue::Signed { value } => Some(*value as f64),
        _ => None,
    }
}

fn metadata_sample_fields(
    field_names: &[String],
    values: &[MetadataValue],
) -> BTreeMap<String, MetadataValue> {
    flattened_metadata_values(values)
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let name = field_names
                .get(index)
                .filter(|name| !name.is_empty())
                .cloned()
                .unwrap_or_else(|| format!("value_{index}"));
            (name, (*value).clone())
        })
        .collect()
}

fn flattened_metadata_values(values: &[MetadataValue]) -> Vec<&MetadataValue> {
    if values.len() == 1 {
        if let MetadataValue::Array { values } = &values[0] {
            return values.iter().collect();
        }
    }
    values.iter().collect()
}

pub(super) fn decode_frame_marker(
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
    metadata: &BTreeMap<u32, CpuMetadataRecord>,
    state: &mut CpuBatchDecodeState<'_, '_>,
) -> Result<(), TraceError> {
    state.batches.count += 1;
    let mut reader = VarintReader::new(data);
    let base_cycle = state.batch_base_cycle.unwrap_or(0);
    // Slack used to detect StartCycle-anchored preamble → absolute timeline jumps.
    // One second of cycles when frequency is known; otherwise 10M (UE default-ish).
    let preamble_slack = state.cycle_frequency.unwrap_or(10_000_000);

    while !reader.is_empty() {
        let first = reader.read_u64()?;
        state.batches.decoded_records += 1;
        let encoded_cycle = first >> 2;
        let is_relative = encoded_cycle < state.thread_state.last_cycle;
        let mut cycle = encoded_cycle;
        // Relative delta against the previous absolute cycle on this thread.
        if is_relative {
            cycle = cycle.saturating_add(state.thread_state.last_cycle);
        }
        // Late-connect / missing absolute base (Insights ProcessBufferV2).
        if cycle < base_cycle {
            cycle = cycle.saturating_add(base_cycle);
        }
        // When a cold-start thread was StartCycle-anchored (late-connect) and then
        // receives a flush-aligned absolute timestamp, open stack entries still
        // carry preamble starts near BaseCycle. Rebase them across the jump so
        // inclusive durations do not span the entire pre-frame gap.
        if base_cycle > 0
            && state.thread_state.last_cycle > 0
            // A large relative delta is a legitimate long scope interval.
            // Only a raw absolute cycle can indicate a flushed CPU buffer
            // switching from the StartCycle-anchored preamble.
            && !is_relative
            && state.thread_state.last_cycle.saturating_sub(base_cycle) < preamble_slack
            && cycle > state.thread_state.last_cycle
            && cycle.saturating_sub(state.thread_state.last_cycle) > preamble_slack
        {
            let shift = cycle.saturating_sub(state.thread_state.last_cycle);
            rebase_cpu_stack_starts(&mut state.thread_state.stack, shift);
            for suspended in state.thread_state.coroutine_stacks.values_mut() {
                rebase_cpu_stack_starts(suspended, shift);
            }
            state.batches.preamble_timeline_rebases =
                state.batches.preamble_timeline_rebases.saturating_add(1);
        }
        match first & 0b11 {
            0b00 => {
                if let Some(entry) = state.thread_state.stack.pop() {
                    let mut duration = entry
                        .accumulated_cycles
                        .saturating_add(cycle.saturating_sub(entry.start_cycle));
                    // Safety net: a single interval should not exceed a generous
                    // multiple of one second of capture when frequency is known.
                    // Primary fix is preamble rebase above; this counts leftovers.
                    let implausible_limit = preamble_slack.saturating_mul(64);
                    if duration > implausible_limit {
                        state.batches.implausible_duration_count =
                            state.batches.implausible_duration_count.saturating_add(1);
                        state.batches.implausible_duration_cycles = state
                            .batches
                            .implausible_duration_cycles
                            .saturating_add(duration);
                        duration = implausible_limit;
                    }
                    match entry.kind {
                        CpuStackEntryKind::PlainSpec(spec_id) => {
                            let total = state.scope_totals.entry(spec_id).or_insert((0, 0));
                            total.0 += 1;
                            total.1 = total.1.saturating_add(duration);
                            let thread_entry =
                                state.thread_scope_totals.entry(spec_id).or_insert((0, 0));
                            thread_entry.0 += 1;
                            thread_entry.1 = thread_entry.1.saturating_add(duration);
                            if let Some(metadata_id) =
                                state.metadata_stack_context.restored_metadata_id()
                            {
                                record_restored_cpu_metadata_scope(
                                    metadata_id,
                                    entry.start_cycle,
                                    cycle,
                                    duration,
                                    metadata,
                                    state,
                                );
                            }
                            record_cpu_frame_scope(
                                spec_id,
                                entry.start_cycle,
                                cycle,
                                duration,
                                metadata,
                                state,
                            );
                            if let Some(timeline) = state.timeline.as_mut() {
                                let active_frame =
                                    state.metadata_stack_context.active_frame_number(metadata);
                                if timeline.note(entry.start_cycle, cycle, active_frame)
                                    == SinkAppetite::WantsRecord
                                {
                                    let name = specs
                                        .get(&spec_id)
                                        .map(|spec| spec.name.clone())
                                        .unwrap_or_else(|| format!("#{spec_id}"));
                                    timeline.record(
                                        CpuTimelineInterval {
                                            thread_id: state.thread_id,
                                            spec_id,
                                            name,
                                            start_cycle: entry.start_cycle,
                                            end_cycle: cycle,
                                            duration,
                                            duration_seconds: state.cycle_frequency.map(
                                                |frequency| duration as f64 / frequency as f64,
                                            ),
                                            metadata_id: None,
                                            rendered_name: None,
                                        },
                                        active_frame,
                                    );
                                }
                            }
                        }
                        CpuStackEntryKind::Metadata {
                            metadata_id,
                            spec_id: Some(spec_id),
                        } => {
                            let total =
                                state.metadata_scope_totals.entry(spec_id).or_insert((0, 0));
                            total.0 += 1;
                            total.1 = total.1.saturating_add(duration);
                            record_cpu_metadata_interval(
                                CpuMetadataIntervalRecord {
                                    metadata_id,
                                    spec_id,
                                    start_cycle: entry.start_cycle,
                                    end_cycle: cycle,
                                    duration,
                                    attribution: CpuMetadataAttribution::Inline,
                                },
                                metadata,
                                state.metadata_interval_state,
                            );
                            if let Some(timeline) = state.timeline.as_mut() {
                                let active_frame =
                                    state.metadata_stack_context.active_frame_number(metadata);
                                if timeline.note(entry.start_cycle, cycle, active_frame)
                                    == SinkAppetite::WantsRecord
                                {
                                    let name = specs
                                        .get(&spec_id)
                                        .map(|spec| spec.name.clone())
                                        .unwrap_or_else(|| format!("#{spec_id}"));
                                    let rendered = metadata
                                        .get(&metadata_id)
                                        .and_then(|record| record.rendered_name.clone());
                                    timeline.record(
                                        CpuTimelineInterval {
                                            thread_id: state.thread_id,
                                            spec_id,
                                            name,
                                            start_cycle: entry.start_cycle,
                                            end_cycle: cycle,
                                            duration,
                                            duration_seconds: state.cycle_frequency.map(
                                                |frequency| duration as f64 / frequency as f64,
                                            ),
                                            metadata_id: Some(metadata_id),
                                            rendered_name: rendered,
                                        },
                                        active_frame,
                                    );
                                }
                            }
                            state.metadata_stack_context.leave_inline(metadata_id);
                        }
                        CpuStackEntryKind::Metadata {
                            metadata_id,
                            spec_id: None,
                        } => {
                            state.metadata_stack_context.leave_inline(metadata_id);
                        }
                    }
                    state.batches.intervals += 1;
                } else {
                    state.batches.unmatched_ends += 1;
                }
            }
            0b01 => {
                let payload = reader.read_u64()?;
                if (payload & 1) != 0 {
                    state.batches.metadata_scopes += 1;
                    let metadata_id = u32::try_from(payload >> 1).map_err(|_| {
                        TraceError::new(
                            TraceErrorKind::MalformedData,
                            0,
                            "CpuProfiler.EventBatchV3.Data",
                            "metadata id does not fit in u32",
                        )
                    })?;
                    let spec_id = metadata.get(&metadata_id).map(|record| record.spec_id);
                    state.metadata_stack_context.enter_inline(metadata_id);
                    state.thread_state.stack.push(CpuStackEntry {
                        kind: CpuStackEntryKind::Metadata {
                            metadata_id,
                            spec_id,
                        },
                        start_cycle: cycle,
                        accumulated_cycles: 0,
                    });
                } else {
                    let spec_id = u32::try_from(payload >> 1).map_err(|_| {
                        TraceError::new(
                            TraceErrorKind::MalformedData,
                            0,
                            "CpuProfiler.EventBatchV3.Data",
                            "scope spec id does not fit in u32",
                        )
                    })?;
                    if !specs.contains_key(&spec_id) {
                        state.batches.unresolved_specs += 1;
                    }
                    state.thread_state.stack.push(CpuStackEntry {
                        kind: CpuStackEntryKind::PlainSpec(spec_id),
                        start_cycle: cycle,
                        accumulated_cycles: 0,
                    });
                }
            }
            0b10 => {
                let depth = reader.read_u64()?;
                state.batches.coroutine_records += 1;
                suspend_cpu_coroutine_stack(
                    &mut state.thread_state.stack,
                    coroutine_stack_depth(depth),
                    cycle,
                    state.thread_state.active_coroutine_id,
                    &mut state.thread_state.coroutine_stacks,
                );
                state.thread_state.active_coroutine_id = None;
            }
            0b11 => {
                let coroutine_id = reader.read_u64()?;
                let depth = reader.read_u64()?;
                state.batches.coroutine_records += 1;
                let depth = coroutine_stack_depth(depth);
                suspend_cpu_coroutine_stack(
                    &mut state.thread_state.stack,
                    depth,
                    cycle,
                    state.thread_state.active_coroutine_id,
                    &mut state.thread_state.coroutine_stacks,
                );
                if let Some(mut restored) =
                    state.thread_state.coroutine_stacks.remove(&coroutine_id)
                {
                    for entry in &mut restored {
                        entry.start_cycle = cycle;
                    }
                    state.thread_state.stack.extend(restored);
                }
                state.thread_state.active_coroutine_id = Some(coroutine_id);
            }
            _ => unreachable!("opcode mask is two bits"),
        }
        state.thread_state.last_cycle = cycle;
    }

    Ok(())
}

fn cpu_batch_thread_state_unterminated_scopes(state: &CpuBatchThreadState) -> u64 {
    u64::try_from(state.stack.len()).unwrap()
        + state
            .coroutine_stacks
            .values()
            .map(|stack| u64::try_from(stack.len()).unwrap())
            .sum::<u64>()
}

fn rebase_cpu_stack_starts(stack: &mut [CpuStackEntry], shift: u64) {
    for entry in stack {
        entry.start_cycle = entry.start_cycle.saturating_add(shift);
    }
}

fn record_restored_cpu_metadata_scope(
    metadata_id: u32,
    start_cycle: u64,
    end_cycle: u64,
    duration: u64,
    metadata: &BTreeMap<u32, CpuMetadataRecord>,
    state: &mut CpuBatchDecodeState<'_, '_>,
) {
    let Some(record) = metadata.get(&metadata_id) else {
        return;
    };
    let spec_id = record.spec_id;
    let total = state.metadata_scope_totals.entry(spec_id).or_insert((0, 0));
    total.0 += 1;
    total.1 = total.1.saturating_add(duration);
    state.batches.restored_metadata_scopes += 1;
    record_cpu_metadata_interval(
        CpuMetadataIntervalRecord {
            metadata_id,
            spec_id,
            start_cycle,
            end_cycle,
            duration,
            attribution: CpuMetadataAttribution::RestoredStack,
        },
        metadata,
        state.metadata_interval_state,
    );
}

fn record_cpu_frame_scope(
    spec_id: u32,
    start_cycle: u64,
    end_cycle: u64,
    duration: u64,
    metadata: &BTreeMap<u32, CpuMetadataRecord>,
    state: &mut CpuBatchDecodeState<'_, '_>,
) {
    let Some(frame_number) = state.metadata_stack_context.active_frame_number(metadata) else {
        return;
    };
    let frame_totals = state.frame_scope_totals.entry(frame_number).or_default();
    let total = frame_totals.entry(spec_id).or_insert((0, 0));
    total.0 += 1;
    total.1 = total.1.saturating_add(duration);
    let bounds = state
        .frame_cycle_bounds
        .entry(frame_number)
        .or_insert((start_cycle, end_cycle));
    bounds.0 = bounds.0.min(start_cycle);
    bounds.1 = bounds.1.max(end_cycle);
}

fn suspend_cpu_coroutine_stack(
    stack: &mut Vec<CpuStackEntry>,
    depth: usize,
    cycle: u64,
    coroutine_id: Option<u64>,
    coroutine_stacks: &mut BTreeMap<u64, Vec<CpuStackEntry>>,
) {
    if stack.len() <= depth {
        return;
    }

    let mut suspended = stack.split_off(depth);
    for entry in &mut suspended {
        entry.accumulated_cycles = entry
            .accumulated_cycles
            .saturating_add(cycle.saturating_sub(entry.start_cycle));
    }
    if let Some(coroutine_id) = coroutine_id {
        coroutine_stacks.insert(coroutine_id, suspended);
    }
}

fn coroutine_stack_depth(depth: u64) -> usize {
    usize::try_from(depth).unwrap_or(usize::MAX)
}

fn record_cpu_metadata_interval(
    interval: CpuMetadataIntervalRecord,
    metadata: &BTreeMap<u32, CpuMetadataRecord>,
    state: &mut CpuMetadataIntervalState,
) {
    let Some(record) = metadata.get(&interval.metadata_id) else {
        return;
    };
    let rendered_name = record.rendered_name.clone();
    if let Some(rendered_name) = &rendered_name {
        let entry = state
            .rendered_scope_totals
            .entry((interval.spec_id, rendered_name.clone()))
            .or_insert((0, 0));
        entry.0 += 1;
        entry.1 = entry.1.saturating_add(interval.duration);
    }
    if state.samples.len() < 40 {
        state.samples.push(CpuMetadataIntervalSample {
            spec_id: interval.spec_id,
            metadata_id: interval.metadata_id,
            attribution: interval.attribution,
            name: record.name.clone(),
            rendered_name,
            start_cycle: interval.start_cycle,
            end_cycle: interval.end_cycle,
            duration_cycles: interval.duration,
        });
    }
}

pub(super) fn decode_new_trace(
    event: &EventTypeInfo,
    data: &[u8],
    base_offset: u64,
) -> Result<TracePrologue, TraceError> {
    Ok(TracePrologue {
        start_cycle: read_u64_field(event, data, "StartCycle", base_offset)?,
        cycle_frequency: read_u64_field(event, data, "CycleFrequency", base_offset)?,
        endian: read_u16_field(event, data, "Endian", base_offset)?,
        pointer_size: read_u8_field(event, data, "PointerSize", base_offset)?,
        start_date_time: read_optional_f64_field(event, data, "StartDateTime", base_offset)?,
    })
}

pub(super) fn decode_thread_info(
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
        groups: Vec::new(),
        active_group: None,
    })
}

pub(crate) fn read_u8_field(
    event: &EventTypeInfo,
    data: &[u8],
    name: &str,
    base_offset: u64,
) -> Result<u8, TraceError> {
    let bytes = fixed_field_bytes(event, data, name, 1, base_offset)?;
    Ok(bytes[0])
}

pub(crate) fn read_u16_field(
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

pub(crate) fn read_u32_field(
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

fn read_optional_i32_field(
    event: &EventTypeInfo,
    data: &[u8],
    name: &str,
    base_offset: u64,
) -> Result<Option<i32>, TraceError> {
    if event.fields.iter().any(|field| field.name == name) {
        Ok(Some(read_i32_field(event, data, name, base_offset)?))
    } else {
        Ok(None)
    }
}

pub(crate) fn read_i32_field(
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

pub(crate) fn read_i64_field(
    event: &EventTypeInfo,
    data: &[u8],
    name: &str,
    base_offset: u64,
) -> Result<i64, TraceError> {
    Ok(i64::from_le_bytes(
        fixed_field_bytes(event, data, name, 8, base_offset)?
            .try_into()
            .expect("fixed field length was checked"),
    ))
}

pub(crate) fn read_u64_field(
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

fn read_pointer_field(
    event: &EventTypeInfo,
    data: &[u8],
    name: &str,
    base_offset: u64,
) -> Result<u64, TraceError> {
    let field = find_field(event, name)?;
    match field.size {
        4 => Ok(u64::from(read_u32_field(event, data, name, base_offset)?)),
        8 => read_u64_field(event, data, name, base_offset),
        size => Err(TraceError::new(
            TraceErrorKind::MalformedData,
            base_offset + u64::from(field.offset),
            format!("{}.{}", event.event, name),
            format!("expected 4 or 8 byte pointer field, got {size}"),
        )),
    }
}

pub(crate) fn read_f32_field(
    event: &EventTypeInfo,
    data: &[u8],
    name: &str,
    base_offset: u64,
) -> Result<f32, TraceError> {
    Ok(f32::from_le_bytes(
        fixed_field_bytes(event, data, name, 4, base_offset)?
            .try_into()
            .expect("fixed field length was checked"),
    ))
}

pub(crate) fn read_f64_field(
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

fn read_optional_f64_field(
    event: &EventTypeInfo,
    data: &[u8],
    name: &str,
    base_offset: u64,
) -> Result<Option<f64>, TraceError> {
    if event.fields.iter().any(|field| field.name == name) {
        Ok(Some(read_f64_field(event, data, name, base_offset)?))
    } else {
        Ok(None)
    }
}

pub(crate) fn fixed_field_bytes<'a>(
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

pub(crate) fn find_field<'a>(
    event: &'a EventTypeInfo,
    name: &str,
) -> Result<&'a FieldInfo, TraceError> {
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

pub(crate) fn event_data_size(event: &EventTypeInfo) -> usize {
    event
        .fields
        .iter()
        .map(|field| usize::from(field.offset) + usize::from(field.size))
        .max()
        .unwrap_or(0)
}

pub(crate) fn parse_protocol5_aux(
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

pub(crate) fn read_aux_string(
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

fn read_aux_text(
    event: &EventTypeInfo,
    aux: &BTreeMap<u8, Vec<u8>>,
    name: &str,
) -> Result<String, TraceError> {
    optional_aux_text(event, aux, name)?.ok_or_else(|| {
        TraceError::new(
            TraceErrorKind::MalformedData,
            0,
            format!("{}.{}", event.event, name),
            "event payload is missing required aux text",
        )
    })
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

pub(crate) fn optional_aux_text(
    event: &EventTypeInfo,
    aux: &BTreeMap<u8, Vec<u8>>,
    name: &str,
) -> Result<Option<String>, TraceError> {
    let Some((index, field)) = event
        .fields
        .iter()
        .enumerate()
        .find(|(_, field)| field.name == name)
    else {
        return Ok(None);
    };
    let Some(bytes) = aux.get(&(index as u8)) else {
        return Ok(None);
    };
    match field.type_name.as_str() {
        "ansi_string" => Ok(Some(decode_ansi_bytes(bytes))),
        "wide_string" => decode_wide_bytes(bytes).map(Some).map_err(|detail| {
            TraceError::new(
                TraceErrorKind::MalformedData,
                0,
                format!("{}.{}", event.event, name),
                detail,
            )
        }),
        _ => Ok(Some(decode_ansi_bytes(bytes))),
    }
}

fn aux_bytes_len(event: &EventTypeInfo, aux: &BTreeMap<u8, Vec<u8>>, name: &str) -> usize {
    let Some(index) = event.fields.iter().position(|field| field.name == name) else {
        return 0;
    };
    aux.get(&(index as u8)).map_or(0, Vec::len)
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

    #[inline]
    fn is_empty(&self) -> bool {
        self.cursor >= self.bytes.len()
    }

    #[inline]
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
    let offset = reader.read_u16(&format_args!("NewEvent.Fields[{index}].Offset"))?;
    let size = reader.read_u16(&format_args!("NewEvent.Fields[{index}].Size"))?;
    let type_info = reader.read_u8(&format_args!("NewEvent.Fields[{index}].TypeInfo"))?;
    let name_size = reader.read_u8(&format_args!("NewEvent.Fields[{index}].NameSize"))?;
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
    let field_type = reader.read_u8(&format_args!("NewEvent.Fields[{index}].FieldType"))?;
    reader.skip(1, &format_args!("NewEvent.Fields[{index}].Padding"))?;
    match field_type {
        0 => read_protocol4_field(reader, index),
        1 => {
            let offset = reader.read_u16(&format_args!("NewEvent.Fields[{index}].Offset"))?;
            let ref_uid = reader.read_u16(&format_args!("NewEvent.Fields[{index}].RefUid"))?;
            let type_info = reader.read_u8(&format_args!("NewEvent.Fields[{index}].TypeInfo"))?;
            let name_size = reader.read_u8(&format_args!("NewEvent.Fields[{index}].NameSize"))?;
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
            let offset = reader.read_u16(&format_args!("NewEvent.Fields[{index}].Offset"))?;
            reader.skip(2, &format_args!("NewEvent.Fields[{index}].Unused1"))?;
            reader.skip(1, &format_args!("NewEvent.Fields[{index}].Unused2"))?;
            let type_info = reader.read_u8(&format_args!("NewEvent.Fields[{index}].TypeInfo"))?;
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
    use crate::ArchiveLimits;

    const UINT8: u8 = 0x00;
    const UINT16: u8 = 0x01;
    const UINT32: u8 = 0x02;
    const UINT64: u8 = 0x03;
    const INT32: u8 = 0x12;
    const INT64: u8 = 0x13;
    const FLOAT64: u8 = 0x43;
    const ARRAY: u8 = 0x80;
    const ANSI_STRING: u8 = 0x88;
    const WIDE_STRING: u8 = 0x89;

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
    fn maps_archive_allocation_limit_as_resource_limit() {
        let limits = ArchiveLimits {
            max_array_elements: 10,
            max_allocation_bytes: 7,
            ..ArchiveLimits::default()
        };
        let bytes = 2_i32.to_le_bytes();
        let archive_error = Reader::with_limits(&bytes, limits)
            .read_tarray::<u32>("Values", 4, |reader, _| reader.read_u32("Value"))
            .unwrap_err();
        assert_eq!(archive_error.kind(), ArchiveErrorKind::AllocationLimit);

        let error = TraceError::from(archive_error);

        assert_eq!(error.kind(), TraceErrorKind::ResourceLimit);
        assert_eq!(error.path(), "Values.Count");
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
    fn rejects_absurd_new_event_field_count_before_allocating_fields() {
        let declaration = [42, 0, u8::MAX, 0x07, 3, 4];

        let error =
            decode_new_event(&declaration, 7, 0).expect_err("absurd field count should fail");

        assert_eq!(error.kind(), TraceErrorKind::MalformedData);
        assert_eq!(error.path(), "NewEvent.FieldCount");
        assert!(error.detail().contains("minimum serialized size"));
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
                start_date_time: Some(1234.5),
            })
        );
        assert_eq!(
            trace.thread_info,
            vec![TraceThreadInfo {
                thread_id: 2,
                system_id: 99,
                sort_hint: -7,
                name: "GameThread".to_owned(),
                groups: Vec::new(),
                active_group: None,
            }]
        );
    }

    #[test]
    fn decodes_ue56_new_trace_without_start_date_time() {
        let new_trace_uid = 10;
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
            ],
        );

        let mut new_trace_data = Vec::new();
        new_trace_data.extend_from_slice(&100_u64.to_le_bytes());
        new_trace_data.extend_from_slice(&1_000_000_u64.to_le_bytes());
        new_trace_data.extend_from_slice(&0x524d_u16.to_le_bytes());
        new_trace_data.push(8);

        let bytes = trace_with_events(&[
            important_event(0, &new_trace_decl),
            important_event(new_trace_uid, &new_trace_data),
        ]);

        let trace = inspect(&bytes).unwrap();
        assert_eq!(
            trace.prologue,
            Some(TracePrologue {
                start_cycle: 100,
                cycle_frequency: 1_000_000,
                endian: 0x524d,
                pointer_size: 8,
                start_date_time: None,
            })
        );
        let json = serde_json::to_value(trace).unwrap();
        assert!(json["prologue"].get("start_date_time").is_none());
    }

    #[test]
    fn summarizes_counter_specs_and_values() {
        let spec_event = test_event_type(
            20,
            "Counters",
            "Spec",
            &[
                regular_field(0, 2, UINT16, "Id"),
                regular_field(2, 1, UINT8, "Type"),
                regular_field(3, 1, UINT8, "DisplayHint"),
                regular_field(4, 0, ANSI_STRING, "Name"),
            ],
        );
        let int_value_event = test_event_type(
            21,
            "Counters",
            "SetValueInt",
            &[
                regular_field(0, 8, UINT64, "Cycle"),
                regular_field(8, 8, INT64, "Value"),
                regular_field(16, 2, UINT16, "CounterId"),
            ],
        );
        let float_value_event = test_event_type(
            22,
            "Counters",
            "SetValueFloat",
            &[
                regular_field(0, 8, UINT64, "Cycle"),
                regular_field(8, 8, FLOAT64, "Value"),
                regular_field(16, 2, UINT16, "CounterId"),
            ],
        );

        let mut spec_data = Vec::new();
        spec_data.extend_from_slice(&7_u16.to_le_bytes());
        spec_data.push(1);
        spec_data.push(1);
        spec_data.extend_from_slice(&aux(3, b"Memory/Used"));
        spec_data.push(3);
        let spec = decode_counter_spec(&spec_event, &spec_data, 0).unwrap();

        let mut specs = BTreeMap::new();
        specs.insert(spec.id, spec);
        let mut states = BTreeMap::new();
        let mut unresolved = 0;

        let mut int_data = Vec::new();
        int_data.extend_from_slice(&100_u64.to_le_bytes());
        int_data.extend_from_slice(&50_i64.to_le_bytes());
        int_data.extend_from_slice(&7_u16.to_le_bytes());
        decode_counter_value(
            &int_value_event,
            &int_data,
            &specs,
            &mut states,
            &mut unresolved,
            0,
        )
        .unwrap();

        let mut float_data = Vec::new();
        float_data.extend_from_slice(&120_u64.to_le_bytes());
        float_data.extend_from_slice(&75.5_f64.to_le_bytes());
        float_data.extend_from_slice(&7_u16.to_le_bytes());
        decode_counter_value(
            &float_value_event,
            &float_data,
            &specs,
            &mut states,
            &mut unresolved,
            0,
        )
        .unwrap();

        let dashboard = counter_dashboard(specs, states, unresolved);
        assert_eq!(dashboard.specs, 1);
        assert_eq!(dashboard.samples, 2);
        assert_eq!(dashboard.int_samples, 1);
        assert_eq!(dashboard.float_samples, 1);
        assert_eq!(dashboard.unresolved_samples, 0);
        assert_eq!(dashboard.counters[0].name, "Memory/Used");
        assert_eq!(dashboard.counters[0].kind, CounterKind::Float);
        assert_eq!(
            dashboard.counters[0].display_hint,
            CounterDisplayHint::Memory
        );
        assert_eq!(dashboard.counters[0].first_cycle, Some(100));
        assert_eq!(dashboard.counters[0].last_cycle, Some(120));
        assert_eq!(dashboard.counters[0].min, Some(50.0));
        assert_eq!(dashboard.counters[0].max, Some(75.5));
        assert_eq!(dashboard.counters[0].latest, Some(75.5));
        assert_eq!(dashboard.counters[0].sample_points.len(), 2);
        assert_eq!(dashboard.counters[0].sample_points[0].cycle, 100);
        assert_eq!(dashboard.counters[0].sample_points[1].value, 75.5);
    }

    #[test]
    fn summarizes_stat_specs() {
        let spec_event = test_event_type(
            25,
            "Stats",
            "Spec",
            &[
                regular_field(0, 4, UINT32, "Id"),
                regular_field(4, 1, UINT8, "IsFloatingPoint"),
                regular_field(5, 1, UINT8, "IsMemory"),
                regular_field(6, 1, UINT8, "ShouldClearEveryFrame"),
                regular_field(7, 0, ANSI_STRING, "Name"),
                regular_field(7, 0, ANSI_STRING, "Description"),
                regular_field(7, 0, ANSI_STRING, "Group"),
            ],
        );

        let mut memory_data = Vec::new();
        memory_data.extend_from_slice(&10_u32.to_le_bytes());
        memory_data.push(0);
        memory_data.push(1);
        memory_data.push(0);
        memory_data.extend_from_slice(&aux(4, b"STAT_TotalLLM"));
        memory_data.extend_from_slice(&aux(5, b"Total"));
        memory_data.extend_from_slice(&aux(6, b"STATGROUP_LLMFULL"));
        memory_data.push(3);
        let memory_stat = decode_stat_spec(&spec_event, &memory_data, 0).unwrap();

        let mut float_data = Vec::new();
        float_data.extend_from_slice(&11_u32.to_le_bytes());
        float_data.push(1);
        float_data.push(0);
        float_data.push(1);
        float_data.extend_from_slice(&aux(4, b"STAT_FrameTime"));
        float_data.extend_from_slice(&aux(5, b"Frame time"));
        float_data.extend_from_slice(&aux(6, b"STATGROUP_Engine"));
        float_data.push(3);
        let float_stat = decode_stat_spec(&spec_event, &float_data, 0).unwrap();

        let mut specs = BTreeMap::new();
        specs.insert(memory_stat.id, memory_stat);
        specs.insert(float_stat.id, float_stat);

        let dashboard = stats_dashboard(specs);
        assert_eq!(dashboard.specs, 2);
        assert_eq!(dashboard.floating_point_specs, 1);
        assert_eq!(dashboard.memory_specs, 1);
        assert_eq!(dashboard.clear_every_frame_specs, 1);
        assert_eq!(dashboard.groups.len(), 2);
        assert!(
            dashboard
                .groups
                .iter()
                .any(|group| group.name == "STATGROUP_LLMFULL"
                    && group.specs == 1
                    && group.memory_specs == 1)
        );
        assert!(dashboard.stats.iter().any(|stat| stat.id == 10
            && stat.name == "STAT_TotalLLM"
            && stat.description == "Total"
            && stat.group == "STATGROUP_LLMFULL"
            && stat.is_memory));
    }

    #[test]
    fn summarizes_csv_profiler_catalog() {
        let category_event = test_event_type(
            26,
            "CsvProfiler",
            "RegisterCategory",
            &[
                regular_field(0, 4, INT32, "Index"),
                regular_field(4, 0, ANSI_STRING, "Name"),
            ],
        );
        let declared_event = test_event_type(
            27,
            "CsvProfiler",
            "DefineDeclaredStat",
            &[
                regular_field(0, 8, UINT64, "StatId"),
                regular_field(8, 4, INT32, "CategoryIndex"),
                regular_field(12, 0, ANSI_STRING, "Name"),
            ],
        );
        let inline_event = test_event_type(
            28,
            "CsvProfiler",
            "DefineInlineStat",
            &[
                regular_field(0, 8, UINT64, "StatId"),
                regular_field(8, 4, INT32, "CategoryIndex"),
                regular_field(12, 0, ANSI_STRING, "Name"),
            ],
        );

        let mut category_data = Vec::new();
        category_data.extend_from_slice(&1_i32.to_le_bytes());
        category_data.extend_from_slice(&aux(1, b"IoDispatcherFileBackend"));
        category_data.push(3);
        let category = decode_csv_category(&category_event, &category_data, 0).unwrap();

        let mut declared_data = Vec::new();
        declared_data.extend_from_slice(&100_u64.to_le_bytes());
        declared_data.extend_from_slice(&1_i32.to_le_bytes());
        declared_data.extend_from_slice(&aux(2, b"FrameBytesScatteredKB"));
        declared_data.push(3);
        let declared = decode_csv_stat(&declared_event, &declared_data, 0).unwrap();

        let mut inline_data = Vec::new();
        inline_data.extend_from_slice(&101_u64.to_le_bytes());
        inline_data.extend_from_slice(&1_i32.to_le_bytes());
        inline_data.extend_from_slice(&aux(2, b"FMsgLogfCount"));
        inline_data.push(3);
        let inline = decode_csv_stat(&inline_event, &inline_data, 0).unwrap();

        let mut orphan_data = Vec::new();
        orphan_data.extend_from_slice(&102_u64.to_le_bytes());
        orphan_data.extend_from_slice(&99_i32.to_le_bytes());
        orphan_data.extend_from_slice(&aux(2, b"Orphan"));
        orphan_data.push(3);
        let orphan = decode_csv_stat(&declared_event, &orphan_data, 0).unwrap();

        let mut categories = BTreeMap::new();
        categories.insert(category.index, category);
        let mut stats = BTreeMap::new();
        stats.insert(declared.stat_id, declared);
        stats.insert(inline.stat_id, inline);
        stats.insert(orphan.stat_id, orphan);

        let dashboard = csv_dashboard(categories, stats);
        assert_eq!(dashboard.categories, 1);
        assert_eq!(dashboard.stats, 3);
        assert_eq!(dashboard.declared_stats, 2);
        assert_eq!(dashboard.inline_stats, 1);
        assert_eq!(dashboard.unresolved_stats, 1);
        assert_eq!(dashboard.top_categories[0].name, "IoDispatcherFileBackend");
        assert_eq!(dashboard.top_categories[0].stats, 2);
        assert_eq!(dashboard.top_categories[0].declared_stats, 1);
        assert_eq!(dashboard.top_categories[0].inline_stats, 1);
        assert!(
            dashboard
                .stat_defs
                .iter()
                .any(|stat| stat.name == "FMsgLogfCount"
                    && stat.kind == CsvStatKind::Inline
                    && stat.category.as_deref() == Some("IoDispatcherFileBackend"))
        );
    }

    #[test]
    fn summarizes_load_time_class_catalog() {
        let class_event = test_event_type(
            29,
            "LoadTime",
            "ClassInfo",
            &[
                regular_field(0, 8, UINT64, "Class"),
                regular_field(8, 0, ANSI_STRING, "Name"),
            ],
        );

        let mut data = Vec::new();
        data.extend_from_slice(&0x1234_u64.to_le_bytes());
        data.extend_from_slice(&aux(1, b"BlueprintGeneratedClass"));
        data.push(3);
        let class_info = decode_load_time_class_info(&class_event, &data, 0).unwrap();
        assert_eq!(class_info.class, 0x1234);
        assert_eq!(class_info.name, "BlueprintGeneratedClass");

        let mut state = LoadTimeState::default();
        state.classes.insert(class_info.class, class_info.name);
        let dashboard = state.dashboard();
        assert_eq!(dashboard.class_count, 1);
        assert_eq!(dashboard.classes[0].class, 0x1234);
        assert_eq!(dashboard.classes[0].name, "BlueprintGeneratedClass");
    }

    #[test]
    fn summarizes_load_time_packages_and_requests() {
        let package_event = test_event_type(
            36,
            "LoadTime",
            "PackageSummary",
            &[
                regular_field(0, 8, UINT64, "AsyncPackage"),
                regular_field(8, 4, UINT32, "TotalHeaderSize"),
                regular_field(12, 4, UINT32, "ImportCount"),
                regular_field(16, 4, UINT32, "ExportCount"),
                regular_field(20, 0, WIDE_STRING, "Name"),
                regular_field(20, 4, INT32, "Priority"),
            ],
        );
        let begin_event = test_event_type(
            37,
            "LoadTime",
            "BeginRequest",
            &[
                regular_field(0, 8, UINT64, "Cycle"),
                regular_field(8, 8, UINT64, "RequestId"),
            ],
        );
        let end_event = test_event_type(
            38,
            "LoadTime",
            "EndRequest",
            &[
                regular_field(0, 8, UINT64, "Cycle"),
                regular_field(8, 8, UINT64, "RequestId"),
            ],
        );
        let start_async_event = test_event_type(
            39,
            "LoadTime",
            "StartAsyncLoading",
            &[regular_field(0, 8, UINT64, "Cycle")],
        );
        let suspend_async_event = test_event_type(
            40,
            "LoadTime",
            "SuspendAsyncLoading",
            &[regular_field(0, 8, UINT64, "Cycle")],
        );
        let resume_async_event = test_event_type(
            41,
            "LoadTime",
            "ResumeAsyncLoading",
            &[regular_field(0, 8, UINT64, "Cycle")],
        );

        let mut state = LoadTimeState::default();

        let mut package_data = Vec::new();
        package_data.extend_from_slice(&0xabc_u64.to_le_bytes());
        package_data.extend_from_slice(&512_u32.to_le_bytes());
        package_data.extend_from_slice(&3_u32.to_le_bytes());
        package_data.extend_from_slice(&7_u32.to_le_bytes());
        package_data.extend_from_slice(&42_i32.to_le_bytes());
        package_data.extend_from_slice(&aux(4, &wide("/Game/Map")));
        package_data.push(3);
        decode_load_time_event(&package_event, &package_data, &mut state, 0).unwrap();

        let mut begin_data = Vec::new();
        begin_data.extend_from_slice(&100_u64.to_le_bytes());
        begin_data.extend_from_slice(&77_u64.to_le_bytes());
        decode_load_time_event(&begin_event, &begin_data, &mut state, 0).unwrap();

        let mut end_data = Vec::new();
        end_data.extend_from_slice(&140_u64.to_le_bytes());
        end_data.extend_from_slice(&77_u64.to_le_bytes());
        decode_load_time_event(&end_event, &end_data, &mut state, 0).unwrap();

        decode_load_time_event(&start_async_event, &10_u64.to_le_bytes(), &mut state, 0).unwrap();
        decode_load_time_event(&suspend_async_event, &30_u64.to_le_bytes(), &mut state, 0).unwrap();
        decode_load_time_event(&resume_async_event, &50_u64.to_le_bytes(), &mut state, 0).unwrap();

        let dashboard = state.dashboard();
        assert_eq!(dashboard.package_count, 1);
        assert_eq!(dashboard.packages[0].async_package, 0xabc);
        assert_eq!(dashboard.packages[0].name, "/Game/Map");
        assert_eq!(dashboard.packages[0].priority, Some(42));
        assert_eq!(dashboard.requests.begun, 1);
        assert_eq!(dashboard.requests.completed, 1);
        assert_eq!(dashboard.requests.total_cycles, 40);
        assert_eq!(dashboard.requests.samples[0].request_id, 77);
        assert_eq!(dashboard.async_loading.starts, 1);
        assert_eq!(dashboard.async_loading.suspends, 1);
        assert_eq!(dashboard.async_loading.resumes, 1);
        assert_eq!(dashboard.async_loading.first_cycle, Some(10));
        assert_eq!(dashboard.async_loading.last_cycle, Some(50));
    }

    #[test]
    fn summarizes_io_store_request_lifecycle() {
        let backend_event = test_event_type(
            42,
            "IoStore",
            "BackendName",
            &[
                regular_field(0, 8, UINT64, "BackendHandle"),
                regular_field(8, 0, WIDE_STRING, "Name"),
            ],
        );
        let create_event = test_event_type(
            43,
            "IoStore",
            "RequestCreate",
            &[
                regular_field(0, 8, UINT64, "Cycle"),
                regular_field(8, 8, UINT64, "RequestHandle"),
                regular_field(16, 8, UINT64, "BatchHandle"),
                regular_field(24, 4, UINT32, "ChunkIdHash"),
                regular_field(28, 1, UINT8, "ChunkType"),
                regular_field(29, 4, UINT32, "CallstackId"),
                regular_field(33, 8, UINT64, "Offset"),
                regular_field(41, 8, UINT64, "Size"),
            ],
        );
        let started_event = test_event_type(
            44,
            "IoStore",
            "RequestStarted",
            &[
                regular_field(0, 8, UINT64, "Cycle"),
                regular_field(8, 8, UINT64, "RequestHandle"),
                regular_field(16, 8, UINT64, "BackendHandle"),
            ],
        );
        let completed_event = test_event_type(
            45,
            "IoStore",
            "RequestCompleted",
            &[
                regular_field(0, 8, UINT64, "Cycle"),
                regular_field(8, 8, UINT64, "RequestHandle"),
                regular_field(16, 8, UINT64, "Size"),
            ],
        );

        let mut state = IoStoreState::default();

        let mut backend_data = Vec::new();
        backend_data.extend_from_slice(&0xbeef_u64.to_le_bytes());
        backend_data.extend_from_slice(&aux(1, &wide("FileBackend")));
        backend_data.push(3);
        decode_io_store_event(&backend_event, &backend_data, &mut state, 0).unwrap();

        let mut create_data = Vec::new();
        create_data.extend_from_slice(&100_u64.to_le_bytes());
        create_data.extend_from_slice(&0x111_u64.to_le_bytes());
        create_data.extend_from_slice(&0x222_u64.to_le_bytes());
        create_data.extend_from_slice(&0x333_u32.to_le_bytes());
        create_data.push(4);
        create_data.extend_from_slice(&0_u32.to_le_bytes());
        create_data.extend_from_slice(&16_u64.to_le_bytes());
        create_data.extend_from_slice(&4096_u64.to_le_bytes());
        decode_io_store_event(&create_event, &create_data, &mut state, 0).unwrap();

        let mut started_data = Vec::new();
        started_data.extend_from_slice(&120_u64.to_le_bytes());
        started_data.extend_from_slice(&0x111_u64.to_le_bytes());
        started_data.extend_from_slice(&0xbeef_u64.to_le_bytes());
        decode_io_store_event(&started_event, &started_data, &mut state, 0).unwrap();

        let mut completed_data = Vec::new();
        completed_data.extend_from_slice(&160_u64.to_le_bytes());
        completed_data.extend_from_slice(&0x111_u64.to_le_bytes());
        completed_data.extend_from_slice(&2048_u64.to_le_bytes());
        decode_io_store_event(&completed_event, &completed_data, &mut state, 0).unwrap();

        let dashboard = state.dashboard();
        assert_eq!(dashboard.backend_count, 1);
        assert_eq!(dashboard.backends[0].name, "FileBackend");
        assert_eq!(dashboard.backends[0].starts, 1);
        assert_eq!(dashboard.requests_created, 1);
        assert_eq!(dashboard.requests_started, 1);
        assert_eq!(dashboard.requests_completed, 1);
        assert_eq!(dashboard.bytes_requested, 4096);
        assert_eq!(dashboard.bytes_completed, 2048);
        assert_eq!(dashboard.request_samples[0].request_handle, 0x111);
        assert_eq!(
            dashboard.request_samples[0].backend_name.as_deref(),
            Some("FileBackend")
        );
        assert_eq!(
            dashboard.request_samples[0].status,
            IoStoreRequestStatus::Completed
        );
    }

    #[test]
    fn summarizes_platform_file_open_read_close() {
        let begin_open = test_event_type(
            50,
            "PlatformFile",
            "BeginOpen",
            &[
                regular_field(0, 8, UINT64, "Cycle"),
                regular_field(8, 0, WIDE_STRING, "Path"),
            ],
        );
        let end_open = test_event_type(
            51,
            "PlatformFile",
            "EndOpen",
            &[
                regular_field(0, 8, UINT64, "Cycle"),
                regular_field(8, 8, UINT64, "FileHandle"),
            ],
        );
        let begin_read = test_event_type(
            52,
            "PlatformFile",
            "BeginRead",
            &[
                regular_field(0, 8, UINT64, "Cycle"),
                regular_field(8, 8, UINT64, "ReadHandle"),
                regular_field(16, 8, UINT64, "FileHandle"),
                regular_field(24, 8, UINT64, "Offset"),
                regular_field(32, 8, UINT64, "Size"),
            ],
        );
        let end_read = test_event_type(
            53,
            "PlatformFile",
            "EndRead",
            &[
                regular_field(0, 8, UINT64, "Cycle"),
                regular_field(8, 8, UINT64, "ReadHandle"),
                regular_field(16, 8, UINT64, "SizeRead"),
            ],
        );
        let begin_close = test_event_type(
            54,
            "PlatformFile",
            "BeginClose",
            &[
                regular_field(0, 8, UINT64, "Cycle"),
                regular_field(8, 8, UINT64, "FileHandle"),
            ],
        );
        let end_close = test_event_type(
            55,
            "PlatformFile",
            "EndClose",
            &[regular_field(0, 8, UINT64, "Cycle")],
        );

        let mut state = PlatformFileProvider::default();

        let mut begin_open_data = Vec::new();
        begin_open_data.extend_from_slice(&10_u64.to_le_bytes());
        begin_open_data.extend_from_slice(&aux(1, &wide("/Game/Pack.uasset")));
        begin_open_data.push(3);
        decode_platform_file_event(&begin_open, &begin_open_data, &mut state, 7, 0).unwrap();

        let mut end_open_data = Vec::new();
        end_open_data.extend_from_slice(&20_u64.to_le_bytes());
        end_open_data.extend_from_slice(&0xabc_u64.to_le_bytes());
        decode_platform_file_event(&end_open, &end_open_data, &mut state, 7, 0).unwrap();

        let mut begin_read_data = Vec::new();
        begin_read_data.extend_from_slice(&30_u64.to_le_bytes());
        begin_read_data.extend_from_slice(&0x111_u64.to_le_bytes());
        begin_read_data.extend_from_slice(&0xabc_u64.to_le_bytes());
        begin_read_data.extend_from_slice(&0_u64.to_le_bytes());
        begin_read_data.extend_from_slice(&512_u64.to_le_bytes());
        decode_platform_file_event(&begin_read, &begin_read_data, &mut state, 7, 0).unwrap();

        let mut end_read_data = Vec::new();
        end_read_data.extend_from_slice(&45_u64.to_le_bytes());
        end_read_data.extend_from_slice(&0x111_u64.to_le_bytes());
        end_read_data.extend_from_slice(&256_u64.to_le_bytes());
        decode_platform_file_event(&end_read, &end_read_data, &mut state, 7, 0).unwrap();

        let mut begin_close_data = Vec::new();
        begin_close_data.extend_from_slice(&50_u64.to_le_bytes());
        begin_close_data.extend_from_slice(&0xabc_u64.to_le_bytes());
        decode_platform_file_event(&begin_close, &begin_close_data, &mut state, 7, 0).unwrap();
        decode_platform_file_event(&end_close, &60_u64.to_le_bytes(), &mut state, 7, 0).unwrap();

        let dashboard = state.dashboard();
        assert_eq!(dashboard.opens, 1);
        assert_eq!(dashboard.reads, 1);
        assert_eq!(dashboard.closes, 1);
        assert_eq!(dashboard.bytes_requested_read, 512);
        assert_eq!(dashboard.bytes_read, 256);
        assert_eq!(dashboard.files[0].path, "/Game/Pack.uasset");
        assert_eq!(dashboard.activity_samples.len(), 3);
        assert_eq!(dashboard.activity_samples[1].duration_cycles, Some(15));
        assert_eq!(dashboard.activity_samples[1].actual_size, Some(256));
    }

    #[test]
    fn submission_latency_handles_extreme_file_timestamps_without_overflow() {
        assert_eq!(submission_delay_cycles(u64::MAX, 0), i128::from(u64::MAX));
        assert_eq!(submission_delay_cycles(0, u64::MAX), -i128::from(u64::MAX));
        let samples = vec![
            GpuSubmissionLatencySample {
                queue_id: 0,
                gpu_timestamp_top: u64::MAX,
                cpu_submit_timestamp: 0,
                delay_cycles: i128::from(u64::MAX),
            },
            GpuSubmissionLatencySample {
                queue_id: 1,
                gpu_timestamp_top: 0,
                cpu_submit_timestamp: u64::MAX,
                delay_cycles: -i128::from(u64::MAX),
            },
        ];

        let summary = gpu_submission_latency(samples).expect("latency summary");
        assert_eq!(summary.min_delay_cycles, -i128::from(u64::MAX));
        assert_eq!(summary.max_delay_cycles, i128::from(u64::MAX));
    }

    #[test]
    fn frame_summaries_expose_total_count_before_bounded_retention() {
        let mut queue = GpuQueueState::default();
        for frame_number in 0..121 {
            queue.frames.insert(frame_number, GpuFrameState::default());
        }
        let queues = [(7, queue)].into_iter().collect::<BTreeMap<_, _>>();
        let mut frames = gpu_frame_summaries(&queues);

        let (total_frame_count, truncated) = cap_gpu_frame_summaries(&mut frames, 120);

        assert_eq!(total_frame_count, 121);
        assert!(truncated);
        assert_eq!(frames.len(), 120);
    }

    #[test]
    fn frame_correlation_exposes_total_count_before_bounded_retention() {
        let cpu_metadata_scope_totals = (1..=3)
            .map(|frame_number| {
                (
                    (frame_number, format!("Frame {frame_number}")),
                    (1, u64::from(frame_number)),
                )
            })
            .collect::<BTreeMap<_, _>>();

        let dashboard = frame_correlation_dashboard(
            &cpu_metadata_scope_totals,
            FxHashMap::default(),
            FxHashMap::default(),
            &BTreeMap::new(),
            &[],
            None,
            2,
        );

        assert_eq!(dashboard.total_frame_count, 3);
        assert!(dashboard.truncated);
        assert_eq!(dashboard.frames.len(), 2);
        assert_eq!(dashboard.frames[0].frame_number, 1);
        assert_eq!(dashboard.frames[1].frame_number, 2);
    }

    #[test]
    fn gpu_timeline_retains_paired_intervals_with_a_bound() {
        let mut timeline = GpuTimelineCollector::new(17, 2);
        timeline.record(
            Some(17),
            3,
            GpuTimelineIntervalKind::Work,
            None,
            "Work".to_owned(),
            100,
            120,
            20,
        );
        timeline.record(
            Some(17),
            3,
            GpuTimelineIntervalKind::Breadcrumb,
            Some(9),
            "RenderPass".to_owned(),
            125,
            140,
            15,
        );
        timeline.record(
            Some(17),
            3,
            GpuTimelineIntervalKind::Work,
            None,
            "Work".to_owned(),
            145,
            170,
            25,
        );
        timeline.record(
            Some(18),
            3,
            GpuTimelineIntervalKind::Work,
            None,
            "OtherFrame".to_owned(),
            200,
            220,
            20,
        );

        let dashboard = timeline.into_dashboard();

        assert_eq!(dashboard.frame_number, 17);
        assert_eq!(dashboard.begin_timestamp, 100);
        assert_eq!(dashboard.end_timestamp, 170);
        assert_eq!(dashboard.interval_count, 3);
        assert!(dashboard.truncated);
        assert_eq!(dashboard.intervals.len(), 2);
        assert_eq!(dashboard.intervals[0].kind, GpuTimelineIntervalKind::Work);
        assert_eq!(dashboard.intervals[1].spec_id, Some(9));
    }

    #[test]
    fn summarizes_lightweight_declared_event_families() {
        let thread_timing_event = test_event_type(
            60,
            "$Trace",
            "ThreadTiming",
            &[regular_field(0, 8, UINT64, "BaseTimestamp")],
        );
        let end_thread_event = test_event_type(
            61,
            "CpuProfiler",
            "EndThread",
            &[regular_field(0, 8, UINT64, "Cycle")],
        );
        let memory_scope_event = test_event_type(
            62,
            "Memory",
            "MemoryScope",
            &[regular_field(0, 4, INT32, "Tag")],
        );
        let clear_scope_event = test_event_type(63, "MetadataStack", "ClearScope", &[]);
        let save_stack_event = test_event_type(
            64,
            "MetadataStack",
            "SaveStack",
            &[regular_field(0, 4, UINT32, "Id")],
        );
        let restore_stack_event = test_event_type(
            65,
            "MetadataStack",
            "RestoreStack",
            &[regular_field(0, 4, UINT32, "Id")],
        );
        let add_widget_event = test_event_type(
            66,
            "SlateTrace",
            "AddWidget",
            &[
                regular_field(0, 8, UINT64, "Cycle"),
                regular_field(8, 8, UINT64, "WidgetId"),
            ],
        );

        let timing_data = 700_u64.to_le_bytes();
        let timing = decode_trace_thread_timing(&thread_timing_event, &timing_data, 0, 2).unwrap();
        assert_eq!(
            trace_timing_dashboard([(timing.thread_id, timing)].into_iter().collect()).threads[0]
                .base_timestamp,
            700
        );

        let end_data = 900_u64.to_le_bytes();
        let end_thread = decode_cpu_end_thread(&end_thread_event, &end_data, 0, 3).unwrap();
        assert_eq!(end_thread.thread_id, 3);
        assert_eq!(end_thread.cycle, 900);

        let memory_data = (-42_i32).to_le_bytes();
        let tag = decode_memory_scope(&memory_scope_event, &memory_data, 0).unwrap();
        let mut memory_provider = MemoryProvider::default();
        memory_provider.record_scope(tag);
        memory_provider.record_scope(tag);
        let memory = memory_provider.dashboard();
        assert_eq!(memory.scope_count, 2);
        assert_eq!(memory.scopes[0].tag, -42);

        let mut metadata_stack = MetadataStackState::default();
        decode_metadata_stack_event(&clear_scope_event, &[], &mut metadata_stack, 0).unwrap();
        decode_metadata_stack_event(
            &save_stack_event,
            &123_u32.to_le_bytes(),
            &mut metadata_stack,
            0,
        )
        .unwrap();
        decode_metadata_stack_event(
            &restore_stack_event,
            &123_u32.to_le_bytes(),
            &mut metadata_stack,
            0,
        )
        .unwrap();
        decode_metadata_stack_event(
            &restore_stack_event,
            &456_u32.to_le_bytes(),
            &mut metadata_stack,
            0,
        )
        .unwrap();
        let metadata_stack = metadata_stack.dashboard();
        assert_eq!(metadata_stack.clear_scope_count, 1);
        assert_eq!(metadata_stack.saved_stack_count, 1);
        assert_eq!(metadata_stack.restored_stack_count, 2);
        assert_eq!(metadata_stack.unmatched_restore_count, 1);
        assert_eq!(metadata_stack.saved_stacks[0].id, 123);
        assert_eq!(metadata_stack.restored_stacks[0].id, 123);
        assert!(metadata_stack.restored_stacks[0].saved);
        assert!(
            metadata_stack
                .stack_ids
                .iter()
                .any(|stack| stack.id == 456 && stack.saves == 0 && stack.restores == 1)
        );

        let mut widget_data = Vec::new();
        widget_data.extend_from_slice(&1000_u64.to_le_bytes());
        widget_data.extend_from_slice(&0xfeed_u64.to_le_bytes());
        let widget = decode_slate_add_widget(&add_widget_event, &widget_data, 0).unwrap();
        let mut widget_state = SlateWidgetState::default();
        widget_state.record(widget.cycle);
        widget_state.record(1100);
        let slate = slate_dashboard([(widget.widget_id, widget_state)].into_iter().collect());
        assert_eq!(slate.added_widgets, 2);
        assert_eq!(slate.widgets[0].widget_id, 0xfeed);
        assert_eq!(slate.widgets[0].first_cycle, Some(1000));
        assert_eq!(slate.widgets[0].last_cycle, Some(1100));
    }

    #[test]
    fn decodes_llm_catalog_and_tag_value_arrays() {
        let tag_event = test_event_type(
            70,
            "LLM",
            "TagsSpec",
            &[
                regular_field(0, 8, UINT64, "TagId"),
                regular_field(8, 8, UINT64, "ParentId"),
                regular_field(16, 1, UINT8, "TagSetId"),
                regular_field(17, 0, ANSI_STRING, "Name"),
            ],
        );
        let tracker_event = test_event_type(
            71,
            "LLM",
            "TrackerSpec",
            &[
                regular_field(0, 1, UINT8, "TrackerId"),
                regular_field(1, 0, ANSI_STRING, "Name"),
            ],
        );
        let tag_set_event = test_event_type(
            72,
            "LLM",
            "TagSetSpec",
            &[
                regular_field(0, 1, UINT8, "TagSetId"),
                regular_field(1, 0, ANSI_STRING, "Name"),
            ],
        );
        let values_event = test_event_type(
            73,
            "LLM",
            "TagValue",
            &[
                regular_field(0, 1, UINT8, "TrackerId"),
                regular_field(1, 8, UINT64, "Cycle"),
                regular_field(9, 0, ARRAY, "Tags"),
                regular_field(9, 0, ARRAY, "Values"),
            ],
        );

        let mut tag_data = Vec::new();
        tag_data.extend_from_slice(&101_i64.to_le_bytes());
        tag_data.extend_from_slice(&100_i64.to_le_bytes());
        tag_data.push(2);
        tag_data.extend_from_slice(&aux(3, b"Textures"));
        tag_data.push(3);
        let tag = decode_llm_tag(&tag_event, &tag_data, 0).unwrap();
        assert_eq!(tag.tag, 101);
        assert_eq!(tag.parent, 100);
        assert_eq!(tag.tag_set, 2);
        assert_eq!(tag.name, "Textures");

        let mut tracker_data = vec![1];
        tracker_data.extend_from_slice(&aux(1, b"Platform"));
        tracker_data.push(3);
        assert_eq!(
            decode_llm_tracker(&tracker_event, &tracker_data, 0)
                .unwrap()
                .name,
            "Platform"
        );

        let mut tag_set_data = vec![2];
        tag_set_data.extend_from_slice(&aux(1, b"Assets"));
        tag_set_data.push(3);
        assert_eq!(
            decode_llm_tag_set(&tag_set_event, &tag_set_data, 0)
                .unwrap()
                .name,
            "Assets"
        );

        let mut tags_data = Vec::new();
        tags_data.extend_from_slice(&101_u32.to_le_bytes());
        tags_data.extend_from_slice(&202_u32.to_le_bytes());
        let mut values_data = Vec::new();
        values_data.extend_from_slice(&64_i64.to_le_bytes());
        values_data.extend_from_slice(&(-16_i64).to_le_bytes());
        let mut sample_data = vec![1];
        sample_data.extend_from_slice(&700_u64.to_le_bytes());
        sample_data.extend_from_slice(&aux(2, &tags_data));
        sample_data.extend_from_slice(&aux(3, &values_data));
        sample_data.push(3);

        let sample = decode_llm_tag_values(&values_event, &sample_data, 0).unwrap();
        assert_eq!(sample.tracker_id, 1);
        assert_eq!(sample.cycle, 700);
        assert_eq!(sample.values, vec![(101, 64), (202, -16)]);
        assert_eq!(sample.dropped_values, 0);
    }

    #[test]
    fn decodes_cpu_metadata_values_and_pairs_field_names() {
        let spec_event = test_event_type(
            66,
            "CpuProfiler",
            "MetadataSpec",
            &[
                regular_field(0, 4, UINT32, "Id"),
                regular_field(4, 0, ANSI_STRING, "Name"),
                regular_field(4, 0, ANSI_STRING, "NameFormat"),
                regular_field(4, 0, ARRAY, "FieldNames"),
            ],
        );
        let record_event = test_event_type(
            67,
            "CpuProfiler",
            "Metadata",
            &[
                regular_field(0, 4, UINT32, "Id"),
                regular_field(4, 4, UINT32, "SpecId"),
                regular_field(8, 0, ARRAY, "Metadata"),
            ],
        );

        let mut spec_data = Vec::new();
        spec_data.extend_from_slice(&7_u32.to_le_bytes());
        spec_data.extend_from_slice(&aux(1, b"LoadPackage"));
        spec_data.extend_from_slice(&aux(2, b" %s %llu"));
        spec_data.extend_from_slice(&aux(3, b"\x82\x64Name\x65Frame"));
        spec_data.push(3);
        let spec = decode_cpu_metadata_spec(&spec_event, &spec_data, 0).unwrap();
        assert_eq!(
            spec.field_names,
            vec!["Name".to_owned(), "Frame".to_owned()]
        );

        let mut record_data = Vec::new();
        record_data.extend_from_slice(&99_u32.to_le_bytes());
        record_data.extend_from_slice(&7_u32.to_le_bytes());
        record_data.extend_from_slice(&aux(2, b"\x82\x6b/Test/Asset\x18\x2a"));
        record_data.push(3);
        let mut record = decode_cpu_metadata_record(&record_event, &record_data, 0).unwrap();
        assert_eq!(record.metadata_bytes, 15);
        assert_eq!(record.decoded_metadata_bytes, 15);
        assert_eq!(record.skipped_metadata_bytes, 0);
        assert!(!record.decode_failed);
        assert_eq!(record.strings, vec!["/Test/Asset".to_owned()]);

        let mut trailing_data = Vec::new();
        trailing_data.extend_from_slice(&101_u32.to_le_bytes());
        trailing_data.extend_from_slice(&7_u32.to_le_bytes());
        trailing_data.extend_from_slice(&aux(2, b"\x65Valid\xc1"));
        trailing_data.push(3);
        let mut trailing_record =
            decode_cpu_metadata_record(&record_event, &trailing_data, 0).unwrap();
        assert_eq!(trailing_record.metadata_bytes, 7);
        assert_eq!(trailing_record.decoded_metadata_bytes, 6);
        assert_eq!(trailing_record.skipped_metadata_bytes, 1);
        assert!(trailing_record.decode_failed);

        let mut malformed_data = Vec::new();
        malformed_data.extend_from_slice(&102_u32.to_le_bytes());
        malformed_data.extend_from_slice(&7_u32.to_le_bytes());
        malformed_data.extend_from_slice(&aux(2, b"\xc1"));
        malformed_data.push(3);
        let mut malformed_record =
            decode_cpu_metadata_record(&record_event, &malformed_data, 0).unwrap();
        assert_eq!(malformed_record.metadata_bytes, 1);
        assert_eq!(malformed_record.decoded_metadata_bytes, 0);
        assert_eq!(malformed_record.skipped_metadata_bytes, 1);
        assert!(malformed_record.decode_failed);

        let other_spec = CpuMetadataSpec {
            spec_id: 8,
            name: "Other".to_owned(),
            name_format: None,
            field_names_bytes: 0,
            field_names: vec!["Reason".to_owned()],
        };
        let other_record = CpuMetadataRecord {
            metadata_id: 100,
            spec_id: 8,
            name: "Other".to_owned(),
            rendered_name: None,
            metadata_bytes: 8,
            decoded_metadata_bytes: 8,
            skipped_metadata_bytes: 0,
            decode_failed: false,
            values: vec![MetadataValue::Text {
                value: "OtherOne".to_owned(),
            }],
            strings: vec!["OtherOne".to_owned()],
        };

        let specs = [(spec.spec_id, spec), (other_spec.spec_id, other_spec)]
            .into_iter()
            .collect::<BTreeMap<_, _>>();
        enrich_cpu_metadata_record(&specs, &mut record);
        enrich_cpu_metadata_record(&specs, &mut trailing_record);
        enrich_cpu_metadata_record(&specs, &mut malformed_record);

        let dashboard = cpu_metadata_dashboard(
            &specs,
            &[
                (record.metadata_id, record),
                (trailing_record.metadata_id, trailing_record),
                (malformed_record.metadata_id, malformed_record),
                (other_record.metadata_id, other_record),
            ]
            .into_iter()
            .collect(),
            [(7, (3, 120)), (8, (4, 300))].into_iter().collect(),
            CpuMetadataIntervalState::default(),
            7,
        );
        assert_eq!(dashboard.decoded_records, 3);
        assert_eq!(dashboard.decoded_values, 3);
        assert_eq!(dashboard.decoded_metadata_bytes, 29);
        assert_eq!(dashboard.undecoded_metadata_bytes, 2);
        assert_eq!(dashboard.decode_failed_records, 2);
        assert_eq!(dashboard.undecoded_records, 1);
        assert_eq!(
            dashboard.strings,
            vec![
                "/Test/Asset".to_owned(),
                "OtherOne".to_owned(),
                "Valid".to_owned()
            ]
        );
        assert_eq!(dashboard.spec_summaries.len(), 2);
        assert_eq!(dashboard.spec_summaries[0].spec_id, 8);
        assert_eq!(dashboard.spec_summaries[0].scopes, 4);
        assert_eq!(dashboard.spec_summaries[0].total_cycles, 300);
        let summary = dashboard
            .spec_summaries
            .iter()
            .find(|summary| summary.spec_id == 7)
            .unwrap();
        assert_eq!(
            summary.rendered_names,
            vec!["LoadPackage /Test/Asset 42".to_owned()]
        );
        assert_eq!(dashboard.samples.len(), 2);
        let sample = dashboard
            .samples
            .iter()
            .find(|sample| sample.spec_id == 7)
            .unwrap();
        assert_eq!(sample.name, "LoadPackage");
        assert_eq!(
            sample.rendered_name.as_deref(),
            Some("LoadPackage /Test/Asset 42")
        );
        assert_eq!(
            sample.fields["Name"],
            MetadataValue::Text {
                value: "/Test/Asset".to_owned()
            }
        );
        assert_eq!(
            sample.fields["Frame"],
            MetadataValue::Unsigned { value: 42 }
        );
    }

    #[test]
    fn restores_cpu_coroutine_scopes_across_suspend_resume() {
        let specs = [
            (
                1,
                CpuScopeSpec {
                    id: 1,
                    name: "Parent".to_owned(),
                    file: None,
                    line: None,
                },
            ),
            (
                2,
                CpuScopeSpec {
                    id: 2,
                    name: "Coroutine".to_owned(),
                    file: None,
                    line: None,
                },
            ),
        ]
        .into_iter()
        .collect::<BTreeMap<_, _>>();

        let mut data = Vec::new();
        push_varint(&mut data, (10 << 2) | 0b01);
        push_varint(&mut data, 1 << 1);
        push_varint(&mut data, (20 << 2) | 0b11);
        push_varint(&mut data, 7);
        push_varint(&mut data, 1);
        push_varint(&mut data, (30 << 2) | 0b01);
        push_varint(&mut data, 2 << 1);
        push_varint(&mut data, (40 << 2) | 0b10);
        push_varint(&mut data, 1);
        push_varint(&mut data, (50 << 2) | 0b11);
        push_varint(&mut data, 7);
        push_varint(&mut data, 1);
        push_varint(&mut data, 70 << 2);
        push_varint(&mut data, 80 << 2);

        let mut batches = CpuBatchSummary::default();
        let mut scope_totals = FxHashMap::default();
        let mut metadata_scope_totals = FxHashMap::default();
        let mut metadata_interval_state = CpuMetadataIntervalState::default();
        let mut metadata_stack_context = CpuMetadataStackRuntimeState::default();
        let mut thread_state = CpuBatchThreadState::default();
        let mut frame_scope_totals = FxHashMap::default();
        let mut frame_cycle_bounds = FxHashMap::default();
        let mut thread_scope_totals = FxHashMap::default();
        let mut state = CpuBatchDecodeState {
            batches: &mut batches,
            scope_totals: &mut scope_totals,
            metadata_scope_totals: &mut metadata_scope_totals,
            metadata_interval_state: &mut metadata_interval_state,
            metadata_stack_context: &mut metadata_stack_context,
            thread_state: &mut thread_state,
            batch_base_cycle: None,
            frame_scope_totals: &mut frame_scope_totals,
            frame_cycle_bounds: &mut frame_cycle_bounds,
            thread_scope_totals: &mut thread_scope_totals,
            timeline: None,
            thread_id: 0,
            cycle_frequency: None,
        };

        decode_cpu_batch(&data, &specs, &BTreeMap::new(), &mut state).unwrap();

        assert_eq!(batches.coroutine_records, 3);
        assert_eq!(batches.intervals, 2);
        assert_eq!(batches.unmatched_ends, 0);
        assert_eq!(cpu_batch_thread_state_unterminated_scopes(&thread_state), 0);
        assert_eq!(scope_totals[&2], (1, 30));
        assert_eq!(scope_totals[&1], (1, 70));
        assert_eq!(thread_scope_totals[&2], (1, 30));
    }

    #[test]
    fn keeps_cpu_scope_stack_and_cycle_state_across_batches() {
        let specs = [(
            1,
            CpuScopeSpec {
                id: 1,
                name: "CrossBatch".to_owned(),
                file: None,
                line: None,
            },
        )]
        .into_iter()
        .collect::<BTreeMap<_, _>>();
        let mut first_batch = Vec::new();
        push_varint(&mut first_batch, (100 << 2) | 0b01);
        push_varint(&mut first_batch, 1 << 1);
        let mut second_batch = Vec::new();
        push_varint(&mut second_batch, 50 << 2);

        let mut batches = CpuBatchSummary::default();
        let mut scope_totals = FxHashMap::default();
        let mut metadata_scope_totals = FxHashMap::default();
        let mut metadata_interval_state = CpuMetadataIntervalState::default();
        let mut metadata_stack_context = CpuMetadataStackRuntimeState::default();
        let mut thread_state = CpuBatchThreadState::default();
        let mut frame_scope_totals = FxHashMap::default();
        let mut frame_cycle_bounds = FxHashMap::default();
        let mut thread_scope_totals = FxHashMap::default();

        for batch in [&first_batch, &second_batch] {
            let mut state = CpuBatchDecodeState {
                batches: &mut batches,
                scope_totals: &mut scope_totals,
                metadata_scope_totals: &mut metadata_scope_totals,
                metadata_interval_state: &mut metadata_interval_state,
                metadata_stack_context: &mut metadata_stack_context,
                thread_state: &mut thread_state,
                batch_base_cycle: None,
                frame_scope_totals: &mut frame_scope_totals,
                frame_cycle_bounds: &mut frame_cycle_bounds,
                thread_scope_totals: &mut thread_scope_totals,
                timeline: None,
                thread_id: 0,
                cycle_frequency: None,
            };
            decode_cpu_batch(batch, &specs, &BTreeMap::new(), &mut state).unwrap();
        }

        assert_eq!(batches.count, 2);
        assert_eq!(batches.intervals, 1);
        assert_eq!(batches.unmatched_ends, 0);
        assert_eq!(cpu_batch_thread_state_unterminated_scopes(&thread_state), 0);
        assert_eq!(scope_totals[&1], (1, 50));
        assert_eq!(thread_scope_totals[&1], (1, 50));
    }

    #[test]
    fn uses_known_scope_cycle_as_initial_cpu_batch_base() {
        let specs = [(
            1,
            CpuScopeSpec {
                id: 1,
                name: "LateConnect".to_owned(),
                file: None,
                line: None,
            },
        )]
        .into_iter()
        .collect::<BTreeMap<_, _>>();
        let mut data = Vec::new();
        push_varint(&mut data, (25 << 2) | 0b01);
        push_varint(&mut data, 1 << 1);
        push_varint(&mut data, 15 << 2);

        let mut batches = CpuBatchSummary::default();
        let mut scope_totals = FxHashMap::default();
        let mut metadata_scope_totals = FxHashMap::default();
        let mut metadata_interval_state = CpuMetadataIntervalState::default();
        let mut metadata_stack_context = CpuMetadataStackRuntimeState::default();
        let mut thread_state = CpuBatchThreadState::default();
        let mut frame_scope_totals = FxHashMap::default();
        let mut frame_cycle_bounds = FxHashMap::default();
        let mut thread_scope_totals = FxHashMap::default();
        let mut state = CpuBatchDecodeState {
            batches: &mut batches,
            scope_totals: &mut scope_totals,
            metadata_scope_totals: &mut metadata_scope_totals,
            metadata_interval_state: &mut metadata_interval_state,
            metadata_stack_context: &mut metadata_stack_context,
            thread_state: &mut thread_state,
            batch_base_cycle: Some(1_000),
            frame_scope_totals: &mut frame_scope_totals,
            frame_cycle_bounds: &mut frame_cycle_bounds,
            thread_scope_totals: &mut thread_scope_totals,
            timeline: None,
            thread_id: 0,
            cycle_frequency: None,
        };

        decode_cpu_batch(&data, &specs, &BTreeMap::new(), &mut state).unwrap();

        assert_eq!(batches.intervals, 1);
        // Relative deltas 25 then 15 against base 1000 → absolute 1025..1040.
        assert_eq!(scope_totals[&1], (1, 15));
        assert_eq!(thread_state.last_cycle, 1_040);
        assert_eq!(batches.preamble_timeline_rebases, 0);
    }

    #[test]
    fn keeps_long_relative_cpu_intervals_after_start_cycle() {
        let specs = [(
            1,
            CpuScopeSpec {
                id: 1,
                name: "LongRunning".to_owned(),
                file: None,
                line: None,
            },
        )]
        .into_iter()
        .collect::<BTreeMap<_, _>>();

        let base = 1_000_000_000_u64;
        let freq = 1_000_000_u64;
        let mut data = Vec::new();
        // The first entry is StartCycle-anchored. The leave is a large relative
        // delta, not a flush-aligned absolute timestamp.
        push_varint(&mut data, (2 << 2) | 0b01);
        push_varint(&mut data, 1 << 1);
        push_varint(&mut data, (5 * freq) << 2);

        let mut batches = CpuBatchSummary::default();
        let mut scope_totals = FxHashMap::default();
        let mut metadata_scope_totals = FxHashMap::default();
        let mut metadata_interval_state = CpuMetadataIntervalState::default();
        let mut metadata_stack_context = CpuMetadataStackRuntimeState::default();
        let mut thread_state = CpuBatchThreadState::default();
        let mut frame_scope_totals = FxHashMap::default();
        let mut frame_cycle_bounds = FxHashMap::default();
        let mut thread_scope_totals = FxHashMap::default();
        let mut state = CpuBatchDecodeState {
            batches: &mut batches,
            scope_totals: &mut scope_totals,
            metadata_scope_totals: &mut metadata_scope_totals,
            metadata_interval_state: &mut metadata_interval_state,
            metadata_stack_context: &mut metadata_stack_context,
            thread_state: &mut thread_state,
            batch_base_cycle: Some(base),
            frame_scope_totals: &mut frame_scope_totals,
            frame_cycle_bounds: &mut frame_cycle_bounds,
            thread_scope_totals: &mut thread_scope_totals,
            timeline: None,
            thread_id: 0,
            cycle_frequency: Some(freq),
        };

        decode_cpu_batch(&data, &specs, &BTreeMap::new(), &mut state).unwrap();

        assert_eq!(batches.preamble_timeline_rebases, 0);
        assert_eq!(scope_totals[&1], (1, 5 * freq));
    }

    #[test]
    fn updates_last_cycle_after_preamble_metadata_enter() {
        let specs = [
            (
                1,
                CpuScopeSpec {
                    id: 1,
                    name: "Outer".to_owned(),
                    file: None,
                    line: None,
                },
            ),
            (
                7,
                CpuScopeSpec {
                    id: 7,
                    name: "Metadata".to_owned(),
                    file: None,
                    line: None,
                },
            ),
        ]
        .into_iter()
        .collect::<BTreeMap<_, _>>();
        let metadata = [(
            42,
            CpuMetadataRecord {
                metadata_id: 42,
                spec_id: 7,
                name: "Metadata".to_owned(),
                rendered_name: None,
                metadata_bytes: 0,
                decoded_metadata_bytes: 0,
                skipped_metadata_bytes: 0,
                decode_failed: false,
                values: Vec::new(),
                strings: Vec::new(),
            },
        )]
        .into_iter()
        .collect::<BTreeMap<_, _>>();

        let base = 1_000_000_000_u64;
        let freq = 1_000_000_u64;
        let absolute = base + 5 * freq;
        let mut data = Vec::new();
        push_varint(&mut data, (2 << 2) | 0b01);
        push_varint(&mut data, 1 << 1);
        // The first absolute record starts an inline metadata scope after the
        // StartCycle-anchored preamble.
        push_varint(&mut data, (absolute << 2) | 0b01);
        push_varint(&mut data, (42 << 1) | 1);
        push_varint(&mut data, 10 << 2);
        push_varint(&mut data, 10 << 2);

        let mut batches = CpuBatchSummary::default();
        let mut scope_totals = FxHashMap::default();
        let mut metadata_scope_totals = FxHashMap::default();
        let mut metadata_interval_state = CpuMetadataIntervalState::default();
        let mut metadata_stack_context = CpuMetadataStackRuntimeState::default();
        let mut thread_state = CpuBatchThreadState::default();
        let mut frame_scope_totals = FxHashMap::default();
        let mut frame_cycle_bounds = FxHashMap::default();
        let mut thread_scope_totals = FxHashMap::default();
        let mut state = CpuBatchDecodeState {
            batches: &mut batches,
            scope_totals: &mut scope_totals,
            metadata_scope_totals: &mut metadata_scope_totals,
            metadata_interval_state: &mut metadata_interval_state,
            metadata_stack_context: &mut metadata_stack_context,
            thread_state: &mut thread_state,
            batch_base_cycle: Some(base),
            frame_scope_totals: &mut frame_scope_totals,
            frame_cycle_bounds: &mut frame_cycle_bounds,
            thread_scope_totals: &mut thread_scope_totals,
            timeline: None,
            thread_id: 0,
            cycle_frequency: Some(freq),
        };

        decode_cpu_batch(&data, &specs, &metadata, &mut state).unwrap();

        assert_eq!(batches.preamble_timeline_rebases, 1);
        assert_eq!(scope_totals[&1], (1, 20));
        assert_eq!(metadata_scope_totals[&7], (1, 10));
        assert_eq!(thread_state.last_cycle, absolute + 20);
    }

    #[test]
    fn rebases_preamble_stack_when_absolute_timeline_jumps() {
        // Cold-start late-connect anchors near BaseCycle, then a flush-aligned
        // absolute timestamp jumps the thread clock. Without rebasing, the
        // outer scope duration spans the entire gap (fixture smoking gun).
        let specs = [
            (
                1,
                CpuScopeSpec {
                    id: 1,
                    name: "Outer".to_owned(),
                    file: None,
                    line: None,
                },
            ),
            (
                2,
                CpuScopeSpec {
                    id: 2,
                    name: "Inner".to_owned(),
                    file: None,
                    line: None,
                },
            ),
        ]
        .into_iter()
        .collect::<BTreeMap<_, _>>();

        let base = 1_000_000_u64;
        let freq = 1_000_000_u64; // 1s slack
        let mut preamble = Vec::new();
        // Enter Outer at relative 2 → absolute base+2 (preamble).
        push_varint(&mut preamble, (2 << 2) | 0b01);
        push_varint(&mut preamble, 1 << 1);
        // A buffer flush resets the writer's LastCycle. The next batch starts
        // with an absolute timestamp, then resumes relative deltas.
        let absolute = base + 5 * freq;
        let mut flush_aligned = Vec::new();
        push_varint(&mut flush_aligned, (absolute << 2) | 0b01);
        push_varint(&mut flush_aligned, 2 << 1);
        // Leave Inner after 10 cycles, then Outer.
        push_varint(&mut flush_aligned, 10 << 2);
        push_varint(&mut flush_aligned, 10 << 2);

        let mut batches = CpuBatchSummary::default();
        let mut scope_totals = FxHashMap::default();
        let mut metadata_scope_totals = FxHashMap::default();
        let mut metadata_interval_state = CpuMetadataIntervalState::default();
        let mut metadata_stack_context = CpuMetadataStackRuntimeState::default();
        let mut thread_state = CpuBatchThreadState::default();
        let mut frame_scope_totals = FxHashMap::default();
        let mut frame_cycle_bounds = FxHashMap::default();
        let mut thread_scope_totals = FxHashMap::default();
        let mut state = CpuBatchDecodeState {
            batches: &mut batches,
            scope_totals: &mut scope_totals,
            metadata_scope_totals: &mut metadata_scope_totals,
            metadata_interval_state: &mut metadata_interval_state,
            metadata_stack_context: &mut metadata_stack_context,
            thread_state: &mut thread_state,
            batch_base_cycle: Some(base),
            frame_scope_totals: &mut frame_scope_totals,
            frame_cycle_bounds: &mut frame_cycle_bounds,
            thread_scope_totals: &mut thread_scope_totals,
            timeline: None,
            thread_id: 0,
            cycle_frequency: Some(freq),
        };

        decode_cpu_batch(&preamble, &specs, &BTreeMap::new(), &mut state).unwrap();
        decode_cpu_batch(&flush_aligned, &specs, &BTreeMap::new(), &mut state).unwrap();

        assert_eq!(batches.count, 2);
        assert_eq!(batches.preamble_timeline_rebases, 1);
        assert_eq!(batches.implausible_duration_count, 0);
        // After rebase, Outer spans only the post-jump work (~20 cycles), not 5s.
        assert_eq!(scope_totals[&2], (1, 10));
        assert_eq!(scope_totals[&1], (1, 20));
        assert!(
            scope_totals[&1].1 < freq,
            "outer total must stay under one second after preamble rebase"
        );
    }

    #[test]
    fn nested_cpu_scopes_keep_inclusive_parent_ge_child() {
        let specs = [
            (
                1,
                CpuScopeSpec {
                    id: 1,
                    name: "Parent".to_owned(),
                    file: None,
                    line: None,
                },
            ),
            (
                2,
                CpuScopeSpec {
                    id: 2,
                    name: "Child".to_owned(),
                    file: None,
                    line: None,
                },
            ),
        ]
        .into_iter()
        .collect::<BTreeMap<_, _>>();

        let mut data = Vec::new();
        push_varint(&mut data, (100 << 2) | 0b01);
        push_varint(&mut data, 1 << 1);
        push_varint(&mut data, (110 << 2) | 0b01);
        push_varint(&mut data, 2 << 1);
        push_varint(&mut data, 140 << 2);
        push_varint(&mut data, 200 << 2);

        let mut batches = CpuBatchSummary::default();
        let mut scope_totals = FxHashMap::default();
        let mut metadata_scope_totals = FxHashMap::default();
        let mut metadata_interval_state = CpuMetadataIntervalState::default();
        let mut metadata_stack_context = CpuMetadataStackRuntimeState::default();
        let mut thread_state = CpuBatchThreadState::default();
        let mut frame_scope_totals = FxHashMap::default();
        let mut frame_cycle_bounds = FxHashMap::default();
        let mut thread_scope_totals = FxHashMap::default();
        let mut state = CpuBatchDecodeState {
            batches: &mut batches,
            scope_totals: &mut scope_totals,
            metadata_scope_totals: &mut metadata_scope_totals,
            metadata_interval_state: &mut metadata_interval_state,
            metadata_stack_context: &mut metadata_stack_context,
            thread_state: &mut thread_state,
            batch_base_cycle: None,
            frame_scope_totals: &mut frame_scope_totals,
            frame_cycle_bounds: &mut frame_cycle_bounds,
            thread_scope_totals: &mut thread_scope_totals,
            timeline: None,
            thread_id: 0,
            cycle_frequency: None,
        };

        decode_cpu_batch(&data, &specs, &BTreeMap::new(), &mut state).unwrap();

        assert_eq!(scope_totals[&2], (1, 30));
        assert_eq!(scope_totals[&1], (1, 100));
        assert!(scope_totals[&1].1 >= scope_totals[&2].1);
    }

    #[test]
    fn unmatched_cpu_leave_does_not_invent_duration() {
        let specs = [(
            1,
            CpuScopeSpec {
                id: 1,
                name: "Only".to_owned(),
                file: None,
                line: None,
            },
        )]
        .into_iter()
        .collect::<BTreeMap<_, _>>();

        let mut data = Vec::new();
        push_varint(&mut data, 50 << 2); // leave with empty stack

        let mut batches = CpuBatchSummary::default();
        let mut scope_totals = FxHashMap::default();
        let mut metadata_scope_totals = FxHashMap::default();
        let mut metadata_interval_state = CpuMetadataIntervalState::default();
        let mut metadata_stack_context = CpuMetadataStackRuntimeState::default();
        let mut thread_state = CpuBatchThreadState::default();
        let mut frame_scope_totals = FxHashMap::default();
        let mut frame_cycle_bounds = FxHashMap::default();
        let mut thread_scope_totals = FxHashMap::default();
        let mut state = CpuBatchDecodeState {
            batches: &mut batches,
            scope_totals: &mut scope_totals,
            metadata_scope_totals: &mut metadata_scope_totals,
            metadata_interval_state: &mut metadata_interval_state,
            metadata_stack_context: &mut metadata_stack_context,
            thread_state: &mut thread_state,
            batch_base_cycle: Some(1_000),
            frame_scope_totals: &mut frame_scope_totals,
            frame_cycle_bounds: &mut frame_cycle_bounds,
            thread_scope_totals: &mut thread_scope_totals,
            timeline: None,
            thread_id: 0,
            cycle_frequency: None,
        };

        decode_cpu_batch(&data, &specs, &BTreeMap::new(), &mut state).unwrap();

        assert_eq!(batches.unmatched_ends, 1);
        assert!(scope_totals.is_empty());
        assert_eq!(batches.implausible_duration_count, 0);
    }

    #[test]
    fn applies_restored_metadata_stack_to_plain_cpu_scopes() {
        let save_stack_event = test_event_type(
            68,
            "MetadataStack",
            "SaveStack",
            &[regular_field(0, 4, UINT32, "Id")],
        );
        let clear_scope_event = test_event_type(69, "MetadataStack", "ClearScope", &[]);
        let restore_stack_event = test_event_type(
            70,
            "MetadataStack",
            "RestoreStack",
            &[regular_field(0, 4, UINT32, "Id")],
        );

        let specs = [(
            1,
            CpuScopeSpec {
                id: 1,
                name: "PlainScope".to_owned(),
                file: None,
                line: None,
            },
        )]
        .into_iter()
        .collect::<BTreeMap<_, _>>();
        let metadata = [(
            42,
            CpuMetadataRecord {
                metadata_id: 42,
                spec_id: 7,
                name: "Frame".to_owned(),
                rendered_name: Some("Frame 366401".to_owned()),
                metadata_bytes: 0,
                decoded_metadata_bytes: 0,
                skipped_metadata_bytes: 0,
                decode_failed: false,
                values: Vec::new(),
                strings: Vec::new(),
            },
        )]
        .into_iter()
        .collect::<BTreeMap<_, _>>();

        let mut metadata_stack_context = CpuMetadataStackRuntimeState::default();
        metadata_stack_context.enter_inline(42);
        apply_metadata_stack_event_to_cpu_context(
            &save_stack_event,
            &123_u32.to_le_bytes(),
            &mut metadata_stack_context,
            0,
        )
        .unwrap();
        metadata_stack_context.leave_inline(42);
        apply_metadata_stack_event_to_cpu_context(
            &clear_scope_event,
            &[],
            &mut metadata_stack_context,
            0,
        )
        .unwrap();
        apply_metadata_stack_event_to_cpu_context(
            &restore_stack_event,
            &123_u32.to_le_bytes(),
            &mut metadata_stack_context,
            0,
        )
        .unwrap();

        let mut data = Vec::new();
        push_varint(&mut data, (10 << 2) | 0b01);
        push_varint(&mut data, 1 << 1);
        push_varint(&mut data, 30 << 2);

        let mut batches = CpuBatchSummary::default();
        let mut scope_totals = FxHashMap::default();
        let mut metadata_scope_totals = FxHashMap::default();
        let mut metadata_interval_state = CpuMetadataIntervalState::default();
        let mut thread_state = CpuBatchThreadState::default();
        let mut frame_scope_totals = FxHashMap::default();
        let mut frame_cycle_bounds = FxHashMap::default();
        let mut thread_scope_totals = FxHashMap::default();
        {
            let mut state = CpuBatchDecodeState {
                batches: &mut batches,
                scope_totals: &mut scope_totals,
                metadata_scope_totals: &mut metadata_scope_totals,
                metadata_interval_state: &mut metadata_interval_state,
                metadata_stack_context: &mut metadata_stack_context,
                thread_state: &mut thread_state,
                batch_base_cycle: None,
                frame_scope_totals: &mut frame_scope_totals,
                frame_cycle_bounds: &mut frame_cycle_bounds,
                thread_scope_totals: &mut thread_scope_totals,
                timeline: None,
                thread_id: 0,
                cycle_frequency: None,
            };

            decode_cpu_batch(&data, &specs, &metadata, &mut state).unwrap();
        }

        assert_eq!(batches.metadata_scopes, 0);
        assert_eq!(batches.restored_metadata_scopes, 1);
        assert_eq!(scope_totals[&1], (1, 20));
        assert_eq!(frame_scope_totals[&366401][&1], (1, 20));
        assert_eq!(frame_cycle_bounds[&366401], (10, 30));
        assert_eq!(metadata_scope_totals[&7], (1, 20));
        assert_eq!(
            metadata_interval_state.rendered_scope_totals[&(7, "Frame 366401".to_owned())],
            (1, 20)
        );
        assert_eq!(metadata_interval_state.samples.len(), 1);
        assert_eq!(
            metadata_interval_state.samples[0].attribution,
            CpuMetadataAttribution::RestoredStack
        );
        assert_eq!(
            metadata_interval_state.samples[0].rendered_name.as_deref(),
            Some("Frame 366401")
        );
    }

    #[test]
    fn summarizes_trace_channels() {
        let announce_event = test_event_type(
            29,
            "Trace",
            "ChannelAnnounce",
            &[
                regular_field(0, 4, UINT32, "Id"),
                regular_field(4, 1, UINT8, "IsEnabled"),
                regular_field(5, 1, UINT8, "ReadOnly"),
                regular_field(6, 0, ANSI_STRING, "Name"),
            ],
        );
        let toggle_event = test_event_type(
            30,
            "Trace",
            "ChannelToggle",
            &[
                regular_field(0, 4, UINT32, "Id"),
                regular_field(4, 1, UINT8, "IsEnabled"),
            ],
        );

        let mut announce_data = Vec::new();
        announce_data.extend_from_slice(&7_u32.to_le_bytes());
        announce_data.push(0);
        announce_data.push(1);
        announce_data.extend_from_slice(&aux(3, b"Concert"));
        announce_data.push(3);
        let announce = decode_trace_channel_announce(&announce_event, &announce_data, 0).unwrap();

        let mut toggle_on_data = Vec::new();
        toggle_on_data.extend_from_slice(&7_u32.to_le_bytes());
        toggle_on_data.push(1);
        let toggle_on = decode_trace_channel_toggle(&toggle_event, &toggle_on_data, 0).unwrap();

        let mut unknown_toggle_data = Vec::new();
        unknown_toggle_data.extend_from_slice(&8_u32.to_le_bytes());
        unknown_toggle_data.push(1);
        let unknown_toggle =
            decode_trace_channel_toggle(&toggle_event, &unknown_toggle_data, 0).unwrap();

        let mut states = BTreeMap::<u32, TraceChannelState>::new();
        states.entry(announce.id).or_default().announce(announce);
        states
            .entry(toggle_on.id)
            .or_default()
            .toggle(toggle_on.is_enabled);
        states
            .entry(unknown_toggle.id)
            .or_default()
            .toggle(unknown_toggle.is_enabled);

        let dashboard = trace_channel_dashboard(states);
        assert_eq!(dashboard.count, 2);
        assert_eq!(dashboard.enabled, 2);
        assert_eq!(dashboard.read_only, 1);
        assert_eq!(dashboard.toggles, 2);
        let concert = dashboard
            .channels
            .iter()
            .find(|channel| channel.name.as_deref() == Some("Concert"))
            .unwrap();
        assert_eq!(concert.id, 7);
        assert!(concert.is_enabled);
        assert!(concert.read_only);
        assert_eq!(concert.toggle_count, 1);
        assert!(dashboard.channels.iter().any(|channel| channel.id == 8
            && channel.name.is_none()
            && channel.is_enabled
            && channel.toggle_count == 1));
    }

    #[test]
    fn summarizes_thread_groups() {
        let begin_event = test_event_type(
            31,
            "$Trace",
            "ThreadGroupBegin",
            &[regular_field(0, 0, ANSI_STRING, "Name")],
        );

        let mut background_data = Vec::new();
        background_data.extend_from_slice(&aux(0, b"BackgroundThreadPool"));
        background_data.push(3);
        let background = decode_thread_group_begin(&begin_event, &background_data, 0).unwrap();

        let mut io_data = Vec::new();
        io_data.extend_from_slice(&aux(0, b"IOThreadPool"));
        io_data.push(3);
        let io = decode_thread_group_begin(&begin_event, &io_data, 0).unwrap();

        let mut state = ThreadGroupState::default();
        state.begin(background.clone());
        state.begin(io);
        state.end();
        state.end();
        state.end();
        state.begin(background);

        let dashboard = state.dashboard();
        assert_eq!(dashboard.begin_events, 3);
        assert_eq!(dashboard.end_events, 3);
        assert_eq!(dashboard.unmatched_ends, 1);
        assert_eq!(dashboard.unclosed_groups, 1);
        let background = dashboard
            .groups
            .iter()
            .find(|group| group.name == "BackgroundThreadPool")
            .unwrap();
        assert_eq!(background.begin_count, 2);
        assert_eq!(background.end_count, 1);
        assert!(!background.balanced);
        assert!(
            dashboard
                .groups
                .iter()
                .any(|group| group.name == "IOThreadPool"
                    && group.begin_count == 1
                    && group.end_count == 1
                    && group.balanced)
        );
    }

    #[test]
    fn summarizes_cpu_named_events() {
        let frame_event = test_event_type(
            26,
            "Cpu",
            "Frame",
            &[regular_field(0, 0, WIDE_STRING, "Name")],
        );
        let buffer_event = test_event_type(
            27,
            "Cpu",
            "FRDGBufferPool_CreateBuffer",
            &[
                regular_field(0, 0, WIDE_STRING, "Name"),
                regular_field(0, 4, UINT32, "SizeInBytes"),
            ],
        );

        let mut frame_data = Vec::new();
        frame_data.extend_from_slice(&aux(0, &wide(" 366401")));
        frame_data.push(3);
        let mut buffer_data = Vec::new();
        buffer_data.extend_from_slice(&65516_u32.to_le_bytes());
        buffer_data.extend_from_slice(&aux(0, &wide("SlateElementsVertexBuffer")));
        buffer_data.push(3);

        let mut states = BTreeMap::new();
        states
            .entry(frame_event.event.clone())
            .or_insert_with(CpuNamedEventState::default)
            .record(&frame_event, &frame_data, 2)
            .unwrap();
        states
            .entry(frame_event.event.clone())
            .or_insert_with(CpuNamedEventState::default)
            .record(&frame_event, &frame_data, 2)
            .unwrap();
        states
            .entry(buffer_event.event.clone())
            .or_insert_with(CpuNamedEventState::default)
            .record(&buffer_event, &buffer_data, 64)
            .unwrap();

        let summaries = cpu_named_event_summaries(states);
        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].event, "Frame");
        assert_eq!(summaries[0].observed_count, 2);
        assert_eq!(summaries[0].sample.as_ref().unwrap().thread_id, 2);
        assert_eq!(
            summaries[0].sample.as_ref().unwrap().fields["Name"],
            SampleValue::String(" 366401".to_owned())
        );
        let buffer = summaries
            .iter()
            .find(|summary| summary.event == "FRDGBufferPool_CreateBuffer")
            .unwrap();
        assert_eq!(
            buffer.sample.as_ref().unwrap().fields["Name"],
            SampleValue::String("SlateElementsVertexBuffer".to_owned())
        );
        assert_eq!(
            buffer.sample.as_ref().unwrap().fields["SizeInBytes"],
            SampleValue::Unsigned(65516)
        );
    }

    #[test]
    fn rejects_invalid_utf16_wide_event_samples() {
        let event = test_event_type(
            26,
            "Cpu",
            "Frame",
            &[regular_field(0, 0, WIDE_STRING, "Name")],
        );
        let mut data = Vec::new();
        data.extend_from_slice(&aux(0, &0xd800_u16.to_le_bytes()));
        data.push(3);

        let error = decode_event_sample(&event, &RawSample { thread_id: 2, data })
            .expect_err("invalid UTF-16 should be rejected");

        assert_eq!(error.kind(), TraceErrorKind::MalformedData);
        assert_eq!(error.path(), "Frame.Name");
        assert!(error.detail().contains("invalid UTF-16"));
    }

    #[test]
    fn summarizes_bookmark_events_and_regions() {
        let bookmark_spec_event = test_event_type(
            30,
            "Misc",
            "BookmarkSpec",
            &[
                regular_field(0, 8, UINT64, "BookmarkPoint"),
                regular_field(8, 4, INT32, "Line"),
                regular_field(12, 0, WIDE_STRING, "FormatString"),
                regular_field(12, 0, ANSI_STRING, "FileName"),
            ],
        );
        let bookmark_event = test_event_type(
            31,
            "Misc",
            "Bookmark",
            &[
                regular_field(0, 8, UINT64, "Cycle"),
                regular_field(8, 8, UINT64, "BookmarkPoint"),
                regular_field(16, 0, ARRAY, "FormatArgs"),
                regular_field(16, 4, UINT32, "CallstackId"),
            ],
        );
        let region_begin_event = test_event_type(
            32,
            "Misc",
            "RegionBegin",
            &[
                regular_field(0, 8, UINT64, "Cycle"),
                regular_field(8, 0, WIDE_STRING, "RegionName"),
                regular_field(8, 0, WIDE_STRING, "Category"),
            ],
        );
        let region_end_event = test_event_type(
            33,
            "Misc",
            "RegionEnd",
            &[
                regular_field(0, 8, UINT64, "Cycle"),
                regular_field(8, 0, WIDE_STRING, "RegionName"),
            ],
        );
        let region_begin_with_id_event = test_event_type(
            34,
            "Misc",
            "RegionBeginWithId",
            &[
                regular_field(0, 8, UINT64, "CycleAndId"),
                regular_field(8, 0, WIDE_STRING, "RegionName"),
                regular_field(8, 0, WIDE_STRING, "Category"),
            ],
        );
        let region_end_with_id_event = test_event_type(
            35,
            "Misc",
            "RegionEndWithId",
            &[
                regular_field(0, 8, UINT64, "Cycle"),
                regular_field(8, 8, UINT64, "RegionId"),
            ],
        );

        let mut spec_data = Vec::new();
        spec_data.extend_from_slice(&0xabc_u64.to_le_bytes());
        spec_data.extend_from_slice(&77_i32.to_le_bytes());
        spec_data.extend_from_slice(&aux(2, &wide("Loading %s")));
        spec_data.extend_from_slice(&aux(3, b"Source.cpp"));
        spec_data.push(3);
        let spec = decode_bookmark_spec(&bookmark_spec_event, &spec_data, 0).unwrap();

        let mut bookmark_specs = BTreeMap::new();
        bookmark_specs.insert(spec.bookmark_point, spec);
        let mut bookmark_states = BTreeMap::new();
        let mut unresolved_bookmark_events = 0;
        let mut region_state = RegionState::default();

        let format_args = crate::utrace_format_args::encode::encode_args(&[
            crate::utrace_format_args::encode::EncodedPart::Wide("PackageA"),
        ]);
        let mut bookmark_data = Vec::new();
        bookmark_data.extend_from_slice(&100_u64.to_le_bytes());
        bookmark_data.extend_from_slice(&0xabc_u64.to_le_bytes());
        bookmark_data.extend_from_slice(&42_u32.to_le_bytes());
        bookmark_data.extend_from_slice(&aux(2, &format_args));
        bookmark_data.push(3);
        decode_misc_annotation_event(
            &bookmark_event,
            &bookmark_data,
            &bookmark_specs,
            &mut bookmark_states,
            &mut unresolved_bookmark_events,
            &mut region_state,
            0,
        )
        .unwrap();

        let mut region_begin_data = Vec::new();
        region_begin_data.extend_from_slice(&200_u64.to_le_bytes());
        region_begin_data.extend_from_slice(&aux(1, &wide("Cook")));
        region_begin_data.extend_from_slice(&aux(2, &wide("Editor")));
        region_begin_data.push(3);
        decode_misc_annotation_event(
            &region_begin_event,
            &region_begin_data,
            &bookmark_specs,
            &mut bookmark_states,
            &mut unresolved_bookmark_events,
            &mut region_state,
            0,
        )
        .unwrap();

        let mut region_end_data = Vec::new();
        region_end_data.extend_from_slice(&250_u64.to_le_bytes());
        region_end_data.extend_from_slice(&aux(1, &wide("Cook")));
        region_end_data.push(3);
        decode_misc_annotation_event(
            &region_end_event,
            &region_end_data,
            &bookmark_specs,
            &mut bookmark_states,
            &mut unresolved_bookmark_events,
            &mut region_state,
            0,
        )
        .unwrap();

        let mut region_begin_with_id_data = Vec::new();
        region_begin_with_id_data.extend_from_slice(&300_u64.to_le_bytes());
        region_begin_with_id_data.extend_from_slice(&aux(1, &wide("Build")));
        region_begin_with_id_data.extend_from_slice(&aux(2, &wide("Tools")));
        region_begin_with_id_data.push(3);
        decode_misc_annotation_event(
            &region_begin_with_id_event,
            &region_begin_with_id_data,
            &bookmark_specs,
            &mut bookmark_states,
            &mut unresolved_bookmark_events,
            &mut region_state,
            0,
        )
        .unwrap();

        let mut region_end_with_id_data = Vec::new();
        region_end_with_id_data.extend_from_slice(&360_u64.to_le_bytes());
        region_end_with_id_data.extend_from_slice(&300_u64.to_le_bytes());
        decode_misc_annotation_event(
            &region_end_with_id_event,
            &region_end_with_id_data,
            &bookmark_specs,
            &mut bookmark_states,
            &mut unresolved_bookmark_events,
            &mut region_state,
            0,
        )
        .unwrap();

        let dashboard = annotation_dashboard(
            bookmark_specs,
            bookmark_states,
            unresolved_bookmark_events,
            region_state,
        );
        assert_eq!(dashboard.bookmarks.specs, 1);
        assert_eq!(dashboard.bookmarks.events, 1);
        assert_eq!(
            dashboard.bookmarks.format_args_bytes,
            u64::try_from(format_args.len()).unwrap()
        );
        assert_eq!(dashboard.bookmarks.bookmarks[0].format_string, "Loading %s");
        assert_eq!(
            dashboard.bookmarks.bookmarks[0].sample_message.as_deref(),
            Some("Loading PackageA")
        );
        assert_eq!(
            dashboard.bookmarks.bookmarks[0].sample_args,
            vec!["PackageA".to_owned()]
        );
        assert_eq!(
            dashboard.bookmarks.bookmarks[0].file.as_deref(),
            Some("Source.cpp")
        );
        assert_eq!(dashboard.bookmarks.bookmarks[0].line, Some(77));
        assert_eq!(dashboard.bookmarks.bookmarks[0].callstack_count, 1);
        assert_eq!(dashboard.bookmarks.bookmarks[0].callstack_samples.len(), 1);
        assert_eq!(
            dashboard.bookmarks.bookmarks[0].callstack_samples[0].callstack_id,
            42
        );
        assert_eq!(
            dashboard.bookmarks.bookmarks[0].callstack_samples[0].cycle,
            100
        );
        assert_eq!(dashboard.regions.begin_events, 2);
        assert_eq!(dashboard.regions.end_events, 2);
        assert_eq!(dashboard.regions.completed, 2);
        assert_eq!(dashboard.regions.with_id_begin_events, 1);
        assert_eq!(dashboard.regions.with_id_end_events, 1);
        assert_eq!(dashboard.regions.regions.len(), 2);
        assert!(
            dashboard
                .regions
                .regions
                .iter()
                .any(|region| region.name == "Cook"
                    && region.category.as_deref() == Some("Editor")
                    && region.total_cycles == 50)
        );
        assert!(
            dashboard
                .regions
                .regions
                .iter()
                .any(|region| region.name == "Build"
                    && region.category.as_deref() == Some("Tools")
                    && region.total_cycles == 60)
        );
    }

    #[test]
    fn summarizes_log_categories_specs_and_messages() {
        let category_event = test_event_type(
            40,
            "Logging",
            "LogCategory",
            &[
                regular_field(0, 8, UINT64, "CategoryPointer"),
                regular_field(8, 1, UINT8, "DefaultVerbosity"),
                regular_field(9, 0, ANSI_STRING, "Name"),
            ],
        );
        let spec_event = test_event_type(
            41,
            "Logging",
            "LogMessageSpec",
            &[
                regular_field(0, 8, UINT64, "LogPoint"),
                regular_field(8, 8, UINT64, "CategoryPointer"),
                regular_field(16, 4, INT32, "Line"),
                regular_field(20, 1, UINT8, "Verbosity"),
                regular_field(21, 0, ANSI_STRING, "FileName"),
                regular_field(21, 0, WIDE_STRING, "FormatString"),
            ],
        );
        let message_event = test_event_type(
            42,
            "Logging",
            "LogMessage",
            &[
                regular_field(0, 8, UINT64, "LogPoint"),
                regular_field(8, 8, UINT64, "Cycle"),
                regular_field(16, 0, ARRAY, "FormatArgs"),
            ],
        );

        let mut category_data = Vec::new();
        category_data.extend_from_slice(&0xca11_u64.to_le_bytes());
        category_data.push(5);
        category_data.extend_from_slice(&aux(2, b"LogTemp"));
        category_data.push(3);
        let (category_pointer, category) =
            decode_log_category(&category_event, &category_data, 0).unwrap();
        assert_eq!(category_pointer, 0xca11);
        let mut categories = BTreeMap::new();
        categories.insert(category_pointer, category);

        let mut spec_data = Vec::new();
        spec_data.extend_from_slice(&0x10c_u64.to_le_bytes());
        spec_data.extend_from_slice(&0xca11_u64.to_le_bytes());
        spec_data.extend_from_slice(&123_i32.to_le_bytes());
        spec_data.push(3);
        spec_data.extend_from_slice(&aux(4, b"Source.cpp"));
        spec_data.extend_from_slice(&aux(5, &wide("Hello %s")));
        spec_data.push(3);
        let (log_point, spec) = decode_log_message_spec(&spec_event, &spec_data, 0).unwrap();
        assert_eq!(log_point, 0x10c);
        let mut specs = BTreeMap::new();
        specs.insert(log_point, spec);

        let mut states = BTreeMap::new();
        let mut unresolved_messages = 0;
        let format_args = crate::utrace_format_args::encode::encode_args(&[
            crate::utrace_format_args::encode::EncodedPart::Wide("World"),
        ]);
        let mut message_data = Vec::new();
        message_data.extend_from_slice(&0x10c_u64.to_le_bytes());
        message_data.extend_from_slice(&900_u64.to_le_bytes());
        message_data.extend_from_slice(&aux(2, &format_args));
        message_data.push(3);
        decode_log_message(
            &message_event,
            &message_data,
            &specs,
            &mut states,
            &mut unresolved_messages,
            0,
        )
        .unwrap();

        // A message with no matching spec is counted as unresolved.
        let mut orphan_data = Vec::new();
        orphan_data.extend_from_slice(&0xdead_u64.to_le_bytes());
        orphan_data.extend_from_slice(&950_u64.to_le_bytes());
        orphan_data.extend_from_slice(&aux(2, &[9]));
        orphan_data.push(3);
        decode_log_message(
            &message_event,
            &orphan_data,
            &specs,
            &mut states,
            &mut unresolved_messages,
            0,
        )
        .unwrap();

        let dashboard = log_dashboard(categories, specs, states, unresolved_messages);
        assert_eq!(dashboard.categories, 1);
        assert_eq!(dashboard.message_specs, 1);
        assert_eq!(dashboard.messages, 2);
        assert_eq!(
            dashboard.format_args_bytes,
            u64::try_from(format_args.len() + 1).unwrap()
        );
        assert_eq!(dashboard.unresolved_messages, 1);
        assert_eq!(dashboard.specs_with_unknown_category, 0);
        assert_eq!(
            dashboard.verbosity,
            vec![LogVerbosityCount {
                verbosity: LogVerbosity::Warning,
                message_specs: 1,
                messages: 1,
            }]
        );
        let message = &dashboard.top_messages[0];
        assert_eq!(message.log_point, 0x10c);
        assert_eq!(message.category.as_deref(), Some("LogTemp"));
        assert_eq!(message.verbosity, LogVerbosity::Warning);
        assert_eq!(message.format_string, "Hello %s");
        assert_eq!(message.file.as_deref(), Some("Source.cpp"));
        assert_eq!(message.line, Some(123));
        assert_eq!(message.count, 1);
        assert_eq!(
            message.format_args_bytes,
            u64::try_from(format_args.len()).unwrap()
        );
        assert_eq!(message.sample_args, vec!["World".to_owned()]);
        assert_eq!(message.sample_message.as_deref(), Some("Hello World"));
        assert_eq!(message.first_cycle, Some(900));
        let category = &dashboard.top_categories[0];
        assert_eq!(category.name, "LogTemp");
        assert_eq!(category.default_verbosity, LogVerbosity::Log);
        assert_eq!(category.message_specs, 1);
        assert_eq!(category.messages, 1);
    }

    #[test]
    fn decodes_diagnostics_session() {
        let session_event = test_event_type(
            50,
            "Diagnostics",
            "Session2",
            &[
                regular_field(0, 0, ANSI_STRING, "Platform"),
                regular_field(0, 0, ANSI_STRING, "AppName"),
                regular_field(0, 0, WIDE_STRING, "ProjectName"),
                regular_field(0, 0, WIDE_STRING, "CommandLine"),
                regular_field(0, 0, WIDE_STRING, "Branch"),
                regular_field(0, 0, WIDE_STRING, "BuildVersion"),
                regular_field(0, 4, UINT32, "Changelist"),
                regular_field(4, 1, UINT8, "ConfigurationType"),
                regular_field(5, 1, UINT8, "TargetType"),
                regular_field(6, 0, ARRAY, "InstanceId"),
                regular_field(6, 0, ANSI_STRING, "VFSPaths"),
            ],
        );

        let mut data = Vec::new();
        data.extend_from_slice(&12345_u32.to_le_bytes());
        data.push(3);
        data.push(4);
        data.extend_from_slice(&aux(0, b"Win64"));
        data.extend_from_slice(&aux(1, b"UnrealEditor"));
        data.extend_from_slice(&aux(2, &wide("MyGame")));
        data.extend_from_slice(&aux(3, &wide("-run")));
        data.extend_from_slice(&aux(4, &wide("main")));
        data.extend_from_slice(&aux(5, &wide("UE5-CL-1")));
        data.extend_from_slice(&aux(9, &[0xaa; 16]));
        data.extend_from_slice(&aux(10, b"A;B;C"));
        data.push(3);

        let session = decode_session(&session_event, &data, 0).unwrap();
        assert_eq!(session.platform, "Win64");
        assert_eq!(session.app_name, "UnrealEditor");
        assert_eq!(session.project_name, "MyGame");
        assert_eq!(session.command_line, "-run");
        assert_eq!(session.branch, "main");
        assert_eq!(session.build_version, "UE5-CL-1");
        assert_eq!(session.changelist, 12345);
        assert_eq!(session.configuration, BuildConfiguration::Development);
        assert_eq!(session.target_type, BuildTargetType::Editor);
        assert_eq!(
            session.instance_id.as_deref(),
            Some("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa")
        );
        assert_eq!(session.vfs_paths, vec!["A", "B", "C"]);
    }

    #[test]
    fn dashboard_summarizes_unmodeled_trace_families() {
        let raw_decl = new_event(
            90,
            0x03,
            "Object",
            "Class",
            &[
                regular_field(0, 8, UINT64, "Cycle"),
                regular_field(8, 8, UINT64, "ClassPointer"),
                regular_field(16, 0, ANSI_STRING, "Name"),
            ],
        );
        let mut raw_data = Vec::new();
        raw_data.extend_from_slice(&1000_u64.to_le_bytes());
        raw_data.extend_from_slice(&0xabc_u64.to_le_bytes());
        raw_data.extend_from_slice(&aux(2, b"MyActor"));
        raw_data.push(3);

        let bytes = trace_with_events(&[
            important_event(0, &raw_decl),
            important_event(90, &raw_data),
        ]);
        let dashboard = dashboard(&bytes).unwrap();

        assert_eq!(dashboard.unmodeled.event_types, 1);
        assert_eq!(dashboard.unmodeled.observed_events, 1);
        let event = &dashboard.unmodeled.events[0];
        assert_eq!(event.logger, "Object");
        assert_eq!(event.event, "Class");
        assert_eq!(event.observed_count, 1);
        let sample = event.sample.as_ref().unwrap();
        assert_eq!(sample.thread_id, 0);
        assert_eq!(sample.fields["Cycle"], SampleValue::Unsigned(1000));
        assert_eq!(sample.fields["ClassPointer"], SampleValue::Unsigned(0xabc));
        assert_eq!(
            sample.fields["Name"],
            SampleValue::String("MyActor".to_owned())
        );
    }

    #[test]
    fn dashboard_decodes_callstack_catalog_as_partial() {
        let callstack_uid = 90_u16;
        let declaration = new_event(
            callstack_uid,
            0x07,
            "Memory",
            "CallstackSpec",
            &[
                regular_field(0, 4, UINT32, "CallstackId"),
                regular_field(4, 0, ARRAY, "Frames"),
            ],
        );

        let mut frames = Vec::new();
        frames.extend_from_slice(&0x1000_u64.to_le_bytes());
        frames.extend_from_slice(&0x2000_u64.to_le_bytes());
        frames.extend_from_slice(&0x3000_u64.to_le_bytes());

        let mut payload = Vec::new();
        payload.extend_from_slice(&7_u32.to_le_bytes());
        payload.extend_from_slice(&aux(1, &frames));
        payload.push(3);

        let bytes = trace_with_events(&[
            important_event(0, &declaration),
            important_event(callstack_uid, &payload),
        ]);
        let dashboard = dashboard(&bytes).unwrap();
        let inventory = inventory(&bytes).unwrap();

        assert_eq!(dashboard.callstacks.observed, 1);
        assert_eq!(dashboard.callstacks.retained, 1);
        assert_eq!(dashboard.callstacks.stacks.len(), 1);
        assert_eq!(dashboard.callstacks.stacks[0].id, 7);
        assert_eq!(
            dashboard.callstacks.stacks[0].frames,
            vec![
                "0x1000".to_owned(),
                "0x2000".to_owned(),
                "0x3000".to_owned()
            ]
        );
        assert!(
            !dashboard
                .unmodeled
                .events
                .iter()
                .any(|event| event.logger == "Memory" && event.event == "CallstackSpec")
        );
        let entry = inventory
            .events
            .iter()
            .find(|event| event.logger == "Memory" && event.event == "CallstackSpec")
            .expect("CallstackSpec should be declared");
        assert_eq!(entry.decode_status, DecodeStatus::Partial);
        assert_eq!(entry.observed_count, 1);
    }

    #[test]
    fn callstack_joins_keep_frames_only_in_catalog() {
        let mut callstacks = CallstackProvider::default();
        callstacks.record(crate::utrace_callstacks::DecodedCallstackSpec {
            id: CallstackId(9),
            frames: vec![0xaa, 0xbb],
            declared_frame_count: 2,
            frames_truncated: false,
        });

        let mut memory = MemoryProvider::default();
        memory.record_allocation(MemoryAllocation {
            address: 0x10,
            size: 32,
            root_heap: 0,
            callstack_id: 9,
            kind: MemoryAllocationKind::Alloc,
        });
        memory.record_allocation(MemoryAllocation {
            address: 0x20,
            size: 8,
            root_heap: 0,
            callstack_id: 0,
            kind: MemoryAllocationKind::Alloc,
        });

        let mut memory_dashboard = memory.dashboard();
        let mut bookmark_dashboard = BookmarkDashboard {
            specs: 1,
            events: 1,
            format_args_bytes: 0,
            unresolved_events: 0,
            bookmarks: vec![BookmarkSummary {
                bookmark_point: 1,
                format_string: "mark".to_owned(),
                file: None,
                line: None,
                count: 1,
                format_args_bytes: 0,
                sample_args: Vec::new(),
                sample_message: None,
                first_cycle: Some(50),
                last_cycle: Some(50),
                callstack_count: 1,
                callstack_samples: vec![BookmarkCallstackSample {
                    cycle: 50,
                    callstack_id: 9,
                    callstack: CallstackResolution::Missing,
                }],
            }],
        };

        apply_callstack_resolutions(
            &mut memory_dashboard,
            &mut bookmark_dashboard,
            &mut callstacks,
        );
        let callstack_dashboard = callstacks.dashboard_mapped(|_| None);

        assert_eq!(
            memory_dashboard.allocs.samples[0].callstack,
            CallstackResolution::Resolved
        );
        assert_eq!(
            memory_dashboard.allocs.samples[1].callstack,
            CallstackResolution::None
        );
        assert_eq!(
            bookmark_dashboard.bookmarks[0].callstack_samples[0].callstack,
            CallstackResolution::Resolved
        );
        assert_eq!(callstack_dashboard.stacks[0].frames.len(), 2);

        let memory_json = serde_json::to_value(&memory_dashboard.allocs.samples[0]).unwrap();
        let bookmark_json =
            serde_json::to_value(&bookmark_dashboard.bookmarks[0].callstack_samples[0]).unwrap();
        let catalog_json = serde_json::to_value(&callstack_dashboard.stacks[0]).unwrap();
        assert!(memory_json.get("frames").is_none());
        assert!(bookmark_json.get("frames").is_none());
        assert_eq!(catalog_json["frames"], serde_json::json!(["0xaa", "0xbb"]));
        assert_eq!(callstack_dashboard.unresolved_references, 0);
    }

    #[test]
    fn event_coverage_table_is_consistent() {
        // No duplicate (logger, event) rows.
        let mut seen = std::collections::BTreeSet::new();
        for entry in EVENT_COVERAGE {
            assert!(
                seen.insert((entry.logger, entry.event)),
                "duplicate coverage row for {}.{}",
                entry.logger,
                entry.event
            );
            // decode_status_for must agree with the table it is derived from.
            let event = test_event_type(0, entry.logger, entry.event, &[]);
            assert_eq!(decode_status_for(&event), entry.status);
        }
        // Anything not in the table is raw.
        let unknown = test_event_type(0, "NoSuchLogger", "NoSuchEvent", &[]);
        assert_eq!(decode_status_for(&unknown), DecodeStatus::Raw);
        // Dynamic Cpu logger events are decoded generically rather than listed individually.
        let cpu = test_event_type(0, "Cpu", "Frame", &[]);
        assert_eq!(decode_status_for(&cpu), DecodeStatus::Partial);
    }

    #[test]
    fn coverage_cross_references_registry_and_universe() {
        let decoded_decl = new_event(
            10,
            0x05,
            "$Trace",
            "NewTrace",
            &[regular_field(0, 8, UINT64, "StartCycle")],
        );
        let raw_decl = new_event(11, 0x05, "Foo", "Bar", &[regular_field(0, 4, UINT32, "Id")]);
        let bytes = trace_with_events(&[
            important_event(0, &decoded_decl),
            important_event(0, &raw_decl),
        ]);

        let universe = ["Foo.Bar".to_owned(), "Extra.Event".to_owned()]
            .into_iter()
            .collect::<std::collections::BTreeSet<String>>();
        let coverage = coverage(&bytes, Some(&universe)).unwrap();

        assert_eq!(coverage.summary.declared_event_types, 2);
        assert_eq!(coverage.summary.decoded_event_types, 1);
        assert_eq!(coverage.summary.raw_event_types, 1);
        assert!(coverage.events.iter().any(|entry| entry.logger == "Foo"
            && entry.event == "Bar"
            && entry.status == DecodeStatus::Raw
            && entry.note.is_none()));
        assert!(
            coverage
                .events
                .iter()
                .any(|entry| entry.event == "NewTrace" && entry.note.is_some())
        );

        let universe = coverage.universe.unwrap();
        assert_eq!(universe.total, 2);
        assert_eq!(universe.declared_in_trace, 1);
        assert_eq!(universe.unseen, vec!["Extra.Event".to_owned()]);
        assert_eq!(universe.not_in_universe, vec!["$Trace.NewTrace".to_owned()]);
    }

    #[test]
    fn decodes_cpu_event_spec_payload() {
        let event = test_event_type(
            1,
            "CpuProfiler",
            "EventSpec",
            &[
                regular_field(0, 4, UINT32, "Id"),
                regular_field(4, 0, ANSI_STRING, "Name"),
                regular_field(4, 0, ANSI_STRING, "File"),
                regular_field(4, 4, UINT32, "Line"),
            ],
        );
        let mut data = Vec::new();
        data.extend_from_slice(&17_u32.to_le_bytes());
        data.extend_from_slice(&42_u32.to_le_bytes());
        data.extend_from_slice(&aux(1, b"Tick"));
        data.extend_from_slice(&aux(2, b"Actor.cpp"));

        let spec = decode_cpu_event_spec(&event, &data, 0).expect("decode EventSpec");

        assert_eq!(spec.id, 17);
        assert_eq!(spec.name, "Tick");
        assert_eq!(spec.file.as_deref(), Some("Actor.cpp"));
        assert_eq!(spec.line, Some(42));
    }

    #[test]
    fn decodes_begin_and_end_frame_payloads() {
        let event = test_event_type(
            1,
            "Misc",
            "BeginFrame",
            &[
                regular_field(0, 8, UINT64, "Cycle"),
                regular_field(8, 1, UINT8, "FrameType"),
            ],
        );
        let mut data = Vec::new();
        data.extend_from_slice(&123_456_u64.to_le_bytes());
        data.push(2);

        let begin = decode_frame_marker(&event, &data, 0, 7, FrameMarkerKind::Begin)
            .expect("decode begin frame");
        let end = decode_frame_marker(&event, &data, 0, 7, FrameMarkerKind::End)
            .expect("decode end frame");

        assert_eq!(begin.kind, FrameMarkerKind::Begin);
        assert_eq!(end.kind, FrameMarkerKind::End);
        assert_eq!(begin.cycle, 123_456);
        assert_eq!(begin.frame_type, 2);
        assert_eq!(begin.thread_id, 7);
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

    fn test_event_type(uid: u16, logger: &str, event: &str, fields: &[TestField]) -> EventTypeInfo {
        EventTypeInfo {
            uid,
            logger: logger.to_owned(),
            event: event.to_owned(),
            flags: EventFlags {
                important: false,
                maybe_has_aux: true,
                no_sync: false,
                definition: false,
            },
            fields: fields
                .iter()
                .map(|field| FieldInfo {
                    name: field.name.to_owned(),
                    offset: field.offset,
                    size: field.size,
                    family: FieldFamily::Regular,
                    type_name: type_info_name(field.type_info),
                    ref_uid: None,
                })
                .collect(),
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

    fn push_varint(bytes: &mut Vec<u8>, mut value: u64) {
        while value >= 0x80 {
            bytes.push(((value & 0x7f) as u8) | 0x80);
            value >>= 7;
        }
        bytes.push(value as u8);
    }

    fn wide(value: &str) -> Vec<u8> {
        value.encode_utf16().flat_map(u16::to_le_bytes).collect()
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
