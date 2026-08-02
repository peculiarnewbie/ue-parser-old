import { For, Show, createMemo, createSignal, type Accessor } from "solid-js";
import type { ColumnDef } from "@tanstack/solid-table";
import { DonutChart, HorizontalBars, LineSeriesChart } from "../Charts";
import { SortableTable, StatCard } from "./SortableTable";
import {
  aggregateTopBreadcrumbs,
  aggregateTopScopes,
  cycleInWindow,
  frameInWindow,
  intervalOverlapsWindow,
  type AnalysisWindow,
} from "../../lib/analysis-range";
import {
  formatBytes,
  formatCompact,
  formatCycles,
  formatGpuCost,
  formatMs,
  formatNumber,
  formatSeconds,
  gpuCostUnit,
  gpuCostValue,
  percentile,
} from "../../lib/format";
import type {
  CounterSummary,
  TraceDashboardBody,
  UtraceInventory,
} from "../../lib/types";

type Dash = Accessor<TraceDashboardBody>;
type WindowAcc = Accessor<AnalysisWindow>;

export function OverviewPanel(props: {
  dash: Dash;
  window: WindowAcc;
  inventory: Accessor<UtraceInventory | null>;
  onOpenFrames: () => void;
}) {
  const frames = createMemo(() => props.window().frames);
  const cpuCosts = createMemo(() =>
    frames()
      .map((frame) => frame.cpu_metadata_seconds ?? 0)
      .filter((value) => value > 0)
      .sort((a, b) => a - b),
  );
  const topScopes = createMemo(() => {
    const fromFrames = aggregateTopScopes(frames(), 12);
    if (props.window().active || fromFrames.length > 0) {
      return fromFrames.map((scope) => ({
        name: scope.name,
        value: scope.total_seconds ?? scope.total_cycles,
      }));
    }
    return [...(props.dash().cpu.scopes ?? [])]
      .sort((a, b) => (b.total_seconds ?? 0) - (a.total_seconds ?? 0))
      .slice(0, 12)
      .map((scope) => ({
        name: scope.name,
        value: scope.total_seconds ?? scope.total_cycles,
      }));
  });
  const cycleFrequency = () => props.dash().prologue?.cycle_frequency;
  const topGpu = createMemo(() =>
    aggregateTopBreadcrumbs(frames(), 12).map((crumb) => ({
      name: crumb.name,
      value: gpuCostValue(crumb.total_cycles, cycleFrequency()),
    })),
  );
  const decodeCoverage = createMemo(() => {
    const summary = props.inventory()?.inventory.summary;
    if (!summary) return [];
    return [
      { name: "decoded", value: summary.decoded_event_types },
      { name: "partial", value: summary.partial_event_types },
      { name: "raw", value: summary.raw_event_types },
    ].filter((row) => row.value > 0);
  });

  const p50 = () => percentile(cpuCosts(), 50);
  const p90 = () => percentile(cpuCosts(), 90);
  const p99 = () => percentile(cpuCosts(), 99);
  const worst = () => cpuCosts()[cpuCosts().length - 1] ?? null;

  const session = () => props.dash().session;
  const prologue = () => props.dash().prologue;

  return (
    <div class="panel-stack">
      <Show when={session()}>
        {(info) => (
          <section class="panel session-panel">
            <p class="eyebrow">Capture identity</p>
            <h2>
              {info().project_name || info().app_name}{" "}
              <span class="muted">· {info().platform}</span>
            </h2>
            <dl class="kv kv-inline">
              <div>
                <dt>Build</dt>
                <dd>
                  {info().configuration} / {info().target_type} ·{" "}
                  {info().build_version}
                </dd>
              </div>
              <div>
                <dt>Branch</dt>
                <dd>
                  {info().branch || "—"} · CL {info().changelist || "—"}
                </dd>
              </div>
              <div>
                <dt>Command</dt>
                <dd class="truncate-cmd">{info().command_line || "—"}</dd>
              </div>
            </dl>
          </section>
        )}
      </Show>

      <div class="stat-grid">
        <StatCard
          label={props.window().active ? "Frames in brush" : "Correlated frames"}
          value={`${frames().length}${!props.window().active && props.dash().frame_correlation.truncated ? "+" : ""}`}
          hint={
            props.window().active
              ? `frames ${props.window().startFrame}–${props.window().endFrame}`
              : `of ${props.dash().frame_correlation.total_frame_count} total`
          }
        />
        <StatCard
          label="CPU p50 / p90 / p99"
          value={`${formatMs(p50())} / ${formatMs(p90())} / ${formatMs(p99())}`}
          hint={worst() != null ? `worst ${formatMs(worst())}` : undefined}
        />
        <StatCard
          label="Top scope cost"
          value={
            topScopes()[0]
              ? formatSeconds(Number(topScopes()[0]!.value))
              : "—"
          }
          hint={topScopes()[0]?.name}
        />
        <StatCard
          label="GPU work in range"
          value={formatGpuCost(
            frames().reduce((sum, frame) => sum + frame.gpu_work_cycles, 0),
            cycleFrequency(),
          )}
          hint={`${props.dash().gpu.queues.length} queues`}
        />
        <StatCard
          label="Counters"
          value={String(props.dash().counters.counters.length)}
          hint={`${formatCompact(props.dash().counters.samples)} samples`}
        />
        <StatCard
          label="Memory net"
          value={formatBytes(props.dash().memory.allocs.net_bytes)}
          hint={`${formatCompact(props.dash().memory.allocs.count)} allocs`}
        />
        <StatCard
          label="Tasks waited"
          value={String(props.dash().tasks.wait_count)}
          hint={`${props.dash().tasks.completed} completed`}
        />
        <StatCard
          label="Cycle frequency"
          value={
            prologue()?.cycle_frequency
              ? formatCompact(prologue()!.cycle_frequency)
              : "—"
          }
          hint={prologue() ? `${prologue()!.pointer_size * 8}-bit` : undefined}
        />
      </div>

      <div class="chart-grid">
        <HorizontalBars
          title={
            props.window().active
              ? "Hottest CPU scopes (brushed frames)"
              : "Hottest CPU scopes"
          }
          subtitle={
            props.window().active
              ? "Merged from per-frame top scopes in the brush"
              : "Capture-wide inclusive cost"
          }
          data={topScopes()}
          valueLabel="cost"
        />
        <Show
          when={topGpu().length > 0}
          fallback={
            <DonutChart
              title="Decoder coverage"
              subtitle="Declared event types by status"
              data={decodeCoverage()}
            />
          }
        >
          <HorizontalBars
            title="Top GPU breadcrumbs in range"
            data={topGpu()}
            valueLabel={gpuCostUnit(cycleFrequency())}
          />
        </Show>
      </div>

      <div class="overview-actions">
        <button type="button" class="btn primary compact" onClick={props.onOpenFrames}>
          Inspect frames
        </button>
        <Show when={props.dash().dispatch}>
          {(dispatch) => (
            <span class="pill status-partial">
              serial dispatch · {dispatch().gap_count} gaps ·{" "}
              {dispatch().serial_ordered ? "ordered" : "unordered"}
            </span>
          )}
        </Show>
      </div>
    </div>
  );
}

