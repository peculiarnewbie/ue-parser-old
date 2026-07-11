import { For, Show, createMemo, createSignal } from "solid-js";

export type TimelineLaneInterval = {
  id: string;
  lane: string;
  label: string;
  start: number;
  end: number;
  durationLabel: string;
};

type ScopeTimelineProps = {
  title: string;
  subtitle?: string;
  begin: number;
  end: number;
  truncated?: boolean;
  intervals: TimelineLaneInterval[];
  empty?: string;
};

const LANE_HEIGHT = 28;
const LANE_GAP = 6;
const LEFT = 160;
const RIGHT = 16;
const TOP = 12;
const BOTTOM = 28;

function colorFor(label: string): string {
  let hash = 0;
  for (let i = 0; i < label.length; i += 1) {
    hash = (hash * 31 + label.charCodeAt(i)) >>> 0;
  }
  const hue = hash % 360;
  return `hsl(${hue} 55% 52%)`;
}

export function ScopeTimeline(props: ScopeTimelineProps) {
  const [hover, setHover] = createSignal<TimelineLaneInterval | null>(null);
  const [zoom, setZoom] = createSignal<[number, number] | null>(null);

  const lanes = createMemo(() => {
    const order: string[] = [];
    const seen = new Set<string>();
    for (const interval of props.intervals) {
      if (!seen.has(interval.lane)) {
        seen.add(interval.lane);
        order.push(interval.lane);
      }
    }
    return order;
  });

  const domain = createMemo(() => {
    const full: [number, number] = [props.begin, Math.max(props.end, props.begin + 1)];
    return zoom() ?? full;
  });

  const width = 960;
  const height = createMemo(
    () => TOP + BOTTOM + lanes().length * (LANE_HEIGHT + LANE_GAP) - LANE_GAP,
  );
  const plotWidth = width - LEFT - RIGHT;

  const x = (value: number) => {
    const [min, max] = domain();
    return LEFT + ((value - min) / (max - min)) * plotWidth;
  };

  const visible = createMemo(() => {
    const [min, max] = domain();
    return props.intervals.filter(
      (interval) => interval.end >= min && interval.start <= max,
    );
  });

  const resetZoom = () => setZoom(null);

  const zoomTo = (interval: TimelineLaneInterval) => {
    const pad = Math.max(1, (interval.end - interval.start) * 0.35);
    setZoom([
      Math.max(props.begin, interval.start - pad),
      Math.min(props.end, interval.end + pad),
    ]);
  };

  return (
    <section class="panel timeline-panel">
      <header class="datatable-head">
        <div>
          <p class="eyebrow">Timeline</p>
          <h2>{props.title}</h2>
          <Show when={props.subtitle}>
            <p class="muted datatable-meta">{props.subtitle}</p>
          </Show>
        </div>
        <div class="timeline-actions">
          <Show when={props.truncated}>
            <span class="pill status-partial">truncated</span>
          </Show>
          <Show when={zoom()}>
            <button type="button" class="btn ghost compact" onClick={resetZoom}>
              Reset zoom
            </button>
          </Show>
        </div>
      </header>

      <Show
        when={props.intervals.length > 0}
        fallback={<p class="chart-empty">{props.empty ?? "No intervals."}</p>}
      >
        <div class="timeline-wrap">
          <svg
            class="timeline-svg"
            viewBox={`0 0 ${width} ${height()}`}
            role="img"
            aria-label={props.title}
          >
            <For each={lanes()}>
              {(lane, index) => {
                const y = TOP + index() * (LANE_HEIGHT + LANE_GAP);
                return (
                  <g>
                    <text
                      x={LEFT - 10}
                      y={y + LANE_HEIGHT / 2}
                      text-anchor="end"
                      dominant-baseline="middle"
                      class="timeline-lane-label"
                    >
                      {truncate(lane, 22)}
                    </text>
                    <rect
                      x={LEFT}
                      y={y}
                      width={plotWidth}
                      height={LANE_HEIGHT}
                      class="timeline-lane-bg"
                    />
                  </g>
                );
              }}
            </For>

            <For each={visible()}>
              {(interval) => {
                const laneIndex = lanes().indexOf(interval.lane);
                if (laneIndex < 0) return null;
                const y = TOP + laneIndex * (LANE_HEIGHT + LANE_GAP) + 3;
                const x0 = x(interval.start);
                const x1 = x(interval.end);
                const w = Math.max(2, x1 - x0);
                return (
                  <rect
                    x={x0}
                    y={y}
                    width={w}
                    height={LANE_HEIGHT - 6}
                    rx={3}
                    fill={colorFor(interval.label)}
                    class="timeline-bar"
                    classList={{ active: hover()?.id === interval.id }}
                    onMouseEnter={() => setHover(interval)}
                    onMouseLeave={() => setHover(null)}
                    onClick={() => zoomTo(interval)}
                  >
                    <title>
                      {interval.label} · {interval.durationLabel}
                    </title>
                  </rect>
                );
              }}
            </For>

            <line
              x1={LEFT}
              x2={LEFT + plotWidth}
              y1={height() - BOTTOM + 4}
              y2={height() - BOTTOM + 4}
              class="timeline-axis"
            />
            <text
              x={LEFT}
              y={height() - 6}
              class="timeline-axis-label"
            >
              {formatDomain(domain()[0])}
            </text>
            <text
              x={LEFT + plotWidth}
              y={height() - 6}
              text-anchor="end"
              class="timeline-axis-label"
            >
              {formatDomain(domain()[1])}
            </text>
          </svg>

          <Show when={hover()}>
            {(interval) => (
              <div class="timeline-tooltip">
                <strong>{interval().label}</strong>
                <span class="muted">{interval().lane}</span>
                <span>{interval().durationLabel}</span>
                <span class="muted">click bar to zoom</span>
              </div>
            )}
          </Show>
        </div>
      </Show>
    </section>
  );
}

function truncate(value: string, max: number): string {
  return value.length > max ? `${value.slice(0, max - 1)}…` : value;
}

function formatDomain(value: number): string {
  return new Intl.NumberFormat("en-US", {
    notation: "compact",
    maximumFractionDigits: 2,
  }).format(value);
}
