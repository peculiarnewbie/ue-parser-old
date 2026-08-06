import { For, Show, createMemo, createSignal, onCleanup } from "solid-js";
import { DropZone } from "../components/DropZone";
import { FrameBrowser } from "../components/FrameBrowser";
import { ScopeTimeline, type TimelineLaneInterval } from "../components/ScopeTimeline";
import { FrameCostBrushChart, HorizontalBars, type FrameYMetric } from "../components/Charts";
import {
  AnnotationsPanel,
  CapturePanel,
  ConcurrencyPanel,
  CpuPanel,
  GpuPanel,
  IoPanel,
  MemoryPanel,
  MetricsPanel,
  OverviewPanel,
} from "../components/utrace/AnalysisPanels";
import {
  FramePercentileTable,
} from "../components/utrace/FramePercentileTable";
import { StatCard } from "../components/utrace/SortableTable";
import {
  analysisWindowFromFrameSelection,
  type AnalysisWindow,
} from "../lib/analysis-range";
import type { FrameSelection } from "../lib/frame-selection";
import { triageFrameMarkers } from "../lib/frame-marker-triage";
import {
  filterFramesByType,
  frameTypeLabel,
  type TraceFrameTypeFilter,
} from "../lib/frame-type";
import { createLatestValuePublisher } from "../lib/latest-value-publisher";
import { liveFrameFromPatch, mergeLiveFrameTiming } from "../lib/live-frame-timing";
import {
  buildFrameDisplayMap,
  formatCaptureElapsed,
  formatFrameLabel,
  type FrameLabelMode,
} from "../lib/frame-display";
import {
  ParseRequestError,
  formatParseTiming,
  type ParseTiming,
} from "../lib/api";
import {
  cancelWasmParsing,
  parseUtraceProgressWithWasm,
  queryUtraceGpuTimelineWithWasm,
  queryUtraceTimelineWithWasm,
} from "../lib/wasm-worker-client";
import {
  formatCompact,
  formatGpuCost,
  gpuCostUnit,
} from "../lib/format";
import type {
  CorrelatedFrameSummary,
  FrameTimingSummary,
  GpuTimelineDashboard,
  UtraceDashboard,
  UtraceInventory,
  UtraceTimelineQuery,
  UtraceProgressEvent,
} from "../lib/types";

type WorkbenchTab =
  | "overview"
  | "frames"
  | "cpu"
  | "gpu"
  | "metrics"
  | "memory"
  | "io"
  | "concurrency"
  | "annotations"
  | "capture";

// Rust/WASM uses a 32-bit usize. This is the browser's uncapped sentinel,
// not a retained-frame UI limit.
const BROWSER_ALL_FRAMES = 4_294_967_295;
const MAX_LIVE_CHART_FRAMES = Number.MAX_SAFE_INTEGER;
const LIVE_FRAME_RENDER_POINT_BUDGET = 600;

function progressTotal(event: UtraceProgressEvent | null, fallback: number): number {
  return event && event.type !== "failed" ? (event.progress.total_bytes ?? fallback) : fallback;
}

function progressPhase(event: UtraceProgressEvent | null): string {
  if (!event) return "Preparing capture";
  if (event.type === "failed") return "Parsing failed";
  if (event.progress.phase === "reading") return "Reading capture";
  if (event.progress.phase === "analyzing") return "Building analysis and CPU index";
  return "Analysis complete";
}

function progressPercent(event: UtraceProgressEvent | null, fallback: number): string {
  if (!event || event.type === "failed") return "Preparing capture";
  if (event.progress.phase === "analyzing") return "Finalizing analysis";
  const total = progressTotal(event, fallback);
  return total > 0
    ? `${Math.min(100, Math.round((event.progress.bytes_consumed / total) * 100))}%`
    : "Reading";
}