export function CpuPanel(props: {
  dash: Dash;
  window: WindowAcc;
  onQueryThread: (threadId: number) => void;
}) {
  const threads = createMemo(() =>
    [...(props.dash().cpu.threads ?? [])].sort(
      (a, b) => (b.total_seconds ?? 0) - (a.total_seconds ?? 0),
    ),
  );
  const scopes = createMemo(() => {
    if (props.window().active) {
      return aggregateTopScopes(props.window().frames, 80);
    }
    return [...(props.dash().cpu.scopes ?? [])].sort(
      (a, b) => (b.total_seconds ?? 0) - (a.total_seconds ?? 0),
    );
  });
  const named = createMemo(() =>
    [...(props.dash().cpu.named_events ?? [])].sort(
      (a, b) => b.observed_count - a.observed_count,
    ),
  );
  const batches = () => props.dash().cpu.batches;
  const metadata = () => props.dash().cpu.metadata;

  type ScopeRow = TraceDashboardBody["cpu"]["scopes"][number];
  type ThreadRow = TraceDashboardBody["cpu"]["threads"][number];

  const scopeColumns = createMemo<ColumnDef<ScopeRow, unknown>[]>(() => [
    { accessorKey: "name", header: "Scope" },
    { accessorKey: "count", header: "Count", cell: (info) => formatCompact(info.getValue<number>()) },
    {
      accessorKey: "total_seconds",
      header: "Seconds",
      cell: (info) => formatSeconds(info.getValue<number | undefined>()),
    },
    {
      accessorKey: "total_cycles",
      header: "Cycles",
      cell: (info) => formatCompact(info.getValue<number>()),
    },
  ]);

  const threadColumns = createMemo<ColumnDef<ThreadRow, unknown>[]>(() => [
    {
      id: "label",
      header: "Thread",
      accessorFn: (row) => row.name ?? `thread ${row.thread_id}`,
    },
    { accessorKey: "thread_id", header: "ID" },
    {
      accessorKey: "active_group",
      header: "Group",
      cell: (info) => info.getValue<string | undefined>() ?? "—",
    },
    { accessorKey: "count", header: "Scopes", cell: (info) => formatCompact(info.getValue<number>()) },
    {
      accessorKey: "total_seconds",
      header: "Seconds",
      cell: (info) => formatSeconds(info.getValue<number | undefined>()),
    },
  ]);

  return (
    <div class="panel-stack">
      <div class="stat-grid">
        <StatCard label="Scope specs" value={String(props.dash().cpu.specs?.length ?? 0)} />
        <StatCard
          label="Decoded intervals"
          value={formatCompact(batches()?.intervals ?? 0)}
          hint={`${formatCompact(batches()?.decoded_records ?? 0)} records`}
        />
        <StatCard
          label="Unresolved specs"
          value={String(batches()?.unresolved_specs ?? 0)}
        />
        <StatCard
          label="Unterminated"
          value={String(batches()?.unterminated_scopes ?? 0)}
          hint={`${batches()?.unmatched_ends ?? 0} unmatched ends`}
        />
        <StatCard
          label="Metadata scopes"
          value={formatCompact(metadata()?.scopes ?? 0)}
          hint={`${metadata()?.resolved_scopes ?? 0} resolved`}
        />
        <StatCard
          label="Restored metadata"
          value={String(batches()?.restored_metadata_scopes ?? 0)}
        />
      </div>

      <div class="chart-grid">
        <HorizontalBars
          title="Top CPU scopes"
          data={scopes()
            .slice(0, 12)
            .map((scope) => ({
              name: scope.name,
              value: scope.total_seconds ?? scope.total_cycles,
            }))}
          valueLabel="cost"
        />
        <HorizontalBars
          title="Hottest threads"
          data={threads()
            .slice(0, 12)
            .map((thread) => ({
              name: thread.name ?? `thread ${thread.thread_id}`,
              value: thread.total_seconds ?? thread.total_cycles,
            }))}
          valueLabel="cost"
        />
      </div>

      <SortableTable
        eyebrow="CPU scopes"
        title={
          props.window().active
            ? "Scopes in brushed frames"
            : "Capture-wide scope rollup"
        }
        subtitle={
          props.window().active
            ? "Merged from per-frame top scopes — not a full exclusive recompute"
            : "Inclusive totals across decoded EventBatchV3 intervals"
        }
        data={scopes()}
        columns={scopeColumns()}
        initialSort={[{ id: "total_seconds", desc: true }]}
        filterFn={(row, query) => row.name.toLowerCase().includes(query)}
        filterPlaceholder="scope name…"
        maxHeightClass="frame-browser-wrap"
      />

      <SortableTable
        eyebrow="Threads"
        title="Per-thread CPU cost"
        subtitle="Click a row to prefill the timeline thread filter"
        data={threads()}
        columns={threadColumns()}
        initialSort={[{ id: "total_seconds", desc: true }]}
        filterFn={(row, query) =>
          (row.name ?? "").toLowerCase().includes(query) ||
          String(row.thread_id).includes(query)
        }
        onRowClick={(row) => props.onQueryThread(row.thread_id)}
        maxHeightClass="frame-browser-wrap"
      />

      <Show when={named().length > 0}>
        <SortableTable
          eyebrow="Cpu.*"
          title="Named CPU events"
          subtitle="Generic Cpu.&lt;Name&gt; markers (separate from CpuProfiler specs)"
          data={named()}
          columns={[
            { accessorKey: "event", header: "Event" },
            {
              accessorKey: "observed_count",
              header: "Count",
              cell: (info) => formatCompact(info.getValue<number>()),
            },
          ]}
          initialSort={[{ id: "observed_count", desc: true }]}
          maxHeightClass="compact-table-wrap"
        />
      </Show>
    </div>
  );
}

