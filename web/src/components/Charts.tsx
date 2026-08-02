import {
  Axis,
  AxisCrosshair,
  AxisGrid,
  AxisLabel,
  AxisMark,
  AxisTooltip,
  Bar,
  Brush,
  type BrushProps,
  type BrushRange,
  Chart,
  Legend,
  Line,
  Pie,
} from "peculiar-charts";
import { For, Show, createMemo, createSignal, type JSX } from "solid-js";
import { cyclesToMs } from "../lib/analysis-range";
import { downsampleMinMax } from "../lib/chart-downsampling";
import {
  brushForFrameSelection,
  frameSelectionFromBrush,
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
  const data = () => props.data.slice(0, 12);
  return (
    <ChartFrame title={props.title} subtitle={props.subtitle} hasData={data().length > 0}>
      <Chart data={data()} barConfig={{ bandGap: "18%", barGap: "8%" }}>
        <Axis axis="y" position="left" dataKey="name" type="band" tickCount={data().length}>
          <AxisLabel class="tick-label" format={(value) => truncate(String(value), 28)} />
        </Axis>
        <Axis axis="x" position="bottom" type="linear">
          <AxisGrid class="grid-line" />
          <AxisMark class="tick-mark" />
          <AxisLabel class="tick-label" />
          <AxisCrosshair class="crosshair" />
          <AxisTooltip>
            {(payload) => (
              <div class="pc-tooltip">
                <div class="pc-tooltip-title">{String(payload.label ?? "")}</div>
                <For each={payload.series}>
                  {(item) => (
                    <div class="pc-tooltip-row">
                      <span>{props.valueLabel ?? item.name}</span>
                      <strong>{formatNumber(Number(item.value))}</strong>
                    </div>
                  )}
                </For>
              </div>
            )}
          </AxisTooltip>
        </Axis>
        <Bar dataKey="value" class="series-amber-fill" layout="horizontal" rx={2} />
      </Chart>
    </ChartFrame>
  );
}

export function LineSeriesChart(props: {
  title: string;
  subtitle?: string;
  data: Record<string, string | number>[];
  xKey: string;
  series: { key: string; class: string; name: string }[];
  height?: number;
}) {
  return (
    <ChartFrame
      title={props.title}
      subtitle={props.subtitle}
      hasData={props.data.length > 0}
      height={props.height ?? 280}
    >
      <Chart data={props.data}>
        <Legend class="pc-legend" />
        <Axis axis="y" position="left" type="linear">
          <AxisGrid class="grid-line" />
          <AxisMark class="tick-mark" />
          <AxisLabel class="tick-label" format={(v) => formatCompact(Number(v))} />
        </Axis>
        <Axis axis="x" position="bottom" dataKey={props.xKey} type="point" tickCount={8}>
          <AxisMark class="tick-mark" />
          <AxisLabel class="tick-label" />
          <AxisCrosshair class="crosshair" />
          <AxisTooltip>
            {(payload) => (
              <div class="pc-tooltip">
                <div class="pc-tooltip-title">{String(payload.label ?? "")}</div>
                <For each={payload.series}>
                  {(item) => (
                    <div class="pc-tooltip-row">
                      <span>{item.name}</span>
                      <strong>{formatNumber(Number(item.value))}</strong>
                    </div>
                  )}
                </For>
              </div>
            )}
          </AxisTooltip>
        </Axis>
        <For each={props.series}>
          {(series) => (
            <Line
              dataKey={series.key}
              name={series.name}
              class={series.class}
              stroke-width={2}
            />
          )}
        </For>
      </Chart>
    </ChartFrame>
  );
}