export default function UtracePage() {
  const [busy, setBusy] = createSignal(false);
  const [timelineBusy, setTimelineBusy] = createSignal(false);
  const [gpuBusy, setGpuBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [file, setFile] = createSignal<File | null>(null);
  const [fileName, setFileName] = createSignal<string | null>(null);
  const [dashboard, setDashboard] = createSignal<UtraceDashboard | null>(null);
  const [inventory, setInventory] = createSignal<UtraceInventory | null>(null);
  const [selectedFrame, setSelectedFrame] = createSignal<number | null>(null);
  const [selectedMarker, setSelectedMarker] = createSignal<FrameTimingSummary | null>(null);
  const [timelineDetail, setTimelineDetail] = createSignal<UtraceTimelineQuery | null>(
    null,
  );
  const [timelineSessionId, setTimelineSessionId] = createSignal<string>();
  const [gpuTimeline, setGpuTimeline] = createSignal<GpuTimelineDashboard | null>(null);
  const [rangeStart, setRangeStart] = createSignal("");
  const [rangeEnd, setRangeEnd] = createSignal("");
  const [threadFilter, setThreadFilter] = createSignal("");
  const [scopeSearch, setScopeSearch] = createSignal("");
  const [tab, setTab] = createSignal<WorkbenchTab>("overview");
  /** Chart-local navigation state. It never filters analysis or changes a query range. */
  const [chartNavigation, setChartNavigation] = createSignal<FrameSelection | null>(null);
  const [frameMetric, setFrameMetric] = createSignal<FrameYMetric>("frame_ms");
  const [frameTypeFilter, setFrameTypeFilter] =
    createSignal<TraceFrameTypeFilter>("game");
  const [frameLabelMode, setFrameLabelMode] =
    createSignal<FrameLabelMode>("relative");
  const [targetFps, setTargetFps] = createSignal(60);
  const [loadTiming, setLoadTiming] = createSignal<{
    dashboard?: ParseTiming;
    inventory?: ParseTiming;
    wall_ms: number;
  } | null>(null);
  const [timelineTiming, setTimelineTiming] = createSignal<ParseTiming | null>(
    null,
  );
  const [gpuTiming, setGpuTiming] = createSignal<ParseTiming | null>(null);
  const [decodeProgress, setDecodeProgress] = createSignal<UtraceProgressEvent | null>(null);
  const [liveFrameTiming, setLiveFrameTiming] = createSignal<FrameTimingSummary[]>([]);
  const [streamedBootstrap, setStreamedBootstrap] = createSignal<
    Extract<UtraceProgressEvent, { type: "bootstrap" }>["bootstrap"]
  >();
  let latestLiveFrameTiming: FrameTimingSummary[] = [];
  const liveFramePublisher = createLatestValuePublisher({
    publish: setLiveFrameTiming,
  });
  const updateLiveFrames = (event: UtraceProgressEvent) => {
    if (event.type !== "snapshot" || event.patch.type !== "frames") return;
    latestLiveFrameTiming = mergeLiveFrameTiming(
      latestLiveFrameTiming,
      event.patch.frames.map(liveFrameFromPatch),
      MAX_LIVE_CHART_FRAMES,
    );
    liveFramePublisher.publishLatest(latestLiveFrameTiming);
  };
  let loadAbort: AbortController | null = null;
  let timelineRequest = 0;
  let gpuRequest = 0;

  const cancelActiveLoad = ({ flushLiveFrames = false } = {}) => {
    if (flushLiveFrames) liveFramePublisher.flush();
    else liveFramePublisher.cancel();
    loadAbort?.abort();
  };

  const invalidateDetailRequests = () => {
    timelineRequest += 1;
    gpuRequest += 1;
    setTimelineBusy(false);
    setGpuBusy(false);
  };

  const resetLiveFrames = () => {
    liveFramePublisher.cancel();
    latestLiveFrameTiming = [];
    setLiveFrameTiming([]);
  };

  onCleanup(() => {
    cancelActiveLoad();
    invalidateDetailRequests();
    cancelWasmParsing();
  });

  const dash = createMemo(() => dashboard()?.dashboard ?? null);
  const frames = createMemo(() => dash()?.frame_correlation.frames ?? []);
  const chartFrames = createMemo((): FrameTimingSummary[] => {
    const completed = dash()?.frame_timing?.frames;
    const all = completed ?? liveFrameTiming();
    return filterFramesByType(all, frameTypeFilter());
  });
  const frameDisplays = createMemo(() =>
    buildFrameDisplayMap({
      frames: frames(),
      cycleFrequency: dash()?.prologue?.cycle_frequency,
    }),
  );
  const presentFrame = (frame: CorrelatedFrameSummary) => {
    const display = frameDisplays().get(frame.frame_number);
    return {
      label: `#${formatFrameLabel({
        frameNumber: frame.frame_number,
        display,
        mode: frameLabelMode(),
      })}`,
      elapsedSeconds: display?.elapsedSeconds,
      elapsedLabel: formatCaptureElapsed(display?.elapsedSeconds),
    };
  };
  const frameCaption = (frameNumber: number) => {
    const display = frameDisplays().get(frameNumber);
    const label = `#${formatFrameLabel({
      frameNumber,
      display,
      mode: frameLabelMode(),
    })}`;
    const time = formatCaptureElapsed(display?.elapsedSeconds);
    return time === "—" ? label : `${label} · ${time}`;
  };
  const markerCaption = (frame: FrameTimingSummary) => {
    const firstCycle = chartFrames()[0]?.begin_cycle;
    const frequency = dash()?.prologue?.cycle_frequency;
    const elapsed =
      firstCycle != null && frequency != null && frequency > 0
        ? (frame.begin_cycle - firstCycle) / frequency
        : undefined;
    const time = formatCaptureElapsed(elapsed);
    const label = `Marker #${frame.frame_number}`;
    return time === "—" ? label : `${label} · ${time}`;
  };
  const analysisWindow = createMemo((): AnalysisWindow =>
    analysisWindowFromFrameSelection(frames(), null),
  );
  const visibleFrames = createMemo(() => analysisWindow().frames);
  const frameBudgetMs = createMemo(() => 1000 / targetFps());
  const triage = createMemo(() =>
    triageFrameMarkers({
      frames: chartFrames(),
      cycleFrequency: dash()?.prologue?.cycle_frequency,
      budgetMs: frameBudgetMs(),
    }),
  );

  const selectedSummary = createMemo((): CorrelatedFrameSummary | null => {
    const frame = selectedFrame();
    if (frame == null) return null;
    return frames().find((row) => row.frame_number === frame) ?? null;
  });

  const threadNames = createMemo(() => {
    const map = new Map<number, string>();
    for (const thread of dash()?.cpu.threads ?? []) {
      map.set(thread.thread_id, thread.name ?? `thread ${thread.thread_id}`);
    }
    for (const info of dash()?.thread_info ?? []) {
      if (info.name && !map.has(info.thread_id)) {
        map.set(info.thread_id, info.name);
      }
    }
    return map;
  });

  const threadOptions = createMemo(() =>
    [...threadNames().entries()]
      .map(([id, name]) => ({ id, name }))
      .sort((a, b) => a.name.localeCompare(b.name)),
  );

  const cycleFrequency = () => dash()?.prologue?.cycle_frequency;
  const perFrameScopes = createMemo(
    () =>
      selectedSummary()?.top_cpu_scopes?.slice(0, 12).map((scope) => ({
        name: scope.name,
        value: scope.total_seconds ?? scope.total_cycles,
      })) ?? [],
  );

  const perFrameGpu = createMemo(() => {
    const freq = cycleFrequency();
    return (
      selectedSummary()?.top_gpu_breadcrumbs?.slice(0, 12).map((crumb) => ({
        name: crumb.name,
        value:
          freq != null && freq > 0
            ? (crumb.total_cycles / freq) * 1000
            : crumb.total_cycles,
      })) ?? []
    );
  });

  const cpuTimelineIntervals = createMemo((): TimelineLaneInterval[] => {
    const timeline = timelineDetail()?.timeline;
    if (!timeline) return [];
    const names = threadNames();
    return timeline.intervals.map((interval, index) => ({
      id: `cpu-${index}-${interval.thread_id}-${interval.start_cycle}`,
      lane: names.get(interval.thread_id) ?? `thread ${interval.thread_id}`,
      label: interval.rendered_name ?? interval.name,
      start: interval.start_cycle,
      end: interval.end_cycle,
      durationLabel:
        interval.duration_seconds != null
          ? `${(interval.duration_seconds * 1000).toFixed(3)} ms`
          : `${interval.duration} cy`,
    }));
  });

  const onChartNavigationChange = (selection: FrameSelection | null) => {
    setChartNavigation(selection);
  };

  const onChartNavigationClear = () => {
    setChartNavigation(null);
  };

  const tabDefs = createMemo(() => {
    const body = dash();
    if (!body) return [];
    const window = analysisWindow();
    return [
      { id: "overview" as const, label: "Overview", badge: null },
      {
        id: "frames" as const,
        label: "Timers",
        badge: String(window.frames.length),
      },
      {
        id: "cpu" as const,
        label: "CPU",
        badge: String(body.cpu.scopes.length),
      },
      {
        id: "gpu" as const,
        label: "GPU",
        badge: String(body.gpu.queues.length),
      },
      {
        id: "metrics" as const,
        label: "Metrics",
        badge: String(body.counters.counters.length + (body.stats.samples?.length ?? 0)),
      },
      {
        id: "memory" as const,
        label: "Memory",
        badge:
          body.memory.allocs.count > 0
            ? String(body.memory.allocs.count)
            : body.memory.scope_count > 0
              ? String(body.memory.scope_count)
              : null,
      },
      {
        id: "io" as const,
        label: "I/O",
        badge:
          body.platform_file.file_count +
            body.loading.package_count +
            body.io_store.backend_count >
          0
            ? String(
                body.platform_file.file_count +
                  body.loading.package_count +
                  body.io_store.backend_count,
              )
            : null,
      },
      {
        id: "concurrency" as const,
        label: "Tasks",
        badge:
          body.tasks.created + body.tasks.wait_count > 0
            ? String(body.tasks.wait_count || body.tasks.created)
            : null,
      },
      {
        id: "annotations" as const,
        label: "Marks",
        badge:
          body.annotations.bookmarks.events + body.logging.messages > 0
            ? String(
                body.annotations.bookmarks.events +
                  body.annotations.regions.completed,
              )
            : null,
      },
      {
        id: "capture" as const,
        label: "Capture",
        badge: String(body.channels.count || body.unmodeled.event_types),
      },
    ];
  });

  const onFile = async (next: File) => {
    setBusy(true);
    cancelActiveLoad();
    invalidateDetailRequests();
    cancelWasmParsing();
    const abortController = new AbortController();
    loadAbort = abortController;
    setError(null);
    setFile(next);
    setTimelineSessionId(undefined);
    setFileName(next.name);
    setSelectedFrame(null);
    setSelectedMarker(null);
    setTimelineDetail(null);
    setGpuTimeline(null);
    setRangeStart("");
    setRangeEnd("");
    setThreadFilter("");
    setScopeSearch("");
    setTab("overview");
    setChartNavigation(null);
    setFrameMetric("frame_ms");
    setFrameTypeFilter("game");
    setLoadTiming(null);
    setTimelineTiming(null);
    setGpuTiming(null);
    setDashboard(null);
    setInventory(null);
    setDecodeProgress({
      protocol_version: 1,
      type: "bootstrap",
      sequence: 0,
      progress: { phase: "reading", bytes_consumed: 0, total_bytes: next.size },
    });
    resetLiveFrames();
    setStreamedBootstrap(undefined);
    const wallStarted = performance.now();
    try {
      let latestSequence = -1;
      const dashResult = await parseUtraceProgressWithWasm(
        next,
        { max_frames: BROWSER_ALL_FRAMES },
        (event) => {
          if (loadAbort !== abortController) return;
          if (event.sequence <= latestSequence) return;
          latestSequence = event.sequence;
          setDecodeProgress(event);
          if (event.type === "bootstrap" && event.bootstrap) {
            setStreamedBootstrap(event.bootstrap);
          }
          updateLiveFrames(event);
          if (event.type === "complete" || event.type === "failed") {
            liveFramePublisher.flush();
          }
          if (event.type === "complete" && event.inventory) {
            setInventory(event.inventory);
          }
        },
        abortController.signal,
      );
      if (loadAbort !== abortController) return;
      setDashboard(dashResult.data);
      setTimelineSessionId(dashResult.sessionId);
      setLoadTiming({
        dashboard: dashResult.timing,
        wall_ms: Math.round(performance.now() - wallStarted),
      });
    } catch (err) {
      if (loadAbort !== abortController) return;
      setDashboard(null);
      setInventory(null);
      setLoadTiming(null);
      if (err instanceof ParseRequestError) {
        setError(err.body.stderr || err.message);
      } else {
        setError(err instanceof Error ? err.message : String(err));
      }
    } finally {
      if (loadAbort === abortController) {
        setBusy(false);
        loadAbort = null;
      }
    }
  };

  const loadCaptureTimeline = async (query: {
      start_cycle: number;
      end_cycle: number;
      thread?: number;
      search?: string;
    },
  ) => {
    const request = ++timelineRequest;
    setTimelineBusy(true);
    setError(null);
    try {
      const sessionId = timelineSessionId();
      if (!sessionId) throw new Error("timeline index is not ready");
      const detail = await queryUtraceTimelineWithWasm(sessionId, {
        ...query,
        limit: 2500,
      });
      if (request !== timelineRequest) return;
      setTimelineDetail(detail.data);
      setTimelineTiming(detail.timing);
      if (detail.sessionId) setTimelineSessionId(detail.sessionId);
    } catch (err) {
      if (request !== timelineRequest) return;
      setTimelineDetail(null);
      setTimelineTiming(null);
      if (err instanceof ParseRequestError) {
        setError(err.body.stderr || err.message);
      } else {
        setError(err instanceof Error ? err.message : String(err));
      }
    } finally {
      if (request === timelineRequest) setTimelineBusy(false);
    }
  };

  const loadGpuFrameTimeline = async (frameNumber: number) => {
    const request = ++gpuRequest;
    setGpuBusy(true);
    setError(null);
    try {
      const sessionId = timelineSessionId();
      if (!sessionId) throw new Error("GPU timeline index is not ready");
      const detail = await queryUtraceGpuTimelineWithWasm(sessionId, {
        frame_number: frameNumber,
        limit: 2500,
      });
      if (request !== gpuRequest) return;
      setGpuTimeline(detail.data.timeline);
      setGpuTiming(detail.timing);
      if (detail.data.timeline.interval_count === 0) {
        setError(`No GPU timeline retained for frame ${frameNumber}.`);
      }
    } catch (err) {
      if (request !== gpuRequest) return;
      setGpuTimeline(null);
      setGpuTiming(null);
      if (err instanceof ParseRequestError) {
        setError(err.body.stderr || err.message);
      } else {
        setError(err instanceof Error ? err.message : String(err));
      }
    } finally {
      if (request === gpuRequest) setGpuBusy(false);
    }
  };

  const loadFrameTimeline = async (
    frameNumber: number,
    sourceDashboard = dashboard(),
  ) => {
    const summary = sourceDashboard?.dashboard.frame_correlation.frames.find(
      (frame) => frame.frame_number === frameNumber,
    );
    if (summary?.cpu_begin_cycle == null || summary.cpu_end_cycle == null) {
      setSelectedFrame(frameNumber);
      setSelectedMarker(null);
      setTimelineDetail(null);
      setError("This frame has no CPU metadata scope bounds to query.");
      return;
    }
    setSelectedFrame(frameNumber);
    setSelectedMarker(null);
    setRangeStart(String(summary.cpu_begin_cycle));
    setRangeEnd(String(summary.cpu_end_cycle));
    await loadCaptureTimeline({
      start_cycle: summary.cpu_begin_cycle,
      end_cycle: summary.cpu_end_cycle,
      thread: parseOptionalUnsigned(threadFilter()),
      search: scopeSearch().trim() || undefined,
    });
  };

  const loadMarkerTimeline = async (marker: FrameTimingSummary) => {
    setSelectedFrame(null);
    setSelectedMarker(marker);
    setRangeStart(String(marker.begin_cycle));
    setRangeEnd(String(marker.end_cycle));
    setTab("frames");
    await loadCaptureTimeline({
      start_cycle: marker.begin_cycle,
      end_cycle: marker.end_cycle,
      thread: parseOptionalUnsigned(threadFilter()),
      search: scopeSearch().trim() || undefined,
    });
  };

  const onSelectFrame = async (frameNumber: number) => {
    setTab("frames");
    await loadFrameTimeline(frameNumber);
  };

  const onTimelineQuery = async (event: SubmitEvent) => {
    event.preventDefault();
    const startCycle = parseOptionalUnsigned(rangeStart());
    const endCycle = parseOptionalUnsigned(rangeEnd());
    const thread = parseOptionalUnsigned(threadFilter());
    if (startCycle == null || endCycle == null) {
      setError("Enter both CPU cycle bounds to query the capture-wide timeline.");
      return;
    }
    if (startCycle > endCycle) {
      setError("The range start must not exceed the range end.");
      return;
    }
    if (thread != null && thread > 65_535) {
      setError("Thread id must fit in an unsigned 16-bit value.");
      return;
    }
    setSelectedFrame(null);
    setSelectedMarker(null);
    setTab("frames");
    await loadCaptureTimeline({
      start_cycle: startCycle,
      end_cycle: endCycle,
      thread,
      search: scopeSearch().trim() || undefined,
    });
  };

  const jumpToCycles = async (start: number, end: number) => {
    setRangeStart(String(start));
    setRangeEnd(String(end));
    setSelectedFrame(null);
    setSelectedMarker(null);
    setTab("frames");
    await loadCaptureTimeline({
      start_cycle: start,
      end_cycle: end,
      thread: parseOptionalUnsigned(threadFilter()),
      search: scopeSearch().trim() || undefined,
    });
  };

  const queryThread = async (threadId: number) => {
    setThreadFilter(String(threadId));
    setTab("frames");
    const startCycle = parseOptionalUnsigned(rangeStart());
    const endCycle = parseOptionalUnsigned(rangeEnd());
    if (startCycle == null || endCycle == null) return;
    await loadCaptureTimeline({
      start_cycle: startCycle,
      end_cycle: endCycle,
      thread: threadId,
      search: scopeSearch().trim() || undefined,
    });
  };

  return (
    <section class="page page-wide">
      <header class="page-head analyzer-intro">
        <p class="eyebrow">Trace analysis</p>
        <h1>Inspect frame timing and trace events.</h1>
        <p class="lede">
          Open an Unreal trace to inspect frame timing, CPU scopes, GPU events,
          allocations, and tasks.
        </p>
      </header>

      <DropZone
        accept=".utrace"
        label="Drop a .utrace"
        hint="Frame timing appears while the browser reads the capture. CPU ranges then query the in-browser index without another upload."
        busy={busy() || timelineBusy() || gpuBusy()}
        onFile={onFile}
      />

      <Show when={error()}>
        <div class="banner error" role="alert">
          <strong>Parse failed</strong>
          <pre>{error()}</pre>
        </div>
      </Show>

      <Show when={(dash()?.frame_timing?.frames ?? liveFrameTiming()).length > 0}>
        <section class="panel">
          <div class="chart-frame-head" style={{ "margin-bottom": "0.75rem" }}>
            <div class="chart-frame-titles">
              <p class="eyebrow">
                {dashboard() ? "Frame-marker timing" : "Frame-marker timing · streaming"}
              </p>
            </div>
            <div class="chart-metric-controls">
              <label class="chart-metric-select">
                <span>Frame type</span>
                <select
                  value={frameTypeFilter()}
                  onChange={(event) =>
                    setFrameTypeFilter(event.currentTarget.value as TraceFrameTypeFilter)
                  }
                >
                  <option value="game">Game (Insights default)</option>
                  <option value="rendering">Rendering</option>
                  <option value="all">All types mixed</option>
                </select>
              </label>
            </div>
          </div>
          <FrameCostBrushChart
            frames={chartFrames()}
            cycleFrequency={dash()?.prologue?.cycle_frequency ?? streamedBootstrap()?.prologue?.cycle_frequency}
            frameLabel={(frame) =>
              frameTypeFilter() === "all"
                ? `${frameTypeLabel(frame.frame_type)} #${frame.frame_number}`
                : `#${frame.frame_number}`
            }
            selection={chartNavigation()}
            onSelectionChange={onChartNavigationChange}
            onSelectionCommit={onChartNavigationChange}
            onSelectionClear={onChartNavigationClear}
            metric={frameMetric()}
            onMetricChange={setFrameMetric}
            height={300}
            renderPointBudget={LIVE_FRAME_RENDER_POINT_BUDGET}
          />
        </section>
      </Show>

      <Show when={file() && !dashboard()}>
        <div class="panel-stack" aria-live="polite">
          <div class="capture-bar">
            <div class="capture-identity">
              <span class="capture-state status-partial" />
              <div>
                <strong>{fileName()}</strong>
                <span>{progressPhase(decodeProgress())}</span>
              </div>
            </div>
            <div class="capture-utilities">
              <span>{progressPercent(decodeProgress(), file()!.size)}</span>
              <button
                type="button"
                class="btn ghost compact"
                onClick={() => cancelActiveLoad({ flushLiveFrames: true })}
              >
                Cancel
              </button>
            </div>
          </div>
          <Show when={streamedBootstrap()} keyed>
            {(bootstrap) => (
              <section class="panel">
                <p class="eyebrow">Capture metadata streaming</p>
                <h2>{bootstrap.header.magic} · protocol {bootstrap.header.protocol}</h2>
                <div class="stat-grid">
                  <StatCard label="Packets decoded" value={String((bootstrap.packets.count as number | undefined) ?? 0)} />
                  <StatCard label="Threads discovered" value={`${bootstrap.thread_info.length}${bootstrap.thread_info_truncated ? "+" : ""}`} />
                  <StatCard label="Event types" value={String(bootstrap.declared_event_types)} />
                  <StatCard
                    label="Cycle frequency"
                    value={bootstrap.prologue?.cycle_frequency ? formatCompact(bootstrap.prologue.cycle_frequency) : "—"}
                  />
                </div>
                <Show when={bootstrap.thread_info.length > 0}>
                  <p class="muted">
                    {bootstrap.thread_info.slice(0, 6).map((thread) => thread.name).join(" · ")}
                  </p>
                </Show>
              </section>
            )}
          </Show>
        </div>
      </Show>

      <Show when={dashboard()} keyed>
        {(dashResult) => (
          <>
            <div class="capture-bar">
              <div class="capture-identity">
                <span class={`capture-state status-${dashResult.status}`} />
                <div>
                  <strong>{fileName()}</strong>
                  <span>
                    {dashResult.dashboard.session?.project_name ||
                      dashResult.dashboard.session?.app_name ||
                      "Unreal capture"}
                    {dashResult.dashboard.session?.configuration
                      ? ` · ${dashResult.dashboard.session.configuration}`
                      : ""}
                  </span>
                </div>
              </div>
              <div class="capture-controls">
                <label class="budget-control">
                  <span>Frame budget</span>
                  <select
                    value={targetFps()}
                    onChange={(event) => setTargetFps(Number(event.currentTarget.value))}
                  >
                    <option value={30}>33.33 ms · 30 FPS</option>
                    <option value={60}>16.67 ms · 60 FPS</option>
                    <option value={90}>11.11 ms · 90 FPS</option>
                    <option value={120}>8.33 ms · 120 FPS</option>
                  </select>
                </label>
                <div class="frame-label-control">
                  <span>Frame labels</span>
                  <div class="toggle-group" role="group" aria-label="Frame label mode">
                    <button
                      type="button"
                      class="toggle-btn"
                      classList={{ active: frameLabelMode() === "relative" }}
                      onClick={() => setFrameLabelMode("relative")}
                    >
                      0-based
                    </button>
                    <button
                      type="button"
                      class="toggle-btn"
                      classList={{ active: frameLabelMode() === "capture" }}
                      onClick={() => setFrameLabelMode("capture")}
                    >
                      Capture IDs
                    </button>
                  </div>
                </div>
              </div>
              <div class="capture-utilities">
                <Show when={timelineBusy() || gpuBusy()}>
                  <span class="capture-working">Analyzing selection…</span>
                </Show>
                <span title={loadTiming()?.dashboard ? formatParseTiming(loadTiming()!.dashboard!) : ""}>
                  {dashResult.dashboard.frame_timing?.total_frame_count ?? frames().length} frame timings
                </span>
              </div>
            </div>

            <Show when={loadTiming()}>
              {(timing) => (
                <section class="parse-performance" aria-label="Parse performance">
                  <div class="parse-performance-title">
                    <span>Parse performance</span>
                    <strong>streaming parser</strong>
                  </div>
                  <Show when={timing().dashboard}>
                    {(dashboardTiming) => (
                      <div class="parse-performance-metric">
                        <span>Dashboard</span>
                        <strong title={formatParseTiming(dashboardTiming())}>
                          {formatPerformanceSummary(dashboardTiming())}
                        </strong>
                      </div>
                    )}
                  </Show>
                  <Show when={timing().inventory}>
                    {(inventoryTiming) => (
                      <div class="parse-performance-metric">
                        <span>Inventory</span>
                        <strong title={formatParseTiming(inventoryTiming())}>
                          {formatPerformanceSummary(inventoryTiming())}
                        </strong>
                      </div>
                    )}
                  </Show>
                  <div class="parse-performance-metric total">
                    <span>Combined wall time</span>
                    <strong>{(timing().wall_ms / 1000).toFixed(2)}s</strong>
                  </div>
                </section>
              )}
            </Show>

            <div class="status-row analyzer-filters">
              <span class={`pill status-${dashResult.status}`}>{dashResult.status}</span>
              <span class="mono">{fileName()}</span>
              <span class="muted">
                {dashResult.dashboard.header.magic} · protocol{" "}
                {dashResult.dashboard.header.protocol}
              </span>
              <Show when={dashResult.dashboard.session}>
                {(session) => (
                  <span class="muted">
                    {session().project_name || session().app_name} ·{" "}
                    {session().configuration}
                  </span>
                )}
              </Show>
            <Show when={timelineBusy()}>
              <span class="pill status-partial">loading CPU timeline…</span>
            </Show>
            <Show when={gpuBusy()}>
              <span class="pill status-partial">loading GPU timeline…</span>
            </Show>
            <Show when={loadTiming()}>
              {(timing) => (
                <span
                  class="pill timing-pill"
                  title={[
                    timing().dashboard
                      ? `dashboard: ${formatParseTiming(timing().dashboard!)}`
                      : null,
                    timing().inventory
                      ? `inventory: ${formatParseTiming(timing().inventory!)}`
                      : null,
                  ]
                    .filter(Boolean)
                    .join("\n")}
                >
                  load {(timing().wall_ms / 1000).toFixed(2)}s
                </span>
              )}
            </Show>
            <Show when={timelineTiming()}>
              {(timing) => (
                <span class="pill timing-pill" title={formatParseTiming(timing())}>
                  timeline {formatParseTiming(timing())}
                </span>
              )}
            </Show>
            <Show when={gpuTiming()}>
              {(timing) => (
                <span class="pill timing-pill" title={formatParseTiming(timing())}>
                  gpu {formatParseTiming(timing())}
                </span>
              )}
            </Show>
          </div>

          <div class="analyzer-layout">
          <nav class="workbench-tabs" aria-label="Analysis sections">
              <For each={tabDefs()}>
                {(item) => (
                  <button
                    type="button"
                    class="workbench-tab"
                    classList={{ active: tab() === item.id }}
                    onClick={() => setTab(item.id)}
                  >
                    <span>{item.label}</span>
                    <Show when={item.badge}>
                      <span class="tab-badge">{item.badge}</span>
                    </Show>
                  </button>
                )}
              </For>
            </nav>

            <main class="analyzer-workspace">
            <Show when={tab() === "overview" && dash()}>
              <div class="panel-stack">
                <FramePercentileTable
                  frames={visibleFrames()}
                  cycleFrequency={dashResult.dashboard.prologue?.cycle_frequency}
                />
                <OverviewPanel dash={() => dash()!} window={analysisWindow} inventory={inventory} onOpenFrames={() => setTab("frames")} />
              </div>
            </Show>

            <Show when={tab() === "frames"}>
              <div class="panel-stack">
                <section class="triage-strip timer-triage" aria-label="Performance triage summary">
                  <div class="triage-primary">
                    <span>Markers over {frameBudgetMs().toFixed(2)} ms</span>
                    <strong>{triage().overBudget.length}</strong>
                    <em>{triage().overBudgetPercent.toFixed(1)}% of capture</em>
                  </div>
                  <button
                    type="button"
                    class="triage-finding"
                    disabled={!triage().worst}
                    onClick={() => {
                      const worst = triage().worst;
                      if (worst) void loadMarkerTimeline(worst.frame);
                    }}
                  >
                    <span>Worst marker</span>
                    <strong>
                      {triage().worst
                        ? markerCaption(triage().worst!.frame)
                        : "—"}
                    </strong>
                    <em>
                      {triage().worst
                        ? `${triage().worst!.frameMs.toFixed(2)} ms marker duration · open CPU window →`
                        : "No timing data"}
                    </em>
                  </button>
                  <div class="triage-split">
                    <span>Chart source</span>
                    <strong>Frame markers</strong>
                    <em>same IDs and durations as the chart</em>
                  </div>
                </section>

                <Show when={timelineDetail()?.timeline}>
                  {(timeline) => (
                    <ScopeTimeline
                      title={
                        selectedMarker() != null
                          ? `CPU timers · ${markerCaption(selectedMarker()!)}`
                          : selectedFrame() != null
                          ? `CPU timers · ${frameCaption(selectedFrame()!)}`
                          : "CPU timers · capture range"
                      }
                      subtitle={
                        timeline().duration_seconds != null
                          ? `${(timeline().duration_seconds! * 1000).toFixed(2)} ms window · nested spans from the browser index`
                          : `${timeline().interval_count} matching spans from the browser index`
                      }
                      begin={timeline().begin_cycle}
                      end={timeline().end_cycle}
                      cycleFrequency={dashResult.dashboard.prologue?.cycle_frequency}
                      truncated={timeline().truncated}
                      intervals={cpuTimelineIntervals()}
                      empty="No CPU intervals match this capture range and filter."
                    />
                  )}
                </Show>

                <div class="stat-grid">
                  <StatCard
                    label="Selected timer window"
                    value={
                      selectedMarker() != null
                        ? markerCaption(selectedMarker()!)
                        : selectedFrame() != null
                          ? frameCaption(selectedFrame()!)
                          : "—"
                    }
                  />
                  <StatCard
                    label="Frames listed"
                    value={String(visibleFrames().length)}
                    hint="full correlated set"
                  />
                  <StatCard
                    label="CPU intervals"
                    value={String(timelineDetail()?.timeline.interval_count ?? "—")}
                  />
                  <StatCard
                    label="Window"
                    value={
                      timelineDetail()?.timeline.duration_seconds != null
                        ? `${(timelineDetail()!.timeline.duration_seconds! * 1000).toFixed(2)} ms`
                        : "—"
                    }
                  />
                </div>

                <FrameBrowser
                  frames={visibleFrames()}
                  selectedFrame={selectedFrame()}
                  onSelect={onSelectFrame}
                  presentFrame={presentFrame}
                />

                <form class="panel timeline-query" onSubmit={onTimelineQuery}>
                  <div>
                    <p class="eyebrow">Capture-wide query</p>
                    <h2>Navigate CPU scopes by cycle range</h2>
                    <p class="muted datatable-meta">
                      Frame clicks fill this range. Narrow the window, pick a thread,
                      or search scope names — the browser index answers without reparsing.
                    </p>
                  </div>
                  <div class="timeline-query-fields">
                    <label>
                      <span>Start cycle</span>
                      <input
                        inputmode="numeric"
                        value={rangeStart()}
                        onInput={(event) => setRangeStart(event.currentTarget.value)}
                        placeholder="e.g. 1280000"
                      />
                    </label>
                    <label>
                      <span>End cycle</span>
                      <input
                        inputmode="numeric"
                        value={rangeEnd()}
                        onInput={(event) => setRangeEnd(event.currentTarget.value)}
                        placeholder="e.g. 1296000"
                      />
                    </label>
                    <label>
                      <span>Thread</span>
                      <select
                        value={threadFilter()}
                        onChange={(event) => setThreadFilter(event.currentTarget.value)}
                      >
                        <option value="">all threads</option>
                        <For each={threadOptions()}>
                          {(thread) => (
                            <option value={String(thread.id)}>
                              {thread.name} ({thread.id})
                            </option>
                          )}
                        </For>
                      </select>
                    </label>
                    <label class="scope-search">
                      <span>Scope search</span>
                      <input
                        value={scopeSearch()}
                        onInput={(event) => setScopeSearch(event.currentTarget.value)}
                        placeholder="render, tick, physics…"
                      />
                    </label>
                    <button
                      class="btn compact"
                      type="submit"
                      disabled={timelineBusy()}
                    >
                      Query range
                    </button>
                  </div>
                </form>

                <Show when={selectedSummary()}>
                  {(summary) => (
                    <div class="chart-grid">
                      <HorizontalBars
                        title={`Frame ${frameCaption(summary().frame_number)} · top CPU scopes`}
                        subtitle="From frame correlation (available before timeline reload)"
                        data={perFrameScopes()}
                        valueLabel="cost"
                      />
                      <Show
                        when={perFrameGpu().length > 0}
                        fallback={
                          <section class="panel frame-summary">
                            <p class="eyebrow">Frame summary</p>
                            <h2>Frame {frameCaption(summary().frame_number)}</h2>
                            <dl class="kv">
                              <div>
                                <dt>CPU metadata</dt>
                                <dd>
                                  {summary().cpu_metadata_seconds?.toFixed(4) ?? "—"} s ·{" "}
                                  {summary().cpu_metadata_count} scopes
                                </dd>
                              </div>
                              <div>
                                <dt>GPU work</dt>
                                <dd>
                                  {formatGpuCost(
                                    summary().gpu_work_cycles,
                                    cycleFrequency(),
                                  )}{" "}
                                  · {summary().gpu_work_count} events
                                </dd>
                              </div>
                              <div>
                                <dt>GPU breadcrumbs</dt>
                                <dd>
                                  {formatGpuCost(
                                    summary().gpu_breadcrumb_cycles,
                                    cycleFrequency(),
                                  )}{" "}
                                  · {summary().gpu_breadcrumb_count} events
                                </dd>
                              </div>
                              <div>
                                <dt>Actions</dt>
                                <dd>
                                  <button
                                    type="button"
                                    class="btn ghost compact"
                                    disabled={gpuBusy()}
                                    onClick={() => {
                                      void loadGpuFrameTimeline(summary().frame_number);
                                    }}
                                  >
                                    Load GPU timeline
                                  </button>
                                </dd>
                              </div>
                            </dl>
                          </section>
                        }
                      >
                        <HorizontalBars
                          title={`Frame ${frameCaption(summary().frame_number)} · top GPU breadcrumbs`}
                          data={perFrameGpu()}
                          valueLabel={gpuCostUnit(cycleFrequency())}
                        />
                      </Show>
                    </div>
                  )}
                </Show>

              </div>
            </Show>

            <Show when={tab() === "cpu" && dash()}>
              <CpuPanel
                dash={() => dash()!}
                window={analysisWindow}
                onQueryThread={queryThread}
              />
            </Show>

            <Show when={tab() === "gpu" && dash()}>
              <GpuPanel
                dash={() => dash()!}
                window={analysisWindow}
                gpuTimeline={gpuTimeline}
                gpuBusy={gpuBusy}
                onLoadGpuFrame={(frameNumber) => {
                  void loadGpuFrameTimeline(frameNumber);
                }}
              />
            </Show>

            <Show when={tab() === "metrics" && dash()}>
              <MetricsPanel dash={() => dash()!} window={analysisWindow} />
            </Show>

            <Show when={tab() === "memory" && dash()}>
              <MemoryPanel dash={() => dash()!} window={analysisWindow} />
            </Show>

            <Show when={tab() === "io" && dash()}>
              <IoPanel dash={() => dash()!} window={analysisWindow} />
            </Show>

            <Show when={tab() === "concurrency" && dash()}>
              <ConcurrencyPanel
                dash={() => dash()!}
                window={analysisWindow}
                onJumpCycles={jumpToCycles}
              />
            </Show>

            <Show when={tab() === "annotations" && dash()}>
              <AnnotationsPanel
                dash={() => dash()!}
                window={analysisWindow}
                onJumpCycles={jumpToCycles}
              />
            </Show>

            <Show when={tab() === "capture" && dash()}>
              <CapturePanel dash={() => dash()!} inventory={inventory} />
            </Show>
            </main>
          </div>
          </>
        )}
      </Show>
    </section>
  );
}

function formatPerformanceSummary(timing: ParseTiming): string {
  const seconds = (milliseconds: number) => `${(milliseconds / 1000).toFixed(2)}s`;
  const parser = timing.parse_ms;
  return parser == null
    ? `browser ${seconds(timing.client_ms)}`
    : `browser ${seconds(timing.client_ms)} · parse ${seconds(parser)}`;
}

function parseOptionalUnsigned(value: string): number | undefined {
  const trimmed = value.trim();
  if (trimmed === "") return undefined;
  const parsed = Number(trimmed);
  return Number.isSafeInteger(parsed) && parsed >= 0 ? parsed : undefined;
}