export function GpuPanel(props: {
  dash: Dash;
  window: WindowAcc;
  gpuTimeline: Accessor<import("../../lib/types").GpuTimelineDashboard | null>;
  gpuBusy: Accessor<boolean>;
  onLoadGpuFrame: (frameNumber: number) => void;
}) {
  const cycleFrequency = () => props.dash().prologue?.cycle_frequency;
  const costUnit = () => gpuCostUnit(cycleFrequency());
  const queues = createMemo(() =>
    [...props.dash().gpu.queues].sort((a, b) => b.work_total_cycles - a.work_total_cycles),
  );
  const breadcrumbs = createMemo(() => {
    if (props.window().active) {
      return aggregateTopBreadcrumbs(props.window().frames, 40);
    }
    return props.dash().gpu.breadcrumbs?.top ?? [];
  });
  const latency = () => props.dash().gpu.submission_latency;
  const gpuFrames = createMemo(() =>
    [...props.dash().gpu.frames]
      .filter((frame) => frameInWindow(frame.frame_number, props.window()))
      .sort((a, b) => b.work_total_cycles - a.work_total_cycles),
  );

  return (
    <div class="panel-stack">
      <div class="stat-grid">
        <StatCard
          label="Queues"
          value={String(queues().length)}
          hint={props.dash().gpu.version != null ? `GpuProfiler v${props.dash().gpu.version}` : undefined}
        />
        <StatCard
          label="Work intervals"
          value={formatCompact(props.dash().gpu.work?.intervals ?? 0)}
          hint={formatGpuCost(
            props.dash().gpu.work?.total_cycles ?? 0,
            cycleFrequency(),
          )}
        />
        <StatCard
          label="Breadcrumbs"
          value={formatCompact(props.dash().gpu.breadcrumbs?.intervals ?? 0)}
          hint={formatGpuCost(
            props.dash().gpu.breadcrumbs?.total_cycles ?? 0,
            cycleFrequency(),
          )}
        />
        <StatCard
          label="Submit→GPU median"
          value={
            latency()
              ? formatGpuCost(latency()!.median_delay_cycles, cycleFrequency())
              : "—"
          }
          hint={
            cycleFrequency()
              ? "converted with cycle_frequency"
              : "cycles (no cycle_frequency)"
          }
        />
      </div>

      <div class="chart-grid">
        <HorizontalBars
          title="Queue work"
          data={queues()
            .slice(0, 12)
            .map((queue) => ({
              name: queue.name ?? `queue ${queue.queue_id}`,
              value: gpuCostValue(queue.work_total_cycles, cycleFrequency()),
            }))}
          valueLabel={costUnit()}
        />
        <HorizontalBars
          title="Top GPU breadcrumbs"
          data={breadcrumbs()
            .slice(0, 12)
            .map((crumb) => ({
              name: crumb.name,
              value: gpuCostValue(crumb.total_cycles, cycleFrequency()),
            }))}
          valueLabel={costUnit()}
        />
      </div>

      <SortableTable
        eyebrow="Queues"
        title="GPU queue rollup"
        data={queues()}
        columns={[
          {
            id: "label",
            header: "Queue",
            accessorFn: (row) => row.name ?? `queue ${row.queue_id}`,
          },
          { accessorKey: "queue_id", header: "ID" },
          {
            accessorKey: "work_count",
            header: "Work",
            cell: (info) => formatCompact(info.getValue<number>()),
          },
          {
            accessorKey: "work_total_cycles",
            header: costUnit() === "ms" ? "Work ms" : "Work cy",
            cell: (info) =>
              formatGpuCost(info.getValue<number>(), cycleFrequency()),
          },
          {
            accessorKey: "wait_total_cycles",
            header: costUnit() === "ms" ? "Wait ms" : "Wait cy",
            cell: (info) =>
              formatGpuCost(info.getValue<number>(), cycleFrequency()),
          },
          {
            accessorKey: "draw_count",
            header: "Draws",
            cell: (info) => formatCompact(info.getValue<number>()),
          },
          {
            accessorKey: "primitive_count",
            header: "Prims",
            cell: (info) => formatCompact(info.getValue<number>()),
          },
          {
            accessorKey: "breadcrumb_count",
            header: "Crumbs",
            cell: (info) => formatCompact(info.getValue<number>()),
          },
        ]}
        initialSort={[{ id: "work_total_cycles", desc: true }]}
        maxHeightClass="frame-browser-wrap"
      />

      <SortableTable
        eyebrow="GPU frames"
        title="Hottest queue-local frames"
        subtitle="Click to load a bounded GPU work/breadcrumb timeline (re-parses dashboard for that frame)"
        data={gpuFrames().slice(0, 200)}
        columns={[
          { accessorKey: "frame_number", header: "Frame" },
          { accessorKey: "queue_id", header: "Queue" },
          {
            accessorKey: "work_total_cycles",
            header: costUnit() === "ms" ? "Work ms" : "Work cy",
            cell: (info) =>
              formatGpuCost(info.getValue<number>(), cycleFrequency()),
          },
          {
            accessorKey: "breadcrumb_total_cycles",
            header: costUnit() === "ms" ? "Crumb ms" : "Crumb cy",
            cell: (info) =>
              formatGpuCost(info.getValue<number>(), cycleFrequency()),
          },
          {
            accessorKey: "wait_total_cycles",
            header: costUnit() === "ms" ? "Wait ms" : "Wait cy",
            cell: (info) =>
              formatGpuCost(info.getValue<number>(), cycleFrequency()),
          },
          {
            accessorKey: "draw_count",
            header: "Draws",
            cell: (info) => formatCompact(info.getValue<number>()),
          },
        ]}
        initialSort={[{ id: "work_total_cycles", desc: true }]}
        onRowClick={(row) => props.onLoadGpuFrame(row.frame_number)}
        maxHeightClass="frame-browser-wrap"
      />

      <Show when={props.gpuBusy()}>
        <p class="muted">Loading GPU timeline…</p>
      </Show>
      <Show when={props.gpuTimeline()}>
        {(timeline) => (
          <section class="panel">
            <p class="eyebrow">GPU timeline</p>
            <h2>Frame {timeline().frame_number}</h2>
            <p class="muted datatable-meta">
              {timeline().interval_count} intervals
              {timeline().truncated ? " · truncated" : ""} · timestamps are
              GPU-domain (not CPU cycles)
            </p>
            <div class="table-wrap datatable-wrap frame-browser-wrap">
              <table class="datatable">
                <thead>
                  <tr>
                    <th>Queue</th>
                    <th>Kind</th>
                    <th>Name</th>
                    <th>Start</th>
                    <th>End</th>
                    <th>Duration</th>
                  </tr>
                </thead>
                <tbody>
                  <For each={timeline().intervals.slice(0, 400)}>
                    {(interval) => (
                      <tr>
                        <td class="mono">{interval.queue_id}</td>
                        <td>
                          <span class="pill">{interval.kind}</span>
                        </td>
                        <td class="mono">{interval.name}</td>
                        <td class="mono">{formatCompact(interval.start_timestamp)}</td>
                        <td class="mono">{formatCompact(interval.end_timestamp)}</td>
                        <td class="mono">{formatCompact(interval.duration)}</td>
                      </tr>
                    )}
                  </For>
                </tbody>
              </table>
            </div>
          </section>
        )}
      </Show>

      <Show when={latency()?.samples?.length}>
        <SortableTable
          eyebrow="Submission latency"
          title="GPU-start − CPU-submit samples"
          subtitle="Positive delay means the GPU started after CPU submit"
          data={latency()!.samples ?? []}
          columns={[
            { accessorKey: "queue_id", header: "Queue" },
            {
              accessorKey: "delay_cycles",
              header: "Delay cy",
              cell: (info) => formatCompact(info.getValue<number>()),
            },
            {
              accessorKey: "cpu_submit_timestamp",
              header: "CPU submit",
              cell: (info) => formatCompact(info.getValue<number>()),
            },
            {
              accessorKey: "gpu_timestamp_top",
              header: "GPU TOP",
              cell: (info) => formatCompact(info.getValue<number>()),
            },
          ]}
          initialSort={[{ id: "delay_cycles", desc: true }]}
          maxHeightClass="compact-table-wrap"
        />
      </Show>
    </div>
  );
}

