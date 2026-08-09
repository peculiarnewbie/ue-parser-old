export const UTRACE_SPAN = {
  load: "utrace.load",
  fileStream: "browser.file.stream",
  workerRoundTrip: "browser.worker.round_trip",
  jsonDecode: "browser.json.decode",
  dashboardStateCommit: "browser.dashboard.state_commit",
  dashboardPaint: "browser.dashboard.paint",
  timersTabPaint: "browser.timers_tab.paint",
  timelineLoad: "utrace.timeline.load",
  timelineStateCommit: "browser.timeline.state_commit",
  timelinePaint: "browser.timeline.paint",
  gpuTimelineLoad: "utrace.gpu_timeline.load",
  gpuTimelineStateCommit: "browser.gpu_timeline.state_commit",
  gpuTimelinePaint: "browser.gpu_timeline.paint",
  workerRequest: "worker.request",
  wasmModuleImport: "worker.wasm.module_import",
  wasmInitialize: "worker.wasm.initialize",
  wasmThreadPoolInitialize: "worker.wasm.thread_pool.initialize",
  wasmParse: "wasm.parse_and_serialize",
  sessionStart: "wasm.session.start",
  sessionPush: "wasm.session.push",
  sessionAnalyzing: "wasm.session.analyzing",
  sessionFinish: "wasm.session.finish_and_serialize",
  sessionFinishTransport: "wasm.session.finish.transport_finalize",
  sessionFinishInventory: "wasm.session.finish.inventory",
  sessionFinishEventRegistry: "wasm.session.finish.event_registry",
  sessionFinishImportantEvents: "wasm.session.finish.important_events",
  sessionFinishProviderImportantEvents:
    "wasm.session.finish.provider_important_events",
  sessionFinishNormalDispatch: "wasm.session.finish.normal_event_dispatch",
  sessionFinishCpuAggregation: "wasm.session.finish.cpu_aggregation",
  sessionFinishProviderFinalize: "wasm.session.finish.provider_finalize",
  sessionFinishCpuTimeline: "wasm.session.finish.cpu_timeline_finalize",
  sessionFinishGpuTimeline: "wasm.session.finish.gpu_timeline_finalize",
  sessionFinishOutput: "wasm.session.finish.output_construction",
  sessionFinishJson: "wasm.session.finish.json_serialize",
  sessionCancel: "wasm.session.cancel",
  timelineQuery: "wasm.timeline.query_and_serialize",
  gpuTimelineQuery: "wasm.gpu_timeline.query_and_serialize",
} as const;

export type UtraceSpanName = (typeof UTRACE_SPAN)[keyof typeof UTRACE_SPAN];

export type UtraceSpanDomain = "browser" | "worker" | "wasm" | "render";

export type UtraceSpanAttributeValue =
  | string
  | number
  | boolean
  | string[]
  | number[]
  | boolean[];

export type UtraceSpanAttributes = Readonly<
  Record<string, UtraceSpanAttributeValue | undefined>
>;

/** A completed Worker/WASM span that will be attached to the page's OpenTelemetry trace. */
export type RemoteUtraceSpan = {
  name: UtraceSpanName;
  domain: "worker" | "wasm";
  start_time_unix_ms: number;
  duration_ms: number;
  attributes?: UtraceSpanAttributes;
};

/** Serializable browser benchmark record derived from an OpenTelemetry span. */
export type UtraceSpanRecord = {
  trace_id: string;
  span_id: string;
  parent_span_id?: string;
  name: UtraceSpanName;
  domain: UtraceSpanDomain;
  start_time_unix_ms: number;
  duration_ms: number;
  status: "ok" | "error" | "unset";
  attributes: Record<string, UtraceSpanAttributeValue>;
};

export type UtraceBenchmarkSnapshot = {
  schema_version: 1;
  generated_at: string;
  spans: UtraceSpanRecord[];
};

export type ChromeTraceEvent = {
  name: UtraceSpanName;
  cat: UtraceSpanDomain;
  ph: "X";
  ts: number;
  dur: number;
  pid: number;
  tid: UtraceSpanDomain;
  args: Record<string, UtraceSpanAttributeValue>;
};
