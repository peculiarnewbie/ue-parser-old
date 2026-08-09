import { barX, colorLegend, defineChart, lineY } from "@tanstack/charts";
import { tooltip } from "@tanstack/charts/tooltip";
import { Chart } from "@tanstack/solid-charts";
import { max } from "d3-array";
import { scaleBand, scaleLinear, scaleOrdinal, scalePoint } from "d3-scale";
import { For, Show, createMemo, createSignal, type JSX } from "solid-js";
import { cyclesToMs } from "../lib/analysis-range";
import { downsampleMinMax } from "../lib/chart-downsampling";
import {
  brushForFrameSelection,
  frameSelectionFromBrush,
  type BrushRange,
  type FrameSelection,
} from "../lib/frame-selection";
import type { FrameTimingSummary } from "../lib/types";

type ChartFrameProps = {
  title: string;
  subtitle?: string;
  empty?: string;
  height?: number;
  children: JSX.Element;
  hasData: boolean;
  actions?: JSX.Element;
};

const CHART_COLORS = ["#e1aa4e", "#8aaec4", "#7dbf8b", "#e07060", "#b59ad8"];

export function ChartFrame(props: ChartFrameProps) {
  return (
    <section class="chart-frame">
      <header class="chart-frame-head">
        <div class="chart-frame-titles">
          <h3>{props.title}</h3>
          <Show when={props.subtitle}>
            <p>{props.subtitle}</p>
          </Show>
        </div>
        <Show when={props.actions}>
          <div class="chart-frame-actions">{props.actions}</div>
        </Show>
      </header>
      <Show
        when={props.hasData}
        fallback={<p class="chart-empty">{props.empty ?? "No data for this chart."}</p>}
      >
        <div class="chart-canvas" style={{ height: `${props.height ?? 260}px` }}>
          {props.children}
        </div>
      </Show>
    </section>
  );
}

type NamedValue = { name: string; value: number };

export function HorizontalBars(props: {
  title: string;
  subtitle?: string;
  data: NamedValue[];
  valueLabel?: string;
}) {
  const data = createMemo(() => props.data.slice(0, 12));
  const definition = createMemo(() => {
    const rows = data();
    const maximum = max(rows, (row) => row.value) ?? 1;
    return defineChart(
      {
        marks: [
          barX(rows, {
            x: "value",
            y: "name",
            key: "name",
            fill: CHART_COLORS[0],
            radius: 3,
            inset: 2,
          }),
        ],
        x: {
          scale: scaleLinear().domain([0, maximum]).nice(),
          grid: true,
          axis: { ticks: { count: 5, format: formatCompact } },
        },
        y: {
          scale: scaleBand<string>()
            .domain(rows.map((row) => row.name))
            .paddingInner(0.18)
            .paddingOuter(0.08),
          reverse: true,
          axis: {
            ticks: { format: (value) => truncate(String(value), 28) },
            tickLabels: { thin: false },
          },
        },
      },
      {
        tooltip: {
          use: tooltip,
          format: (point) =>
            `${props.valueLabel ?? "value"}: ${formatNumber(Number(point.xValue))}`,
        },
      },
    );
  });

  return (
    <ChartFrame title={props.title} subtitle={props.subtitle} hasData={data().length > 0}>
      <Chart
        definition={definition()}
        height={260}
        ariaLabel={`${props.title}. Horizontal comparison of ${data().length} values.`}
      />
    </ChartFrame>
  );
}

type LineDatum = {
  x: string;
  series: string;
  value: number;
  key: string;
};

export function LineSeriesChart(props: {
  title: string;
  subtitle?: string;
  data: Record<string, string | number>[];
  xKey: string;
  series: { key: string; name: string }[];
  height?: number;
}) {
  const rows = createMemo((): LineDatum[] =>
    props.data.flatMap((datum, datumIndex) =>
      props.series.flatMap((series) => {
        const value = Number(datum[series.key]);
        if (!Number.isFinite(value)) return [];
        const x = String(datum[props.xKey] ?? datumIndex);
        return [{ x, series: series.name, value, key: `${series.key}:${x}:${datumIndex}` }];
      }),
    ),
  );
  const definition = createMemo(() => {
    const chartRows = rows();
    const labels = [...new Set(chartRows.map((row) => row.x))];
    const seriesNames = props.series.map((series) => series.name);
    return defineChart(
      {
        marks: [
          lineY(chartRows, {
            x: "x",
            y: "value",
            z: "series",
            color: "series",
            key: "key",
            strokeWidth: 2,
            points: chartRows.length < 160,
          }),
        ],
        x: {
          scale: scalePoint<string>().domain(labels).padding(0.25),
          axis: { ticks: { count: 8 }, tickLabels: { thin: { priority: "ends" } } },
        },
        y: {
          scale: scaleLinear()
            .domain([0, max(chartRows, (row) => row.value) ?? 1])
            .nice(),
          grid: true,
          axis: { ticks: { count: 5, format: formatCompact } },
        },
        color: {
          scale: scaleOrdinal(seriesNames, CHART_COLORS.slice(0, seriesNames.length)),
          legend: colorLegend({ label: "Series", placement: "top" }),
        },
      },
      {
        tooltip: {
          use: tooltip,
          format: (point) => formatNumber(Number(point.yValue)),
        },
      },
    );
  });

  return (
    <ChartFrame
      title={props.title}
      subtitle={props.subtitle}
      hasData={rows().length > 0}
      height={props.height ?? 280}
    >
      <Chart
        definition={definition()}
        height={props.height ?? 280}
        ariaLabel={`${props.title}. ${props.series.length} time series.`}
      />
    </ChartFrame>
  );
}