export function MetricsPanel(props: { dash: Dash; window: WindowAcc }) {
  const counters = createMemo(() => {
    const base = [...props.dash().counters.counters].sort(
      (a, b) => b.samples - a.samples,
    );
    if (!props.window().active) return base;
    return base
      .map((counter) => {
        const points = (counter.sample_points ?? []).filter((point) =>
          cycleInWindow(point.cycle, props.window()),
        );
        if (points.length === 0 && (counter.sample_points?.length ?? 0) > 0) {
          return null;
        }
        if (points.length === 0) return counter;
        const values = points.map((point) => point.value);
        return {
          ...counter,
          samples: points.length,
          sample_points: points,
          min: Math.min(...values),
          max: Math.max(...values),
          latest: values[values.length - 1],
          first_cycle: points[0]?.cycle,
          last_cycle: points[points.length - 1]?.cycle,
        } satisfies CounterSummary;
      })
      .filter((row): row is CounterSummary => row != null);
  });
  const [selectedId, setSelectedId] = createSignal<number | null>(null);
  const selected = createMemo((): CounterSummary | null => {
    const id = selectedId();
    const list = counters();
    if (id != null) {
      return list.find((counter) => counter.id === id) ?? null;
    }
    return (
      list.find((counter) => (counter.sample_points?.length ?? 0) > 1) ??
      list[0] ??
      null
    );
  });
  const series = createMemo(() => {
    const points = selected()?.sample_points ?? [];
    const frameCycles = props.dash().frame_correlation.frames
      .map((frame) => frame.cpu_begin_cycle)
      .filter((cycle): cycle is number => cycle != null);
    const captureStart = Math.min(...frameCycles, points[0]?.cycle ?? 0);
    const frequency = props.dash().prologue?.cycle_frequency;
    return points.map((point) => ({
      time: formatCounterTime(point.cycle, captureStart, frequency),
      value: point.value,
      cycle: point.cycle,
    }));
  });
  const statsSamples = createMemo(() => {
    const samples = [...(props.dash().stats.samples ?? [])].sort(
      (a, b) => b.samples - a.samples,
    );
    if (!props.window().active) return samples;
    return samples
      .map((sample) => {
        const points = (sample.sample_points ?? []).filter((point) =>
          cycleInWindow(point.cycle, props.window()),
        );
        if (points.length === 0 && (sample.sample_points?.length ?? 0) > 0) {
          return null;
        }
        if (points.length === 0) return sample;
        const values = points.map((point) => point.value);
        return {
          ...sample,
          samples: points.length,
          sample_points: points,
          min: Math.min(...values),
          max: Math.max(...values),
          latest: values[values.length - 1],
          first_cycle: points[0]?.cycle,
          last_cycle: points[points.length - 1]?.cycle,
        };
      })
      .filter((row): row is NonNullable<typeof row> => row != null);
  });
  const statName = (id: number) =>
    props.dash().stats.stats.find((stat) => stat.id === id)?.name ?? `stat ${id}`;

  return (
    <div class="panel-stack">
      <div class="stat-grid">
        <StatCard
          label="Counter specs"
          value={String(props.dash().counters.specs)}
          hint={`${formatCompact(props.dash().counters.samples)} samples`}
        />
        <StatCard
          label="Stat specs"
          value={String(props.dash().stats.specs)}
          hint={`${formatCompact(props.dash().stats.sample_events)} sample events`}
        />
        <StatCard
          label="CSV stats"
          value={String(props.dash().csv.stats)}
          hint={`${props.dash().csv.categories} categories`}
        />
        <StatCard
          label="CSV samples"
          value={formatCompact(props.dash().csv.sample_events)}
        />
      </div>

      <Show when={selected() && series().length > 1}>
        <LineSeriesChart
          title={`Counter · ${selected()!.name}`}
          subtitle={`${selected()!.samples} samples · min ${formatNumber(selected()!.min ?? NaN)} · max ${formatNumber(selected()!.max ?? NaN)}`}
          data={series()}
          xKey="time"
          height={260}
          series={[{ key: "value", name: selected()!.name, class: "series-amber" }]}
        />
      </Show>

      <SortableTable
        eyebrow="Counters"
        title="Counter catalog"
        subtitle="Click a row with sample points to chart it"
        data={counters()}
        columns={[
          { accessorKey: "name", header: "Name" },
          { accessorKey: "kind", header: "Kind" },
          {
            accessorKey: "samples",
            header: "Samples",
            cell: (info) => formatCompact(info.getValue<number>()),
          },
          {
            accessorKey: "latest",
            header: "Latest",
            cell: (info) => formatNumber(info.getValue<number | undefined>() ?? NaN),
          },
          {
            accessorKey: "min",
            header: "Min",
            cell: (info) => formatNumber(info.getValue<number | undefined>() ?? NaN),
          },
          {
            accessorKey: "max",
            header: "Max",
            cell: (info) => formatNumber(info.getValue<number | undefined>() ?? NaN),
          },
        ]}
        initialSort={[{ id: "samples", desc: true }]}
        filterFn={(row, query) => row.name.toLowerCase().includes(query)}
        onRowClick={(row) => setSelectedId(row.id)}
        rowClass={(row) => ({
          selected: row.id === (selected()?.id ?? selectedId()),
        })}
        maxHeightClass="frame-browser-wrap"
      />

      <Show when={statsSamples().length > 0}>
        <SortableTable
          eyebrow="Stats samples"
          title="Hot stats (EventBatch2)"
          data={statsSamples()}
          columns={[
            {
              id: "name",
              header: "Stat",
              accessorFn: (row) => statName(row.id),
            },
            {
              accessorKey: "samples",
              header: "Samples",
              cell: (info) => formatCompact(info.getValue<number>()),
            },
            {
              accessorKey: "latest",
              header: "Latest",
              cell: (info) => formatNumber(info.getValue<number | undefined>() ?? NaN),
            },
            {
              accessorKey: "min",
              header: "Min",
              cell: (info) => formatNumber(info.getValue<number | undefined>() ?? NaN),
            },
            {
              accessorKey: "max",
              header: "Max",
              cell: (info) => formatNumber(info.getValue<number | undefined>() ?? NaN),
            },
          ]}
          initialSort={[{ id: "samples", desc: true }]}
          maxHeightClass="frame-browser-wrap"
        />
      </Show>

      <Show when={props.dash().stats.groups.length > 0}>
        <SortableTable
          eyebrow="Stat groups"
          title="Stats catalog groups"
          data={props.dash().stats.groups}
          columns={[
            { accessorKey: "name", header: "Group" },
            { accessorKey: "specs", header: "Specs" },
            { accessorKey: "memory_specs", header: "Memory" },
            { accessorKey: "floating_point_specs", header: "Float" },
            { accessorKey: "clear_every_frame_specs", header: "Clear/frame" },
          ]}
          maxHeightClass="compact-table-wrap"
        />
      </Show>

      <Show when={props.dash().csv.top_categories.length > 0}>
        <SortableTable
          eyebrow="CSV profiler"
          title="CSV categories"
          data={props.dash().csv.top_categories}
          columns={[
            { accessorKey: "name", header: "Category" },
            { accessorKey: "stats", header: "Stats" },
            { accessorKey: "declared_stats", header: "Declared" },
            { accessorKey: "inline_stats", header: "Inline" },
          ]}
          maxHeightClass="compact-table-wrap"
        />
      </Show>
    </div>
  );
}

