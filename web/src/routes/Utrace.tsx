import { For, Show, createMemo, createSignal } from "solid-js";
import { DropZone } from "../components/DropZone";
import { DonutChart, HorizontalBars, LineSeriesChart } from "../components/Charts";
import { FrameBrowser } from "../components/FrameBrowser";
import { ScopeTimeline, type TimelineLaneInterval } from "../components/ScopeTimeline";
import {
  ParseRequestError,
  utraceDashboard,
  utraceInventory,
} from "../lib/api";
import type {
  CorrelatedFrameSummary,
  UtraceDashboard,
  UtraceInventory,
} from "../lib/types";

export default function UtracePage() {
  const [busy, setBusy] = createSignal(false);
  const [timelineBusy, setTimelineBusy] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);
  const [file, setFile] = createSignal<File | null>(null);
  const [fileName, setFileName] = createSignal<string | null>(null);
  const [dashboard, setDashboard] = createSignal<UtraceDashboard | null>(null);
  const [inventory, setInventory] = createSignal<UtraceInventory | null>(null);
  const [selectedFrame, setSelectedFrame] = createSignal<number | null>(null);
  const [frameDetail, setFrameDetail] = createSignal<UtraceDashboard | null>(null);

  const frames = createMemo(
    () => dashboard()?.dashboard.frame_correlation.frames ?? [],
  );

  const selectedSummary = createMemo((): CorrelatedFrameSummary | null => {
    const frame = selectedFrame();
    if (frame == null) return null;
    return frames().find((row) => row.frame_number === frame) ?? null;
  });

  const frameSeries = createMemo(() =>
    frames().map((frame) => ({
      frame: String(frame.frame_number),
      cpu_s: frame.cpu_metadata_seconds ?? 0,
      gpu_work: frame.gpu_work_cycles,
    })),
  );

  const threadNames = createMemo(() => {
    const map = new Map<number, string>();
    for (const thread of dashboard()?.dashboard.cpu.threads ?? []) {
      map.set(thread.thread_id, thread.name ?? `thread ${thread.thread_id}`);
    }
    for (const info of dashboard()?.dashboard.thread_info ?? []) {
      if (info.name && !map.has(info.thread_id)) {
        map.set(info.thread_id, info.name);
      }
    }
    return map;
  });

  const perFrameScopes = createMemo(() => {
    const summary = selectedSummary();
    if (!summary) return [];
    return summary.top_cpu_scopes?.slice(0, 12).map((scope) => ({
      name: scope.name,
      value: scope.total_seconds ?? scope.total_cycles,
    })) ?? [];
  });

  const cpuTimelineIntervals = createMemo((): TimelineLaneInterval[] => {
    const timeline = frameDetail()?.dashboard.cpu.timeline;
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

  const gpuTimelineIntervals = createMemo((): TimelineLaneInterval[] => {
    const timeline = frameDetail()?.dashboard.gpu.timeline;
    if (!timeline) return [];
    return timeline.intervals.map((interval, index) => ({
      id: `gpu-${index}-${interval.queue_id}-${interval.start_timestamp}`,
      lane: `queue ${interval.queue_id} · ${interval.kind}`,
      label: interval.name,
      start: interval.start_timestamp,
      end: interval.end_timestamp,
      durationLabel: `${interval.duration} ts`,
    }));
  });

  const decodeCoverage = createMemo(() => {
    const summary = inventory()?.inventory.summary;
    if (!summary) return [];
    return [
      { name: "decoded", value: summary.decoded_event_types },
      { name: "partial", value: summary.partial_event_types },
      { name: "raw", value: summary.raw_event_types },
    ].filter((row) => row.value > 0);
  });

  const onFile = async (next: File) => {
    setBusy(true);
    setError(null);
    setFile(next);
    setFileName(next.name);
    setSelectedFrame(null);
    setFrameDetail(null);
    try {
      const [dash, inv] = await Promise.all([
        utraceDashboard(next, { max_frames: 500 }),
        utraceInventory(next),
      ]);
      setDashboard(dash);
      setInventory(inv);

      const hottest = [...dash.dashboard.frame_correlation.frames].sort(
        (a, b) =>
          (b.cpu_metadata_seconds ?? 0) - (a.cpu_metadata_seconds ?? 0),
      )[0];
      if (hottest) {
        await loadFrameTimeline(next, hottest.frame_number);
      }
    } catch (err) {
      setDashboard(null);
      setInventory(null);
      if (err instanceof ParseRequestError) {
        setError(err.body.stderr || err.message);
      } else {
        setError(err instanceof Error ? err.message : String(err));
      }
    } finally {
      setBusy(false);
    }
  };

  const loadFrameTimeline = async (source: File, frameNumber: number) => {
    setTimelineBusy(true);
    setSelectedFrame(frameNumber);
    setError(null);
    try {
      const detail = await utraceDashboard(source, {
        max_frames: 500,
        frame: frameNumber,
        timeline_limit: 2500,
        gpu_frame: frameNumber,
        gpu_timeline_limit: 2500,
      });
      setFrameDetail(detail);
    } catch (err) {
      setFrameDetail(null);
      if (err instanceof ParseRequestError) {
        setError(err.body.stderr || err.message);
      } else {
        setError(err instanceof Error ? err.message : String(err));
      }
    } finally {
      setTimelineBusy(false);
    }
  };

  const onSelectFrame = async (frameNumber: number) => {
    const source = file();
    if (!source) return;
    await loadFrameTimeline(source, frameNumber);
  };

  return (
    <section class="page">
      <header class="page-head">
        <p class="eyebrow">Route /utrace</p>
        <h1>Trace workbench</h1>
        <p class="lede">
          Overview charts find expensive frames. Select one to load a bounded
          CPU/GPU timeline (Insights-style swimlanes) from{" "}
          <code>uasset utrace dashboard --frame</code>.
        </p>
      </header>

      <DropZone
        accept=".utrace"
        label="Drop a .utrace"
        hint="Selecting a frame re-parses the capture with timeline options — expect a short wait on large files."
        busy={busy() || timelineBusy()}
        onFile={onFile}
      />

      <Show when={error()}>
        <div class="banner error" role="alert">
          <strong>Parse failed</strong>
          <pre>{error()}</pre>
        </div>
      </Show>

      <Show when={dashboard()} keyed>
        {(dash) => (
          <>
            <div class="status-row">
              <span class={`pill status-${dash.status}`}>{dash.status}</span>
              <span class="mono">{fileName()}</span>
              <span class="muted">
                {dash.dashboard.header.magic} · protocol{" "}
                {dash.dashboard.header.protocol}
              </span>
              <Show when={timelineBusy()}>
                <span class="pill status-partial">loading timeline…</span>
              </Show>
            </div>

            <div class="stat-grid">
              <Stat
                label="Correlated frames"
                value={`${dash.dashboard.frame_correlation.frames.length}${
                  dash.dashboard.frame_correlation.truncated ? "+" : ""
                }`}
              />
              <Stat
                label="Selected frame"
                value={selectedFrame() != null ? String(selectedFrame()) : "—"}
              />
              <Stat
                label="CPU intervals"
                value={String(
                  frameDetail()?.dashboard.cpu.timeline?.interval_count ?? "—",
                )}
              />
              <Stat
                label="GPU intervals"
                value={String(
                  frameDetail()?.dashboard.gpu.timeline?.interval_count ?? "—",
                )}
              />
            </div>

            <div class="chart-grid">
              <LineSeriesChart
                title="CPU frame cost"
                subtitle="cpu_metadata_seconds — use the frame browser below to drill in"
                data={frameSeries()}
                xKey="frame"
                height={240}
                series={[
                  { key: "cpu_s", name: "CPU seconds", class: "series-amber" },
                ]}
              />
              <LineSeriesChart
                title="GPU work cycles"
                subtitle="Per correlated frame"
                data={frameSeries()}
                xKey="frame"
                height={240}
                series={[
                  {
                    key: "gpu_work",
                    name: "GPU work cycles",
                    class: "series-steel",
                  },
                ]}
              />
            </div>

            <FrameBrowser
              frames={frames()}
              selectedFrame={selectedFrame()}
              onSelect={onSelectFrame}
            />

            <Show when={selectedSummary()}>
              {(summary) => (
                <div class="chart-grid">
                  <HorizontalBars
                    title={`Frame ${summary().frame_number} · top CPU scopes`}
                    subtitle="From frame correlation (available before timeline reload)"
                    data={perFrameScopes()}
                    valueLabel="cost"
                  />
                  <section class="panel frame-summary">
                    <p class="eyebrow">Frame summary</p>
                    <h2>Frame {summary().frame_number}</h2>
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
                          {summary().gpu_work_cycles.toLocaleString()} cy ·{" "}
                          {summary().gpu_work_count} events
                        </dd>
                      </div>
                      <div>
                        <dt>GPU breadcrumbs</dt>
                        <dd>
                          {summary().gpu_breadcrumb_cycles.toLocaleString()} cy ·{" "}
                          {summary().gpu_breadcrumb_count} events
                        </dd>
                      </div>
                    </dl>
                  </section>
                </div>
              )}
            </Show>

            <Show when={frameDetail()?.dashboard.cpu.timeline}>
              {(timeline) => (
                <ScopeTimeline
                  title={`CPU timeline · frame ${timeline().frame_number}`}
                  subtitle={
                    timeline().duration_seconds != null
                      ? `${(timeline().duration_seconds! * 1000).toFixed(2)} ms window · ${timeline().interval_count} intervals`
                      : `${timeline().interval_count} intervals`
                  }
                  begin={timeline().begin_cycle}
                  end={timeline().end_cycle}
                  truncated={timeline().truncated}
                  intervals={cpuTimelineIntervals()}
                  empty="No CPU intervals retained for this frame."
                />
              )}
            </Show>

            <Show when={frameDetail()?.dashboard.gpu.timeline}>
              {(timeline) => (
                <ScopeTimeline
                  title={`GPU timeline · frame ${timeline().frame_number}`}
                  subtitle={`${timeline().interval_count} work/breadcrumb intervals`}
                  begin={timeline().begin_timestamp}
                  end={timeline().end_timestamp}
                  truncated={timeline().truncated}
                  intervals={gpuTimelineIntervals()}
                  empty="No GPU intervals retained for this frame."
                />
              )}
            </Show>

            <details class="panel secondary-panel">
              <summary>Decoder coverage & inventory</summary>
              <div class="chart-grid" style={{ "margin-top": "1rem" }}>
                <DonutChart
                  title="Event decode coverage"
                  subtitle="Declared event types by decoder status"
                  data={decodeCoverage()}
                />
                <HorizontalBars
                  title="Hottest observed events"
                  subtitle="Volume, not cost — useful for parser gaps"
                  data={[...(inventory()?.inventory.events ?? [])]
                    .sort((a, b) => b.observed_count - a.observed_count)
                    .slice(0, 12)
                    .map((event) => ({
                      name: `${event.logger}.${event.event}`,
                      value: event.observed_count,
                    }))}
                  valueLabel="count"
                />
              </div>
              <Show when={(inventory()?.inventory.events.length ?? 0) > 0}>
                <div class="table-wrap" style={{ "margin-top": "1rem" }}>
                  <table>
                    <thead>
                      <tr>
                        <th>Logger.Event</th>
                        <th>Status</th>
                        <th>Count</th>
                      </tr>
                    </thead>
                    <tbody>
                      <For
                        each={[...(inventory()?.inventory.events ?? [])]
                          .sort((a, b) => b.observed_count - a.observed_count)
                          .slice(0, 40)}
                      >
                        {(event) => (
                          <tr>
                            <td class="mono">
                              {event.logger}.{event.event}
                            </td>
                            <td>
                              <span class={`pill status-${event.decode_status}`}>
                                {event.decode_status}
                              </span>
                            </td>
                            <td>{event.observed_count}</td>
                          </tr>
                        )}
                      </For>
                    </tbody>
                  </table>
                </div>
              </Show>
            </details>
          </>
        )}
      </Show>
    </section>
  );
}

function Stat(props: { label: string; value: string }) {
  return (
    <div class="stat">
      <span class="stat-label">{props.label}</span>
      <span class="stat-value">{props.value}</span>
    </div>
  );
}
