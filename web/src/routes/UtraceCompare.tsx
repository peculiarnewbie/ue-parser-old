import { Match, Show, Switch, createMemo, createSignal, onCleanup, onMount } from "solid-js";
import { ScopeTimeline, type TimelineLaneInterval } from "../components/ScopeTimeline";
import { ComparisonCaptureSlot } from "../components/utrace/compare/CaptureSlot";
import { ComparisonOverview } from "../components/utrace/compare/ComparisonOverview";
import { CpuComparisonTable } from "../components/utrace/compare/CpuComparisonTable";
import { PairedRangeControls } from "../components/utrace/compare/PairedRangeControls";
import {
  ParseRequestError,
  RETAIN_ALL_UTRACE_FRAMES,
} from "../lib/api";
import { formatNumber } from "../lib/format";
import type {
  ComparisonCaptureState,
  ComparisonRangeMs,
  ComparisonSide,
  LoadedComparisonCapture,
  TimerComparisonRow,
} from "../lib/utrace-comparison/model";
import { captureDurationMs, cycleRange } from "../lib/utrace-comparison/ranges";
import { compareCpuTimerRanges, compareCpuTimers } from "../lib/utrace-comparison/timers";
import type {
  UtraceProgressEvent,
  UtraceTimelineQuery,
  UtraceTimerStatsQuery,
} from "../lib/types";
import {
  parseUtraceProgressWithWasm,
  prepareUtraceWasm,
  queryUtraceTimerStatsWithWasm,
  queryUtraceTimelineWithWasm,
  releaseUtraceSessionWithWasm,
} from "../lib/wasm-worker-client";

type CompareTab = "outcome" | "cpu" | "evidence";

type TimerEvidenceState =
  | { status: "idle" }
  | { status: "loading"; timer: TimerComparisonRow }
  | { status: "ready"; timer: TimerComparisonRow; baseline: UtraceTimelineQuery; candidate: UtraceTimelineQuery }
  | { status: "error"; timer: TimerComparisonRow; message: string };

type RangeStatsState =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "ready"; baseline: UtraceTimerStatsQuery; candidate: UtraceTimerStatsQuery }
  | { status: "error"; message: string };

const EMPTY_CAPTURE: ComparisonCaptureState = { status: "empty" };

function readyCapture(state: ComparisonCaptureState): LoadedComparisonCapture | null {
  return state.status === "ready" ? state.capture : null;
}

function timelineIntervals(
  query: UtraceTimelineQuery,
  capture: LoadedComparisonCapture,
  side: ComparisonSide,
): TimelineLaneInterval[] {
  const names = new Map<number, string>();
  for (const thread of capture.dashboard.dashboard.cpu.threads) {
    names.set(thread.thread_id, thread.name ?? `thread ${thread.thread_id}`);
  }
  for (const thread of capture.dashboard.dashboard.thread_info ?? []) {
    if (thread.name && !names.has(thread.thread_id)) names.set(thread.thread_id, thread.name);
  }
  return query.timeline.intervals.map((interval, index) => ({
    id: `${side}-${index}-${interval.thread_id}-${interval.start_cycle}`,
    lane: names.get(interval.thread_id) ?? `thread ${interval.thread_id}`,
    label: interval.rendered_name ?? interval.name,
    sourceLabel: interval.name,
    start: interval.start_cycle,
    end: interval.end_cycle,
    threadId: interval.thread_id,
    specId: interval.spec_id,
    metadataId: interval.metadata_id,
    durationLabel: interval.duration_seconds != null
      ? `${formatNumber(interval.duration_seconds * 1_000, 3)} ms`
      : `${formatNumber(interval.duration, 0)} cy`,
  }));
}

function comparisonError(error: unknown): string {
  if (error instanceof ParseRequestError) {
    return error.body.stderr || error.body.error || error.message;
  }
  return error instanceof Error ? error.message : String(error);
}