function formatCounterTime(
  cycle: number,
  captureStart: number,
  cycleFrequency: number | undefined,
): string {
  if (cycleFrequency == null || cycleFrequency <= 0) return `cycle ${formatCompact(cycle)}`;
  const milliseconds = ((cycle - captureStart) / cycleFrequency) * 1000;
  if (milliseconds >= 1000) return `+${(milliseconds / 1000).toFixed(2)} s`;
  return `+${milliseconds.toFixed(milliseconds < 10 ? 2 : 1)} ms`;
}

export function MemoryPanel(props: { dash: Dash; window: WindowAcc }) {
  const heaps = createMemo(() =>
    [...props.dash().memory.allocs.by_root_heap].sort(
      (a, b) => Math.abs(b.net_bytes) - Math.abs(a.net_bytes),
    ),
  );
  const scopes = createMemo(() =>
    [...props.dash().memory.scopes].sort((a, b) => b.count - a.count),
  );
  const stacks = createMemo(() => props.dash().callstacks.stacks ?? []);
  const modules = createMemo(() => props.dash().modules.modules ?? []);
  const llmValues = createMemo(() =>
    (props.dash().memory.llm.latest_values ?? []).filter((value) =>
      cycleInWindow(value.cycle, props.window()),
    ),
  );

  return (
    <div class="panel-stack">
      <div class="stat-grid">
        <StatCard
          label="Allocated"
          value={formatBytes(props.dash().memory.allocs.bytes_allocated)}
          hint={`${formatCompact(props.dash().memory.allocs.count)} ops`}
        />
        <StatCard
          label="Freed"
          value={formatBytes(props.dash().memory.allocs.bytes_freed)}
          hint={`${formatCompact(props.dash().memory.allocs.free_count)} frees`}
        />
        <StatCard
          label="Outstanding"
          value={formatBytes(props.dash().memory.allocs.outstanding_bytes)}
          hint={
            props.dash().memory.allocs.outstanding_overflow
              ? "tracking overflowed"
              : `${formatCompact(props.dash().memory.allocs.outstanding_allocations)} live`
          }
        />
        <StatCard
          label="Callstacks"
          value={`${props.dash().callstacks.retained}/${props.dash().memory.allocs.count ? props.dash().callstacks.observed : props.dash().callstacks.observed}`}
          hint={
            props.dash().callstacks.truncated
              ? `${props.dash().callstacks.dropped} dropped`
              : "catalog retained"
          }
        />
      </div>

      <div class="chart-grid">
        <HorizontalBars
          title="Net bytes by root heap"
          data={heaps()
            .slice(0, 12)
            .map((heap) => ({ name: heap.name, value: Math.abs(heap.net_bytes) }))}
          valueLabel="|net|"
        />
        <HorizontalBars
          title="Memory scopes"
          data={scopes()
            .slice(0, 12)
            .map((scope) => ({
              name: scope.display ?? `tag ${scope.tag}`,
              value: scope.count,
            }))}
          valueLabel="count"
        />
      </div>

      <SortableTable
        eyebrow="Heaps"
        title="Allocation by root heap"
        data={heaps()}
        columns={[
          { accessorKey: "name", header: "Heap" },
          {
            accessorKey: "alloc_count",
            header: "Allocs",
            cell: (info) => formatCompact(info.getValue<number>()),
          },
          {
            accessorKey: "bytes_allocated",
            header: "Alloc bytes",
            cell: (info) => formatBytes(info.getValue<number>()),
          },
          {
            accessorKey: "bytes_freed",
            header: "Free bytes",
            cell: (info) => formatBytes(info.getValue<number>()),
          },
          {
            accessorKey: "net_bytes",
            header: "Net",
            cell: (info) => formatBytes(info.getValue<number>()),
          },
        ]}
        initialSort={[{ id: "net_bytes", desc: true }]}
        maxHeightClass="compact-table-wrap"
      />

      <Show when={(props.dash().memory.allocs.samples?.length ?? 0) > 0}>
        <SortableTable
          eyebrow="Samples"
          title="Retained allocation samples"
          data={props.dash().memory.allocs.samples ?? []}
          columns={[
            {
              accessorKey: "address",
              header: "Address",
              cell: (info) => `0x${info.getValue<number>().toString(16)}`,
            },
            {
              accessorKey: "size",
              header: "Size",
              cell: (info) => formatBytes(info.getValue<number>()),
            },
            { accessorKey: "root_heap", header: "Heap" },
            { accessorKey: "kind", header: "Kind" },
            { accessorKey: "callstack_id", header: "Stack" },
            { accessorKey: "callstack", header: "Resolution" },
          ]}
          maxHeightClass="frame-browser-wrap"
        />
      </Show>

      <Show when={stacks().length > 0}>
        <section class="panel">
          <p class="eyebrow">Callstacks</p>
          <h2>Retained stack catalog</h2>
          <div class="callstack-list">
            <For each={stacks().slice(0, 20)}>
              {(stack) => (
                <details class="callstack-item">
                  <summary>
                    #{stack.id} · {stack.frame_count} frames
                    {stack.frames_truncated ? " · truncated" : ""}
                  </summary>
                  <ol>
                    <For
                      each={
                        stack.mapped_frames && stack.mapped_frames.length > 0
                          ? stack.mapped_frames.map((frame) =>
                              frame.symbol
                                ? `${frame.symbol}${frame.file ? ` (${frame.file}:${frame.line ?? "?"})` : ""}`
                                : frame.module
                                  ? `${frame.module}+${frame.relative_address ?? "?"}`
                                  : frame.address,
                            )
                          : stack.frames
                      }
                    >
                      {(frame) => <li class="mono">{frame}</li>}
                    </For>
                  </ol>
                </details>
              )}
            </For>
          </div>
        </section>
      </Show>

      <Show when={modules().length > 0}>
        <SortableTable
          eyebrow="Modules"
          title="Loaded modules"
          data={modules()}
          columns={[
            { accessorKey: "name", header: "Module" },
            { accessorKey: "base", header: "Base" },
            {
              accessorKey: "size",
              header: "Size",
              cell: (info) => formatBytes(info.getValue<number>()),
            },
            {
              id: "identity",
              header: "PDB identity",
              accessorFn: (row) =>
                row.identity ? `${row.identity.guid} age ${row.identity.age}` : "—",
            },
            {
              accessorKey: "unloaded",
              header: "Unloaded",
              cell: (info) => (info.getValue<boolean>() ? "yes" : ""),
            },
          ]}
          filterFn={(row, query) => row.name.toLowerCase().includes(query)}
          maxHeightClass="frame-browser-wrap"
        />
      </Show>

      <Show when={llmValues().length > 0}>
        <SortableTable
          eyebrow="LLM"
          title={
            props.window().active
              ? "LLM tag values in brush cycles"
              : "Latest LLM tag values"
          }
          data={llmValues()}
          columns={[
            { accessorKey: "tracker_id", header: "Tracker" },
            { accessorKey: "tag", header: "Tag" },
            {
              accessorKey: "value",
              header: "Value",
              cell: (info) => formatBytes(info.getValue<number>()),
            },
            {
              accessorKey: "cycle",
              header: "Cycle",
              cell: (info) => formatCompact(info.getValue<number>()),
            },
          ]}
          maxHeightClass="compact-table-wrap"
        />
      </Show>
    </div>
  );
}