/** Ranked bars make trace-provider composition easier to compare than angles. */
export function CompositionBars(props: {
  title: string;
  subtitle?: string;
  data: NamedValue[];
}) {
  return (
    <HorizontalBars
      title={props.title}
      subtitle={props.subtitle}
      data={[...props.data].sort((left, right) => right.value - left.value)}
      valueLabel="count"
    />
  );
}

export type FrameYMetric =
  | "frame_gpu_ms"
  | "frame_ms"
  | "gpu_submitted_work_ms"
  | "gpu_submitted_work_cycles";

export type FramePoint = {
  frame: string;
  frame_number: number;
  frame_ms: number;
  gpu_submitted_work_ms: number;
  gpu_submitted_work: number;
  begin_cycle: number;
  end_cycle: number;
};

type FrameSeriesDatum = {
  index: number;
  frame: string;
  frame_number: number;
  series: string;
  value: number;
  key: string;
};

type FrameSeries = {
  key: "frame_ms" | "gpu_submitted_work_ms" | "gpu_submitted_work";
  name: string;
  color: string;
};

const METRIC_OPTIONS: { id: FrameYMetric; label: string }[] = [
  { id: "frame_ms", label: "Frame marker ms" },
  { id: "frame_gpu_ms", label: "Frame + GPU submitted work ms" },
  { id: "gpu_submitted_work_ms", label: "GPU submitted work ms (sum, not GPU frame time)" },
  { id: "gpu_submitted_work_cycles", label: "GPU submitted work cycles (sum)" },
];

export function buildFramePoints(
  frames: FrameTimingSummary[],
  cycleFrequency: number | undefined,
  frameLabel?: (frame: FrameTimingSummary) => string,
): FramePoint[] {
  return frames.map((frame) => {
    const frame_ms =
      frame.duration_seconds != null
        ? frame.duration_seconds * 1000
        : cyclesToMs(frame.duration_cycles, cycleFrequency) ?? 0;
    const gpu_submitted_work_ms =
      cyclesToMs(frame.gpu_submitted_work_cycles, cycleFrequency) ?? 0;
    return {
      frame: frameLabel?.(frame) ?? String(frame.frame_number),
      frame_number: frame.frame_number,
      frame_ms,
      gpu_submitted_work_ms,
      gpu_submitted_work: frame.gpu_submitted_work_cycles,
      begin_cycle: frame.begin_cycle,
      end_cycle: frame.end_cycle,
    };
  });
}