export default function UtraceComparePage() {
  onMount(() => { void prepareUtraceWasm(); });
  const [baselineState, setBaselineState] = createSignal<ComparisonCaptureState>(EMPTY_CAPTURE);
  const [candidateState, setCandidateState] = createSignal<ComparisonCaptureState>(EMPTY_CAPTURE);
  const [activeTab, setActiveTab] = createSignal<CompareTab>("outcome");
  const [frameType, setFrameType] = createSignal(0);
  const [budgetMs, setBudgetMs] = createSignal(16.67);
  const [timerEvidence, setTimerEvidence] = createSignal<TimerEvidenceState>({ status: "idle" });
  const [timerMode, setTimerMode] = createSignal<"capture" | "ranges">("capture");
  const [baselineRange, setBaselineRange] = createSignal<ComparisonRangeMs>({ startMs: 0, endMs: 0 });
  const [candidateRange, setCandidateRange] = createSignal<ComparisonRangeMs>({ startMs: 0, endMs: 0 });
  const [rangeStats, setRangeStats] = createSignal<RangeStatsState>({ status: "idle" });
  let activeLoad: { side: ComparisonSide; controller: AbortController; generation: number } | null = null;
  let generation = 0;
  let rangeGeneration = 0;
  let evidenceGeneration = 0;
  const invalidateEvidence = () => {
    evidenceGeneration += 1;
    setTimerEvidence({ status: "idle" });
  };
  const invalidateQueries = () => {
    rangeGeneration += 1;
    setRangeStats({ status: "idle" });
    invalidateEvidence();
  };

  const stateFor = (side: ComparisonSide) => side === "baseline" ? baselineState() : candidateState();
  const setStateFor = (side: ComparisonSide, state: ComparisonCaptureState) => {
    if (side === "baseline") setBaselineState(state);
    else setCandidateState(state);
  };
  const baseline = createMemo(() => readyCapture(baselineState()));
  const candidate = createMemo(() => readyCapture(candidateState()));
  const parsing = createMemo(() => baselineState().status === "loading" || candidateState().status === "loading");
  const captureTimers = createMemo(() => {
    const left = baseline();
    const right = candidate();
    return left && right ? compareCpuTimers(left.dashboard, right.dashboard) : [];
  });
  const timers = createMemo(() => {
    const stats = rangeStats();
    const left = baseline();
    const right = candidate();
    if (timerMode() !== "ranges" || stats.status !== "ready" || !left || !right) {
      return captureTimers();
    }
    return compareCpuTimerRanges({
      baselineDashboard: left.dashboard,
      candidateDashboard: right.dashboard,
      baselineStats: stats.baseline,
      candidateStats: stats.candidate,
    });
  });
  const loadingEvidence = createMemo(() => {
    const state = timerEvidence();
    return state.status === "loading" ? state : undefined;
  });
  const failedEvidence = createMemo(() => {
    const state = timerEvidence();
    return state.status === "error" ? state : undefined;
  });
  const readyEvidence = createMemo(() => {
    const state = timerEvidence();
    return state.status === "ready" ? state : undefined;
  });
  const rangeError = createMemo(() => {
    const state = rangeStats();
    return state.status === "error" ? state.message : undefined;
  });
  const readyRangeStats = createMemo(() => {
    const state = rangeStats();
    return state.status === "ready" ? state : undefined;
  });

  const releaseCapture = (capture: LoadedComparisonCapture | null) => {
    if (!capture) return;
    void releaseUtraceSessionWithWasm(capture.sessionId).catch(() => undefined);
  };

  const clearSide = (side: ComparisonSide) => {
    const current = stateFor(side);
    if (activeLoad?.side === side) {
      activeLoad.controller.abort();
      activeLoad = null;
    }
    if (current.status === "ready") releaseCapture(current.capture);
    setStateFor(side, EMPTY_CAPTURE);
    invalidateQueries();
  };

  const loadCapture = async (side: ComparisonSide, file: File) => {
    if (activeLoad) return;
    const previous = readyCapture(stateFor(side));
    releaseCapture(previous);
    const controller = new AbortController();
    const loadGeneration = ++generation;
    activeLoad = { side, controller, generation: loadGeneration };
    setStateFor(side, {
      status: "loading",
      file,
      progress: { phase: "reading", bytesConsumed: 0, totalBytes: file.size },
    });
    invalidateQueries();
    let inventory: LoadedComparisonCapture["inventory"];
    let timelineIndex: LoadedComparisonCapture["timelineIndex"];
    try {
      const result = await parseUtraceProgressWithWasm({
        file,
        options: { max_frames: RETAIN_ALL_UTRACE_FRAMES },
        signal: controller.signal,
        onEvent: (event: UtraceProgressEvent) => {
          if (activeLoad?.generation !== loadGeneration || event.type === "failed") return;
          if (event.type === "complete") {
            inventory = event.inventory;
            timelineIndex = event.timeline_index;
            return;
          }
          setStateFor(side, {
            status: "loading",
            file,
            progress: {
              phase: event.progress.phase === "analyzing" ? "analyzing" : "reading",
              bytesConsumed: event.progress.bytes_consumed,
              totalBytes: event.progress.total_bytes ?? file.size,
            },
          });
        },
      });
      if (activeLoad?.generation !== loadGeneration) {
        releaseCapture({ file, dashboard: result.data, sessionId: result.sessionId, timing: result.timing });
        return;
      }
      const capture: LoadedComparisonCapture = {
        file,
        dashboard: result.data,
        inventory,
        timelineIndex,
        sessionId: result.sessionId,
        timing: result.timing,
      };
      setStateFor(side, { status: "ready", capture });
      const fullRange = { startMs: 0, endMs: captureDurationMs(capture) };
      if (side === "baseline") setBaselineRange(fullRange);
      else setCandidateRange(fullRange);
      setRangeStats({ status: "idle" });
      if (baseline() && candidate()) setActiveTab("outcome");
    } catch (error) {
      if (controller.signal.aborted) return;
      setStateFor(side, { status: "error", file, message: comparisonError(error) });
    } finally {
      if (activeLoad?.generation === loadGeneration) activeLoad = null;
    }
  };

  const swapCaptures = () => {
    if (parsing()) return;
    const left = baselineState();
    setBaselineState(candidateState());
    setCandidateState(left);
    const leftRange = baselineRange();
    setBaselineRange(candidateRange());
    setCandidateRange(leftRange);
    invalidateQueries();
  };

  const analyzeRanges = async () => {
    const left = baseline();
    const right = candidate();
    if (!left || !right) return;
    const requestGeneration = ++rangeGeneration;
    setRangeStats({ status: "loading" });
    invalidateEvidence();
    try {
      const leftRange = cycleRange(left, baselineRange());
      const rightRange = cycleRange(right, candidateRange());
      const [baselineResult, candidateResult] = await Promise.all([
        queryUtraceTimerStatsWithWasm({
          sessionId: left.sessionId,
          options: { ...leftRange, limit: 10_000 },
        }),
        queryUtraceTimerStatsWithWasm({
          sessionId: right.sessionId,
          options: { ...rightRange, limit: 10_000 },
        }),
      ]);
      if (requestGeneration !== rangeGeneration) return;
      setRangeStats({
        status: "ready",
        baseline: baselineResult.data,
        candidate: candidateResult.data,
      });
    } catch (error) {
      if (requestGeneration === rangeGeneration) {
        setRangeStats({ status: "error", message: comparisonError(error) });
      }
    }
  };

  const inspectTimer = async (timer: TimerComparisonRow) => {
    const left = baseline();
    const right = candidate();
    if (!left || !right) return;
    const requestGeneration = ++evidenceGeneration;
    setActiveTab("evidence");
    setTimerEvidence({ status: "loading", timer });
    try {
      const query = (capture: LoadedComparisonCapture, range: ComparisonRangeMs) => queryUtraceTimelineWithWasm({
        sessionId: capture.sessionId,
        options: {
          ...(timerMode() === "ranges" && rangeStats().status === "ready" ? cycleRange(capture, range) : {
            start_cycle: capture.timelineIndex?.begin_cycle,
            end_cycle: capture.timelineIndex?.end_cycle,
          }),
          search: timer.name,
          limit: 2_500,
        },
      });
      const [baselineResult, candidateResult] = await Promise.all([
        query(left, baselineRange()),
        query(right, candidateRange()),
      ]);
      if (requestGeneration !== evidenceGeneration) return;
      setTimerEvidence({
        status: "ready",
        timer,
        baseline: baselineResult.data,
        candidate: candidateResult.data,
      });
    } catch (error) {
      if (requestGeneration === evidenceGeneration) {
        setTimerEvidence({ status: "error", timer, message: comparisonError(error) });
      }
    }
  };

  onCleanup(() => {
    activeLoad?.controller.abort();
    activeLoad = null;
    invalidateQueries();
    releaseCapture(baseline());
    releaseCapture(candidate());
  });

  return (
    <section class="page page-wide compare-page">
      <header class="compare-hero">
        <div>
          <p class="eyebrow">Local regression laboratory</p>
          <h1>Compare the outcome.<br /><span>Then interrogate the cause.</span></h1>
        </div>
        <p>Two independent Unreal captures. Exact frame distributions and capture-wide CPU attribution first; timelines only when you ask for evidence.</p>
      </header>

      <div class="compare-capture-grid">
        <ComparisonCaptureSlot
          side="baseline"
          state={baselineState()}
          disabled={parsing() && baselineState().status !== "loading"}
          onFile={(file) => loadCapture("baseline", file)}
          onClear={() => clearSide("baseline")}
        />
        <button
          type="button"
          class="compare-swap"
          disabled={parsing() || (!baseline() && !candidate())}
          onClick={swapCaptures}
          aria-label="Swap baseline and candidate"
        >
          ⇄
        </button>
        <ComparisonCaptureSlot
          side="candidate"
          state={candidateState()}
          disabled={parsing() && candidateState().status !== "loading"}
          onFile={(file) => loadCapture("candidate", file)}
          onClear={() => clearSide("candidate")}
        />
      </div>

      <Show
        when={baseline() && candidate()}
        fallback={
          <section class="compare-empty-state">
            <span>01</span><p>Load a known-good baseline.</p>
            <span>02</span><p>Load the candidate under test.</p>
            <span>03</span><p>Start with distributions—not synchronized clocks.</p>
          </section>
        }
      >
        <div class="compare-command-bar">
          <nav aria-label="Comparison views">
            {(["outcome", "cpu", "evidence"] as const).map((tab) => (
              <button
                type="button"
                classList={{ active: activeTab() === tab }}
                onClick={() => setActiveTab(tab)}
              >
                {tab === "outcome" ? "Outcome" : tab === "cpu" ? "CPU attribution" : "Timeline evidence"}
                <Show when={tab === "cpu"}><span>{timers().length.toLocaleString()}</span></Show>
              </button>
            ))}
          </nav>
          <div class="compare-controls">
            <div class="toggle-group" role="group" aria-label="Frame type">
              <button type="button" class="toggle-btn" classList={{ active: frameType() === 0 }} onClick={() => setFrameType(0)}>Game</button>
              <button type="button" class="toggle-btn" classList={{ active: frameType() === 1 }} onClick={() => setFrameType(1)}>Rendering</button>
            </div>
            <label>
              Frame budget
              <span><input type="number" min="0.01" step="0.01" value={budgetMs()} onInput={(event) => setBudgetMs(Math.max(0.01, event.currentTarget.valueAsNumber || 16.67))} /> ms</span>
            </label>
          </div>
        </div>

        <Switch>
          <Match when={activeTab() === "outcome"}>
            <ComparisonOverview baseline={baseline()!} candidate={candidate()!} frameType={frameType()} budgetMs={budgetMs()} />
          </Match>
          <Match when={activeTab() === "cpu"}>
            <div class="compare-attribution-intro">
              <div><p class="eyebrow">Where the time moved</p><h2>Rank absolute impact before percentages.</h2></div>
              <p>These are exhaustive capture-wide inclusive CPU totals, normalized by each run’s completed Game frames. Select a row to inspect retained intervals on local timelines.</p>
            </div>
            <div class="compare-attribution-mode">
              <div class="toggle-group" role="group" aria-label="CPU comparison range">
                <button type="button" class="toggle-btn" classList={{ active: timerMode() === "capture" }} onClick={() => { setTimerMode("capture"); invalidateEvidence(); }}>Whole captures</button>
                <button type="button" class="toggle-btn" classList={{ active: timerMode() === "ranges" }} onClick={() => { setTimerMode("ranges"); invalidateEvidence(); }}>Explicit ranges</button>
              </div>
              <span>{timerMode() === "capture" ? "Exact capture-wide timer set" : "Exact interval aggregation · independent cycle domains"}</span>
            </div>
            <Show when={timerMode() === "ranges"}>
              <PairedRangeControls
                baseline={baseline()!}
                candidate={candidate()!}
                baselineRange={baselineRange()}
                candidateRange={candidateRange()}
                baselineDurationMs={captureDurationMs(baseline()!)}
                candidateDurationMs={captureDurationMs(candidate()!)}
                busy={rangeStats().status === "loading"}
                error={rangeError()}
                onBaselineRange={(range) => { setBaselineRange(range); invalidateQueries(); }}
                onCandidateRange={(range) => { setCandidateRange(range); invalidateQueries(); }}
                onAnalyze={() => void analyzeRanges()}
              />
              <Show when={readyRangeStats()?.baseline.timer_stats.truncated || readyRangeStats()?.candidate.timer_stats.truncated}>
                <div class="compare-local-axis-warning"><strong>Bounded identity response.</strong> At least one index or response is incomplete. Grouped timer totals may be lower bounds; change classification is disabled.</div>
              </Show>
            </Show>
            <Show
              when={timerMode() === "capture" || rangeStats().status === "ready"}
              fallback={<section class="compare-range-prompt"><strong>Choose both local ranges, then aggregate.</strong><p>No frame, time, or cycle pairing is implied.</p></section>}
            >
              <CpuComparisonTable rows={timers()} onSelect={(timer) => void inspectTimer(timer)} />
            </Show>
          </Match>
          <Match when={activeTab() === "evidence"}>
            <Switch>
              <Match when={timerEvidence().status === "idle"}>
                <section class="compare-timeline-empty">
                  <p class="eyebrow">Evidence, not alignment theater</p>
                  <h2>Select a CPU attribution row.</h2>
                  <p>The analyzer will query both retained sessions for that timer and show local-axis small multiples. Their horizontal positions are not synchronized.</p>
                  <button class="btn ghost" type="button" onClick={() => setActiveTab("cpu")}>Choose a timer</button>
                </section>
              </Match>
              <Match when={loadingEvidence()}>
                {(state) => <section class="compare-timeline-empty"><span class="compare-orbit" /><h2>Querying {state().timer.name}</h2><p>Scanning both retained CPU indexes.</p></section>}
              </Match>
              <Match when={failedEvidence()}>
                {(state) => <section class="banner error"><strong>Timeline query failed</strong><p>{state().message}</p></section>}
              </Match>
              <Match when={readyEvidence()}>
                {(state) => {
                  const left = () => state().baseline.timeline;
                  const right = () => state().candidate.timeline;
                  return (
                    <div class="compare-timeline-evidence">
                      <header>
                        <div><p class="eyebrow">Timer evidence</p><h2>{state().timer.name}</h2></div>
                        <p>Independent local axes · earliest retained matches · substring search may include other timers and rendered-name variants</p>
                      </header>
                      <div class="compare-local-axis-warning"><strong>No temporal pairing.</strong> Zoom and selection stay local to each capture.</div>
                      <div class="compare-timeline-grid">
                        <ScopeTimeline
                          title="A · Baseline"
                          subtitle={`${left().interval_count.toLocaleString()} matching intervals`}
                          begin={left().begin_cycle}
                          end={left().end_cycle}
                          cycleFrequency={left().index.cycle_frequency}
                          truncated={left().truncated}
                          totalIntervalCount={left().interval_count}
                          intervals={timelineIntervals(state().baseline, baseline()!, "baseline")}
                        />
                        <ScopeTimeline
                          title="B · Candidate"
                          subtitle={`${right().interval_count.toLocaleString()} matching intervals`}
                          begin={right().begin_cycle}
                          end={right().end_cycle}
                          cycleFrequency={right().index.cycle_frequency}
                          truncated={right().truncated}
                          totalIntervalCount={right().interval_count}
                          intervals={timelineIntervals(state().candidate, candidate()!, "candidate")}
                        />
                      </div>
                    </div>
                  );
                }}
              </Match>
            </Switch>
          </Match>
        </Switch>
      </Show>
    </section>
  );
}