export function IoPanel(props: { dash: Dash; window: WindowAcc }) {
  const files = createMemo(() =>
    [...props.dash().platform_file.files].sort(
      (a, b) => b.bytes_read + b.bytes_written - (a.bytes_read + a.bytes_written),
    ),
  );
  const packages = createMemo(() => props.dash().loading.packages);
  const ioSamples = createMemo(() =>
    props.dash().io_store.request_samples.filter((sample) =>
      cycleInWindow(sample.create_cycle, props.window()),
    ),
  );
  const fileActivity = createMemo(() =>
    props.dash().platform_file.activity_samples.filter((sample) =>
      intervalOverlapsWindow(
        sample.start_cycle,
        sample.end_cycle ?? sample.start_cycle,
        props.window(),
      ),
    ),
  );
  const loadRequests = createMemo(() =>
    props.dash().loading.requests.samples.filter((sample) =>
      intervalOverlapsWindow(sample.start_cycle, sample.end_cycle, props.window()),
    ),
  );

  return (
    <div class="panel-stack">
      <div class="stat-grid">
        <StatCard
          label="Load classes"
          value={String(props.dash().loading.class_count)}
        />
        <StatCard
          label="Packages"
          value={String(props.dash().loading.package_count)}
        />
        <StatCard
          label="Load requests"
          value={`${props.dash().loading.requests.completed}/${props.dash().loading.requests.begun}`}
          hint={formatCycles(props.dash().loading.requests.total_cycles)}
        />
        <StatCard
          label="IoStore"
          value={formatBytes(props.dash().io_store.bytes_completed)}
          hint={`${props.dash().io_store.requests_completed} completed · ${props.dash().io_store.requests_failed} failed`}
        />
        <StatCard
          label="PlatformFile read"
          value={formatBytes(props.dash().platform_file.bytes_read)}
          hint={`${formatCompact(props.dash().platform_file.reads)} reads`}
        />
        <StatCard
          label="PlatformFile write"
          value={formatBytes(props.dash().platform_file.bytes_written)}
          hint={`${formatCompact(props.dash().platform_file.writes)} writes`}
        />
      </div>

      <Show when={loadRequests().length > 0}>
        <SortableTable
          eyebrow="LoadTime"
          title="Load requests in range"
          data={loadRequests()}
          columns={[
            { accessorKey: "request_id", header: "Request" },
            {
              accessorKey: "duration_cycles",
              header: "Duration",
              cell: (info) => formatCycles(info.getValue<number>()),
            },
            {
              accessorKey: "start_cycle",
              header: "Start",
              cell: (info) => formatCompact(info.getValue<number>()),
            },
            {
              accessorKey: "end_cycle",
              header: "End",
              cell: (info) => formatCompact(info.getValue<number>()),
            },
          ]}
          initialSort={[{ id: "duration_cycles", desc: true }]}
          maxHeightClass="compact-table-wrap"
        />
      </Show>

      <Show when={packages().length > 0}>
        <SortableTable
          eyebrow="LoadTime"
          title="Async packages"
          data={packages()}
          columns={[
            { accessorKey: "name", header: "Package" },
            {
              accessorKey: "total_header_size",
              header: "Header",
              cell: (info) => formatBytes(info.getValue<number>()),
            },
            { accessorKey: "import_count", header: "Imports" },
            { accessorKey: "export_count", header: "Exports" },
            {
              accessorKey: "priority",
              header: "Priority",
              cell: (info) => String(info.getValue<number | undefined>() ?? "—"),
            },
          ]}
          filterFn={(row, query) => row.name.toLowerCase().includes(query)}
          maxHeightClass="frame-browser-wrap"
        />
      </Show>

      <Show when={files().length > 0}>
        <SortableTable
          eyebrow="PlatformFile"
          title="Files by traffic"
          data={files()}
          columns={[
            { accessorKey: "path", header: "Path" },
            {
              accessorKey: "bytes_read",
              header: "Read",
              cell: (info) => formatBytes(info.getValue<number>()),
            },
            {
              accessorKey: "bytes_written",
              header: "Write",
              cell: (info) => formatBytes(info.getValue<number>()),
            },
            { accessorKey: "reads", header: "Reads" },
            { accessorKey: "writes", header: "Writes" },
            { accessorKey: "opens", header: "Opens" },
            { accessorKey: "open_failures", header: "Fail" },
          ]}
          initialSort={[{ id: "bytes_read", desc: true }]}
          filterFn={(row, query) => row.path.toLowerCase().includes(query)}
          maxHeightClass="frame-browser-wrap"
        />
      </Show>

      <Show when={ioSamples().length > 0}>
        <SortableTable
          eyebrow="IoStore"
          title="Request samples"
          data={ioSamples()}
          columns={[
            {
              accessorKey: "backend_name",
              header: "Backend",
              cell: (info) => info.getValue<string | undefined>() ?? "—",
            },
            { accessorKey: "status", header: "Status" },
            {
              accessorKey: "size",
              header: "Size",
              cell: (info) => formatBytes(info.getValue<number>()),
            },
            {
              accessorKey: "completed_size",
              header: "Completed",
              cell: (info) => formatBytes(info.getValue<number | undefined>()),
            },
            {
              accessorKey: "create_cycle",
              header: "Create",
              cell: (info) => formatCompact(info.getValue<number>()),
            },
          ]}
          maxHeightClass="frame-browser-wrap"
        />
      </Show>

      <Show when={fileActivity().length > 0}>
        <SortableTable
          eyebrow="PlatformFile"
          title="Activity samples"
          data={fileActivity()}
          columns={[
            { accessorKey: "kind", header: "Kind" },
            {
              accessorKey: "path",
              header: "Path",
              cell: (info) => info.getValue<string | undefined>() ?? "—",
            },
            { accessorKey: "thread_id", header: "Thread" },
            {
              accessorKey: "duration_cycles",
              header: "Duration",
              cell: (info) => formatCycles(info.getValue<number | undefined>()),
            },
            {
              accessorKey: "actual_size",
              header: "Bytes",
              cell: (info) => formatBytes(info.getValue<number | undefined>()),
            },
            {
              accessorKey: "failed",
              header: "Failed",
              cell: (info) => (info.getValue<boolean>() ? "yes" : ""),
            },
          ]}
          maxHeightClass="frame-browser-wrap"
        />
      </Show>

      <Show
        when={
          packages().length === 0 &&
          files().length === 0 &&
          ioSamples().length === 0 &&
          props.dash().loading.class_count === 0
        }
      >
        <p class="chart-empty">
          No LoadTime / IoStore / PlatformFile traffic in this capture.
        </p>
      </Show>
    </div>
  );
}