export function FrameCostBrushChart(props: {
  frames: FrameTimingSummary[];
  cycleFrequency?: number;
  selection?: FrameSelection | null;
  onSelectionChange?: (selection: FrameSelection | null) => void;
  onSelectionCommit?: (selection: FrameSelection | null) => void;
  onSelectionClear?: () => void;
  height?: number;
  metric?: FrameYMetric;
  onMetricChange?: (metric: FrameYMetric) => void;
  frameLabel?: (frame: FrameTimingSummary) => string;
  renderPointBudget?: number;
}) {
  const [localMetric, setLocalMetric] = createSignal<FrameYMetric>("frame_ms");
  const metric = () => props.metric ?? localMetric();
  const setMetric = (next: FrameYMetric) => {
    if (props.metric == null) setLocalMetric(next);
    props.onMetricChange?.(next);
  };
  const canConvertGpu = () =>
    props.cycleFrequency != null && props.cycleFrequency > 0;
  const renderedFrames = createMemo(() => {
    if (!props.renderPointBudget) return props.frames;
    return downsampleMinMax({
      values: props.frames,
      maxPoints: props.renderPointBudget,
      metrics: (frame) => frameMetricValues(frame, metric(), props.cycleFrequency),
    });
  });
  const points = createMemo(() =>
    buildFramePoints(renderedFrames(), props.cycleFrequency, props.frameLabel),
  );
  const frameNumbers = createMemo(() => points().map((point) => point.frame_number));
  const brush = createMemo(() =>
    brushForFrameSelection({
      frameNumbers: frameNumbers(),
      selection: props.selection,
    }),
  );

  const series = createMemo((): FrameSeries[] => {
    switch (metric()) {
      case "frame_ms":
        return [{ key: "frame_ms", name: "Frame marker ms", color: CHART_COLORS[0] }];
      case "gpu_submitted_work_ms":
        return [
          {
            key: "gpu_submitted_work_ms",
            name: "GPU submitted work ms",
            color: CHART_COLORS[1],
          },
        ];
      case "gpu_submitted_work_cycles":
        return [
          {
            key: "gpu_submitted_work",
            name: "GPU submitted work cycles",
            color: CHART_COLORS[1],
          },
        ];
      case "frame_gpu_ms":
      default:
        return [
          { key: "frame_ms", name: "Frame marker ms", color: CHART_COLORS[0] },
          {
            key: canConvertGpu() ? "gpu_submitted_work_ms" : "gpu_submitted_work",
            name: canConvertGpu()
              ? "GPU submitted work ms"
              : "GPU submitted work cycles",
            color: CHART_COLORS[1],
          },
        ];
    }
  });
  const dualUnits = createMemo(() => metric() === "frame_gpu_ms" && !canConvertGpu());
  const subtitle = createMemo(() => {
    if (metric() === "gpu_submitted_work_ms" || metric() === "gpu_submitted_work_cycles") {
      return "GPU submitted work is the sum of overlapping GPU intervals whose CPU submit time fell in the marker — not Insights GPU frame time.";
    }
    if (metric() === "frame_gpu_ms" && canConvertGpu()) {
      return "Frame marker ms is BeginFrame→EndFrame. GPU submitted work is a sum of scopes, not GPU frame duration.";
    }
    if (dualUnits()) {
      return "The capture has no cycle frequency, so milliseconds and GPU cycles are shown as aligned small multiples instead of a misleading dual axis.";
    }
    return "BeginFrame→EndFrame marker duration (Insights Frames track). Use the range controls to focus this chart without changing the CPU query range.";
  });

  const rowsFor = (selectedSeries: readonly FrameSeries[]) =>
    points().flatMap((point, index) =>
      selectedSeries.map((item) => ({
        index,
        frame: point.frame,
        frame_number: point.frame_number,
        series: item.name,
        value: point[item.key],
        key: `${item.key}:${point.frame_number}:${index}`,
      })),
    );

  const primarySeries = createMemo(() => (dualUnits() ? series().slice(0, 1) : series()));
  const primaryRows = createMemo(() => rowsFor(primarySeries()));
  const primaryDefinition = createMemo(() =>
    createFrameDefinition(primaryRows(), primarySeries(), points()),
  );
  const secondarySeries = createMemo(() => (dualUnits() ? series().slice(1) : []));
  const secondaryRows = createMemo(() => rowsFor(secondarySeries()));
  const secondaryDefinition = createMemo(() =>
    createFrameDefinition(secondaryRows(), secondarySeries(), points()),
  );

  const changeRange = (edge: "start" | "end", nextIndex: number, commit: boolean) => {
    const current = brush();
    if (!current) return;
    const next: BrushRange =
      edge === "start"
        ? { startIndex: Math.min(nextIndex, current.endIndex), endIndex: current.endIndex }
        : { startIndex: current.startIndex, endIndex: Math.max(nextIndex, current.startIndex) };
    const selection = frameSelectionFromBrush({ frameNumbers: frameNumbers(), brush: next });
    if (commit) props.onSelectionCommit?.(selection);
    else props.onSelectionChange?.(selection);
  };

  return (
    <ChartFrame
      title="Frame-marker timing"
      subtitle={subtitle()}
      hasData={points().length > 0}
      height={props.height ?? 340}
      actions={
        <label class="chart-metric-select">
          <span>Y metric</span>
          <select
            value={metric()}
            onChange={(event) => setMetric(event.currentTarget.value as FrameYMetric)}
          >
            <For each={METRIC_OPTIONS}>
              {(option) => <option value={option.id}>{option.label}</option>}
            </For>
          </select>
        </label>
      }
    >
      <div class="frame-chart-stack">
        <Chart
          definition={primaryDefinition()}
          height={dualUnits() ? 190 : props.height ?? 300}
          ariaLabel="Frame marker timing across the capture"
        />
        <Show when={dualUnits()}>
          <Chart
            definition={secondaryDefinition()}
            height={150}
            ariaLabel="GPU submitted work cycles across the capture"
          />
        </Show>
        <Show when={brush()} keyed>
          {(range) => (
            <div class="chart-range-editor" aria-label="Visible frame range">
              <label>
                <span>Range start</span>
                <input
                  type="range"
                  min={0}
                  max={Math.max(0, frameNumbers().length - 1)}
                  value={range.startIndex}
                  onInput={(event) =>
                    changeRange("start", Number(event.currentTarget.value), false)
                  }
                  onChange={(event) =>
                    changeRange("start", Number(event.currentTarget.value), true)
                  }
                />
                <output>{points()[range.startIndex]?.frame ?? "—"}</output>
              </label>
              <label>
                <span>Range end</span>
                <input
                  type="range"
                  min={0}
                  max={Math.max(0, frameNumbers().length - 1)}
                  value={range.endIndex}
                  onInput={(event) =>
                    changeRange("end", Number(event.currentTarget.value), false)
                  }
                  onChange={(event) =>
                    changeRange("end", Number(event.currentTarget.value), true)
                  }
                />
                <output>{points()[range.endIndex]?.frame ?? "—"}</output>
              </label>
              <button
                type="button"
                class="btn ghost compact"
                disabled={!props.selection}
                onClick={() => props.onSelectionClear?.()}
              >
                Full capture
              </button>
            </div>
          )}
        </Show>
      </div>
    </ChartFrame>
  );
}