export function DonutChart(props: {
  title: string;
  subtitle?: string;
  data: NamedValue[];
}) {
  return (
    <ChartFrame title={props.title} subtitle={props.subtitle} hasData={props.data.length > 0}>
      <Chart data={props.data}>
        <Legend class="pc-legend" />
        <Pie dataKey="value" nameKey="name" innerRadius="55%" padAngle={0.04} />
      </Chart>
    </ChartFrame>
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

  const series = createMemo(() => {
    switch (metric()) {
      case "frame_ms":
        return [
          { key: "frame_ms", name: "Frame marker ms", class: "series-amber", yAxisId: "y" },
        ];
      case "gpu_submitted_work_ms":
        return [
          {
            key: "gpu_submitted_work_ms",
            name: "GPU submitted work ms",
            class: "series-steel",
            yAxisId: "y",
          },
        ];
      case "gpu_submitted_work_cycles":
        return [
          {
            key: "gpu_submitted_work",
            name: "GPU submitted work cycles",
            class: "series-steel",
            yAxisId: "y",
          },
        ];
      case "frame_gpu_ms":
      default:
        return [
          { key: "frame_ms", name: "Frame marker ms", class: "series-amber", yAxisId: "y" },
          {
            key: canConvertGpu() ? "gpu_submitted_work_ms" : "gpu_submitted_work",
            name: canConvertGpu()
              ? "GPU submitted work ms"
              : "GPU submitted work cycles",
            class: "series-steel",
            yAxisId: canConvertGpu() ? "y" : "gpu",
          },
        ];
    }
  });

  const dualAxis = createMemo(
    () => metric() === "frame_gpu_ms" && !canConvertGpu(),
  );

  const subtitle = createMemo(() => {
    if (metric() === "gpu_submitted_work_ms" || metric() === "gpu_submitted_work_cycles") {
      return "GPU submitted work is the sum of overlapping GPU intervals whose CPU submit time fell in the marker — not Insights GPU frame time.";
    }
    if (metric() === "frame_gpu_ms" && canConvertGpu()) {
      return "Frame marker ms is BeginFrame→EndFrame. GPU submitted work is a sum of scopes, not GPU frame duration.";
    }
    if (dualAxis()) {
      return "Drag the navigator to zoom this chart. GPU uses a second axis because this capture has no cycle frequency.";
    }
    return "BeginFrame→EndFrame marker duration (Insights Frames track). Drag the navigator to zoom; it does not change the CPU query range.";
  });

  // peculiar-charts BrushProps intersects SVG <g> onChange; narrow to the brush callbacks.
  const brushHandlers = {
    onChange: (range: BrushRange) =>
      props.onSelectionChange?.(
        frameSelectionFromBrush({ frameNumbers: frameNumbers(), brush: range }),
      ),
    onDragEnd: (range: BrushRange) =>
      props.onSelectionCommit?.(
        frameSelectionFromBrush({ frameNumbers: frameNumbers(), brush: range }),
      ),
  } as Pick<BrushProps, "onChange" | "onDragEnd">;

  return (
    <ChartFrame
      title="Frame-marker timing"
      subtitle={subtitle()}
      hasData={points().length > 0}
      height={props.height ?? 340}
      actions={
        <div class="chart-metric-controls">
          <label class="chart-metric-select">
            <span>Y metric</span>
            <select
              value={metric()}
              onChange={(event) =>
                setMetric(event.currentTarget.value as FrameYMetric)
              }
            >
              <For each={METRIC_OPTIONS}>
                {(option) => <option value={option.id}>{option.label}</option>}
              </For>
            </select>
          </label>
          <Show when={props.selection}>
            <button
              type="button"
              class="btn ghost compact"
              onClick={() => props.onSelectionClear?.()}
            >
              Clear brush
            </button>
          </Show>
        </div>
      }
    >
      <Chart data={points()} class="frame-cost-chart">
        <Legend class="pc-legend" />
        <Axis axis="y" axisId="y" position="left" type="linear">
          <AxisGrid class="grid-line" />
          <AxisMark class="tick-mark" />
          <AxisLabel class="tick-label" format={(v) => formatCompact(Number(v))} />
        </Axis>
        <Show when={dualAxis()}>
          <Axis axis="y" axisId="gpu" position="right" type="linear">
            <AxisMark class="tick-mark" />
            <AxisLabel class="tick-label" format={(v) => formatCompact(Number(v))} />
          </Axis>
        </Show>
        <Axis axis="x" position="bottom" dataKey="frame" type="point" tickCount={10}>
          <AxisMark class="tick-mark" />
          <AxisLabel class="tick-label" />
          <AxisCrosshair class="crosshair" />
          <AxisTooltip>
            {(payload) => (
              <div class="pc-tooltip">
                <div class="pc-tooltip-title">Marker {String(payload.label ?? "")}</div>
                <For each={payload.series}>
                  {(item) => (
                    <div class="pc-tooltip-row">
                      <span>{item.name}</span>
                      <strong>{formatNumber(Number(item.value))}</strong>
                    </div>
                  )}
                </For>
              </div>
            )}
          </AxisTooltip>
        </Axis>
        <For each={series()}>
          {(item) => (
            <Line
              dataKey={item.key}
              name={item.name}
              class={item.class}
              yAxisId={item.yAxisId}
              stroke-width={2}
            />
          )}
        </For>
        <Brush
          class="pc-brush"
          height={44}
          gap={8}
          handleWidth={6}
          startIndex={brush()?.startIndex}
          endIndex={brush()?.endIndex}
          {...brushHandlers}
        >
          <Line
            dataKey={series()[0]?.key ?? "frame_ms"}
            class="series-amber"
            stroke-width={1}
          />
        </Brush>
      </Chart>
    </ChartFrame>
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

function truncate(value: string, max: number): string {
  return value.length > max ? `${value.slice(0, max - 1)}…` : value;
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
