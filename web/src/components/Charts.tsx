import {
  Axis,
  AxisCrosshair,
  AxisGrid,
  AxisLabel,
  AxisMark,
  AxisTooltip,
  Bar,
  Chart,
  Legend,
  Line,
  Pie,
} from "peculiar-charts";
import { For, Show, type JSX } from "solid-js";

type ChartFrameProps = {
  title: string;
  subtitle?: string;
  empty?: string;
  height?: number;
  children: JSX.Element;
  hasData: boolean;
};

export function ChartFrame(props: ChartFrameProps) {
  return (
    <section class="chart-frame">
      <header class="chart-frame-head">
        <h3>{props.title}</h3>
        <Show when={props.subtitle}>
          <p>{props.subtitle}</p>
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