function createFrameDefinition(
  rows: FrameSeriesDatum[],
  series: readonly FrameSeries[],
  points: readonly FramePoint[],
) {
  const upperIndex = Math.max(1, points.length - 1);
  const labels = points.map((point) => point.frame);
  const names = series.map((item) => item.name);
  return defineChart(
    {
      marks: [
        lineY(rows, {
          x: "index",
          y: "value",
          z: "series",
          color: "series",
          key: "key",
          strokeWidth: 2,
          points: rows.length < 160,
        }),
      ],
      x: {
        scale: scaleLinear().domain([0, upperIndex]),
        axis: {
          ticks: {
            count: 8,
            format: (value) => labels[Math.round(Number(value))] ?? "",
          },
          tickLabels: { thin: { priority: "ends" } },
        },
      },
      y: {
        scale: scaleLinear()
          .domain([0, max(rows, (row) => row.value) ?? 1])
          .nice(),
        grid: true,
        axis: { ticks: { count: 5, format: formatCompact } },
      },
      color: {
        scale: scaleOrdinal(
          names,
          series.map((item) => item.color),
        ),
        legend: colorLegend({ placement: "top" }),
      },
    },
    {
      tooltip: {
        use: tooltip,
        format: (point) => `${formatNumber(Number(point.yValue))}`,
        formatGroup: (focused) => focused[0]?.datum.frame ?? "Frame",
      },
    },
  );
}

function frameMetricValues(
  frame: FrameTimingSummary,
  metric: FrameYMetric,
  cycleFrequency: number | undefined,
): number[] {
  const frameMs =
    frame.duration_seconds != null
      ? frame.duration_seconds * 1000
      : cyclesToMs(frame.duration_cycles, cycleFrequency) ?? 0;
  const gpuSubmittedWorkMs =
    cyclesToMs(frame.gpu_submitted_work_cycles, cycleFrequency) ?? 0;
  switch (metric) {
    case "frame_ms":
      return [frameMs];
    case "gpu_submitted_work_ms":
      return [gpuSubmittedWorkMs];
    case "gpu_submitted_work_cycles":
      return [frame.gpu_submitted_work_cycles];
    case "frame_gpu_ms":
      return [
        frameMs,
        cycleFrequency != null && cycleFrequency > 0
          ? gpuSubmittedWorkMs
          : frame.gpu_submitted_work_cycles,
      ];
  }
}

function truncate(value: string, maximum: number): string {
  return value.length > maximum ? `${value.slice(0, maximum - 1)}…` : value;
}

function formatNumber(value: number): string {
  if (!Number.isFinite(value)) return "—";
  return new Intl.NumberFormat("en-US", { maximumFractionDigits: 3 }).format(value);
}

function formatCompact(value: number): string {
  if (!Number.isFinite(value)) return "—";
  return new Intl.NumberFormat("en-US", {
    notation: "compact",
    maximumFractionDigits: 1,
  }).format(value);
}