export function ConcurrencyPanel(props: {
  dash: Dash;
  window: WindowAcc;
  onJumpCycles: (start: number, end: number) => void;
}) {
  const waits = createMemo(() =>
    [...(props.dash().tasks.wait_samples ?? [])]
      .filter((wait) =>
        intervalOverlapsWindow(wait.start_cycle, wait.end_cycle, props.window()),
      )
      .sort((a, b) => b.duration_cycles - a.duration_cycles),
  );
  const named = createMemo(() => props.dash().tasks.named_tasks ?? []);
  const groups = createMemo(() => props.dash().thread_groups.groups);

  return (
    <div class="panel-stack">
      <div class="stat-grid">
        <StatCard label="Created" value={formatCompact(props.dash().tasks.created)} />
        <StatCard label="Completed" value={formatCompact(props.dash().tasks.completed)} />
        <StatCard label="Waits" value={formatCompact(props.dash().tasks.wait_count)} />
        <StatCard
          label="Open waits"
          value={String(props.dash().tasks.open_waits ?? 0)}
          hint={`${props.dash().tasks.unmatched_wait_ends ?? 0} unmatched ends`}
        />
        <StatCard
          label="Thread groups"
          value={String(groups().length)}
          hint={`${props.dash().thread_groups.unclosed_groups} unclosed`}
        />
        <StatCard
          label="Slate widgets"
          value={formatCompact(props.dash().slate.added_widgets)}
        />
      </div>

      <Show when={waits().length > 0}>
        <SortableTable
          eyebrow="TaskTrace"
          title="Wait intervals"
          subtitle="Click to jump the CPU timeline query to that wait window"
          data={waits()}
          columns={[
            { accessorKey: "thread_id", header: "Thread" },
            {
              accessorKey: "duration_cycles",
              header: "Duration",
              cell: (info) => formatCycles(info.getValue<number>()),
            },
            {
              accessorKey: "start_cycle",
              header: "Start",
              cell: (info) => formatCompact(info.getValue<number>()),
            },
            {
              accessorKey: "end_cycle",
              header: "End",
              cell: (info) => formatCompact(info.getValue<number>()),
            },
            {
              id: "tasks",
              header: "Task ids",
              accessorFn: (row) => (row.task_ids ?? []).join(", ") || "—",
            },
          ]}
          initialSort={[{ id: "duration_cycles", desc: true }]}
          onRowClick={(row) => props.onJumpCycles(row.start_cycle, row.end_cycle)}
          maxHeightClass="frame-browser-wrap"
        />
      </Show>

      <Show when={named().length > 0}>
        <SortableTable
          eyebrow="Tasks"
          title="Named tasks"
          data={named()}
          columns={[
            { accessorKey: "task_id", header: "ID" },
            { accessorKey: "debug_name", header: "Name" },
          ]}
          filterFn={(row, query) => row.debug_name.toLowerCase().includes(query)}
          maxHeightClass="compact-table-wrap"
        />
      </Show>

      <Show when={groups().length > 0}>
        <SortableTable
          eyebrow="Thread groups"
          title="Group begin/end balance"
          data={groups()}
          columns={[
            { accessorKey: "name", header: "Group" },
            { accessorKey: "begin_count", header: "Begin" },
            { accessorKey: "end_count", header: "End" },
            {
              accessorKey: "balanced",
              header: "Balanced",
              cell: (info) => (info.getValue<boolean>() ? "yes" : "no"),
            },
          ]}
          maxHeightClass="compact-table-wrap"
        />
      </Show>
    </div>
  );
}

export function AnnotationsPanel(props: {
  dash: Dash;
  window: WindowAcc;
  onJumpCycles: (start: number, end: number) => void;
}) {
  const bookmarks = createMemo(() =>
    [...props.dash().annotations.bookmarks.bookmarks]
      .filter((bookmark) => {
        if (!props.window().active) return true;
        if (bookmark.first_cycle != null && bookmark.last_cycle != null) {
          return intervalOverlapsWindow(
            bookmark.first_cycle,
            bookmark.last_cycle,
            props.window(),
          );
        }
        return cycleInWindow(bookmark.first_cycle, props.window());
      })
      .sort((a, b) => b.count - a.count),
  );
  const regions = createMemo(() =>
    [...props.dash().annotations.regions.regions].sort(
      (a, b) => b.total_cycles - a.total_cycles,
    ),
  );
  const logs = createMemo(() =>
    props.dash().logging.top_messages.filter((message) => {
      if (!props.window().active) return true;
      if (message.first_cycle != null && message.last_cycle != null) {
        return intervalOverlapsWindow(
          message.first_cycle,
          message.last_cycle,
          props.window(),
        );
      }
      return cycleInWindow(message.first_cycle, props.window());
    }),
  );

  return (
    <div class="panel-stack">
      <div class="stat-grid">
        <StatCard
          label="Bookmarks"
          value={formatCompact(props.dash().annotations.bookmarks.events)}
          hint={`${props.dash().annotations.bookmarks.specs} specs`}
        />
        <StatCard
          label="Regions"
          value={formatCompact(props.dash().annotations.regions.completed)}
          hint={`${props.dash().annotations.regions.unterminated} open`}
        />
        <StatCard
          label="Log messages"
          value={formatCompact(props.dash().logging.messages)}
          hint={`${props.dash().logging.categories} categories`}
        />
        <StatCard
          label="Metadata stack"
          value={String(props.dash().metadata_stack.restored_stack_count)}
          hint={`${props.dash().metadata_stack.unmatched_restore_count} unmatched restores`}
        />
      </div>

      <Show when={bookmarks().length > 0}>
        <SortableTable
          eyebrow="Bookmarks"
          title="Bookmark points"
          subtitle="Click a bookmark with cycle bounds to jump the CPU timeline"
          data={bookmarks()}
          columns={[
            {
              id: "label",
              header: "Bookmark",
              accessorFn: (row) => row.sample_message ?? row.format_string,
            },
            { accessorKey: "count", header: "Count" },
            {
              accessorKey: "first_cycle",
              header: "First",
              cell: (info) => formatCompact(info.getValue<number | undefined>() ?? NaN),
            },
            {
              accessorKey: "callstack_count",
              header: "Stacks",
            },
            {
              id: "loc",
              header: "Location",
              accessorFn: (row) =>
                row.file ? `${row.file}:${row.line ?? "?"}` : "—",
            },
          ]}
          initialSort={[{ id: "count", desc: true }]}
          filterFn={(row, query) =>
            (row.sample_message ?? row.format_string).toLowerCase().includes(query)
          }
          onRowClick={(row) => {
            if (row.first_cycle != null && row.last_cycle != null) {
              props.onJumpCycles(row.first_cycle, row.last_cycle);
            }
          }}
          maxHeightClass="frame-browser-wrap"
        />
      </Show>

      <Show when={regions().length > 0}>
        <SortableTable
          eyebrow="Regions"
          title="Named regions"
          data={regions()}
          columns={[
            { accessorKey: "name", header: "Region" },
            {
              accessorKey: "category",
              header: "Category",
              cell: (info) => info.getValue<string | undefined>() ?? "—",
            },
            { accessorKey: "count", header: "Count" },
            {
              accessorKey: "total_cycles",
              header: "Total cy",
              cell: (info) => formatCompact(info.getValue<number>()),
            },
          ]}
          initialSort={[{ id: "total_cycles", desc: true }]}
          filterFn={(row, query) => row.name.toLowerCase().includes(query)}
          maxHeightClass="frame-browser-wrap"
        />
      </Show>

      <Show when={logs().length > 0}>
        <SortableTable
          eyebrow="Logging"
          title="Hottest log points"
          data={logs()}
          columns={[
            {
              accessorKey: "category",
              header: "Category",
              cell: (info) => info.getValue<string | undefined>() ?? "—",
            },
            { accessorKey: "verbosity", header: "Level" },
            {
              id: "message",
              header: "Message",
              accessorFn: (row) => row.sample_message ?? row.format_string,
            },
            { accessorKey: "count", header: "Count" },
          ]}
          initialSort={[{ id: "count", desc: true }]}
          filterFn={(row, query) =>
            (row.sample_message ?? row.format_string).toLowerCase().includes(query) ||
            (row.category ?? "").toLowerCase().includes(query)
          }
          maxHeightClass="frame-browser-wrap"
        />
      </Show>
    </div>
  );
}

export function CapturePanel(props: {
  dash: Dash;
  inventory: Accessor<UtraceInventory | null>;
}) {
  const channels = createMemo(() => props.dash().channels.channels);
  const unmodeled = createMemo(() =>
    [...props.dash().unmodeled.events].sort(
      (a, b) => b.observed_count - a.observed_count,
    ),
  );
  const inventoryEvents = createMemo(() =>
    [...(props.inventory()?.inventory.events ?? [])].sort(
      (a, b) => b.observed_count - a.observed_count,
    ),
  );
  const decodeCoverage = createMemo(() => {
    const summary = props.inventory()?.inventory.summary;
    if (!summary) return [];
    return [
      { name: "decoded", value: summary.decoded_event_types },
      { name: "partial", value: summary.partial_event_types },
      { name: "raw", value: summary.raw_event_types },
    ].filter((row) => row.value > 0);
  });

  return (
    <div class="panel-stack">
      <div class="stat-grid">
        <StatCard
          label="Channels"
          value={`${props.dash().channels.enabled}/${props.dash().channels.count}`}
          hint={`${props.dash().channels.toggles} toggles`}
        />
        <StatCard
          label="Unmodeled types"
          value={String(props.dash().unmodeled.event_types)}
          hint={`${formatCompact(props.dash().unmodeled.observed_events)} events`}
        />
        <StatCard
          label="Declared events"
          value={String(props.inventory()?.inventory.summary.declared_event_types ?? "—")}
        />
        <StatCard
          label="Observed events"
          value={formatCompact(
            props.inventory()?.inventory.summary.observed_events ?? 0,
          )}
        />
      </div>

      <div class="chart-grid">
        <DonutChart title="Decode coverage" data={decodeCoverage()} />
        <HorizontalBars
          title="Hottest observed events"
          data={inventoryEvents()
            .slice(0, 12)
            .map((event) => ({
              name: `${event.logger}.${event.event}`,
              value: event.observed_count,
            }))}
          valueLabel="count"
        />
      </div>

      <Show when={channels().length > 0}>
        <SortableTable
          eyebrow="Channels"
          title="Trace channels"
          data={channels()}
          columns={[
            {
              id: "label",
              header: "Channel",
              accessorFn: (row) => row.name ?? `channel ${row.id}`,
            },
            {
              accessorKey: "is_enabled",
              header: "Enabled",
              cell: (info) => (info.getValue<boolean>() ? "yes" : "no"),
            },
            {
              accessorKey: "read_only",
              header: "RO",
              cell: (info) => (info.getValue<boolean>() ? "yes" : ""),
            },
            { accessorKey: "toggle_count", header: "Toggles" },
          ]}
          filterFn={(row, query) =>
            (row.name ?? "").toLowerCase().includes(query) ||
            String(row.id).includes(query)
          }
          maxHeightClass="compact-table-wrap"
        />
      </Show>

      <Show when={unmodeled().length > 0}>
        <SortableTable
          eyebrow="Unmodeled"
          title="Declared families without dedicated provider semantics"
          data={unmodeled()}
          columns={[
            {
              id: "name",
              header: "Logger.Event",
              accessorFn: (row) => `${row.logger}.${row.event}`,
            },
            {
              accessorKey: "observed_count",
              header: "Count",
              cell: (info) => formatCompact(info.getValue<number>()),
            },
          ]}
          initialSort={[{ id: "observed_count", desc: true }]}
          filterFn={(row, query) =>
            `${row.logger}.${row.event}`.toLowerCase().includes(query)
          }
          maxHeightClass="frame-browser-wrap"
        />
      </Show>

      <SortableTable
        eyebrow="Inventory"
        title="Declared event inventory"
        data={inventoryEvents()}
        columns={[
          {
            id: "name",
            header: "Logger.Event",
            accessorFn: (row) => `${row.logger}.${row.event}`,
          },
          { accessorKey: "decode_status", header: "Status" },
          {
            accessorKey: "observed_count",
            header: "Count",
            cell: (info) => formatCompact(info.getValue<number>()),
          },
        ]}
        initialSort={[{ id: "observed_count", desc: true }]}
        filterFn={(row, query) =>
          `${row.logger}.${row.event}`.toLowerCase().includes(query)
        }
        maxHeightClass="frame-browser-wrap"
      />
    </div>
  );
}
