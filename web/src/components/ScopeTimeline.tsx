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
  cycleFrequency?: number;
  truncated?: boolean;
  intervals: TimelineLaneInterval[];
  empty?: string;
};

type TimerTrack = {
  id: string;
  lane: string;
  depth: number;
  intervals: TimelineLaneInterval[];
};

const MAX_VISIBLE_DEPTH = 7;
const WIDTH = 1200;
const LEFT = 214;
const RIGHT = 22;
const OVERVIEW_TOP = 14;
const OVERVIEW_HEIGHT = 24;
const RULER_TOP = 56;
const TRACK_TOP = 82;
const TRACK_HEIGHT = 24;
const TRACK_GAP = 3;
const BOTTOM = 30;

function colorFor(label: string): string {
  let hash = 5381;
  for (let i = 0; i < label.length; i += 1) {
    hash = (hash * 33) ^ label.charCodeAt(i);
  }
  const hue = Math.abs(hash) % 360;
  return `hsl(${hue} 60% 51%)`;
}

function truncate(value: string, max: number): string {
  return value.length > max ? `${value.slice(0, max - 1)}…` : value;
}

function plural(count: number, noun: string): string {
  return `${count} ${noun}${count === 1 ? "" : "s"}`;
}

function buildTracks(intervals: TimelineLaneInterval[]): TimerTrack[] {
  const byLane = new Map<string, TimelineLaneInterval[]>();
  for (const interval of intervals) {
    const lane = byLane.get(interval.lane);
    if (lane) lane.push(interval);
    else byLane.set(interval.lane, [interval]);
  }

  const tracks: TimerTrack[] = [];
  for (const [lane, laneIntervals] of byLane) {
    const rows = new Map<number, TimelineLaneInterval[]>();
    let active: TimelineLaneInterval[] = [];
    const ordered = [...laneIntervals].sort(
      (left, right) => left.start - right.start || right.end - left.end,
    );

    for (const interval of ordered) {
      active = active.filter((open) => open.end > interval.start);
      const depth = Math.min(active.length, MAX_VISIBLE_DEPTH - 1);
      const row = rows.get(depth);
      if (row) row.push(interval);
      else rows.set(depth, [interval]);
      active.push(interval);
    }

    for (const [depth, row] of rows) {
      tracks.push({ id: `${lane}-${depth}`, lane, depth, intervals: row });
    }
  }
  return tracks;
}

function tickValues(min: number, max: number): number[] {
  const span = Math.max(1, max - min);
  const target = span / 7;
  const power = 10 ** Math.floor(Math.log10(target));
  const scaled = target / power;
  const base = scaled <= 1 ? 1 : scaled <= 2 ? 2 : scaled <= 5 ? 5 : 10;
  const step = base * power;
  const first = Math.ceil(min / step) * step;
  const ticks: number[] = [];
  for (let value = first; value < max && ticks.length < 10; value += step) {
    ticks.push(value);
  }
  return ticks;
}

export function ScopeTimeline(props: ScopeTimelineProps) {
  const [hover, setHover] = createSignal<TimelineLaneInterval | null>(null);
  const [zoom, setZoom] = createSignal<[number, number] | null>(null);
  const [brush, setBrush] = createSignal<[number, number] | null>(null);
  let svgRef: SVGSVGElement | undefined;

  const tracks = createMemo(() => buildTracks(props.intervals));
  const laneCount = createMemo(() => new Set(props.intervals.map((item) => item.lane)).size);
  const domain = createMemo(() => {
    const full: [number, number] = [props.begin, Math.max(props.end, props.begin + 1)];
    return zoom() ?? full;
  });
  const height = createMemo(
    () => TRACK_TOP + Math.max(1, tracks().length) * (TRACK_HEIGHT + TRACK_GAP) + BOTTOM,
  );
  const plotWidth = WIDTH - LEFT - RIGHT;
  const ticks = createMemo(() => tickValues(domain()[0], domain()[1]));
  const visible = createMemo(() => {
    const [min, max] = domain();
    return new Set(
      props.intervals
        .filter((interval) => interval.end >= min && interval.start <= max)
        .map((interval) => interval.id),
    );
  });

  const x = (value: number) => {
    const [min, max] = domain();
    return LEFT + ((value - min) / (max - min)) * plotWidth;
  };

  const overviewX = (value: number) =>
    LEFT + ((value - props.begin) / Math.max(1, props.end - props.begin)) * plotWidth;

  const barBounds = (interval: TimelineLaneInterval): [number, number] => {
    const left = Math.max(LEFT, x(interval.start));
    const right = Math.min(LEFT + plotWidth, x(interval.end));
    return [left, Math.max(left + 1, right)];
  };

  const cycleAtClientX = (clientX: number): number | null => {
    if (!svgRef) return null;
    const rect = svgRef.getBoundingClientRect();
    const svgX = ((clientX - rect.left) / rect.width) * WIDTH;
    if (svgX < LEFT || svgX > LEFT + plotWidth) return null;
    const [min, max] = domain();
    return min + ((svgX - LEFT) / plotWidth) * (max - min);
  };

  const formatOffset = (value: number) => {
    const relative = value - props.begin;
    if (props.cycleFrequency != null && props.cycleFrequency > 0) {
      const milliseconds = (relative / props.cycleFrequency) * 1000;
      return milliseconds >= 1000
        ? `+${(milliseconds / 1000).toFixed(3)} s`
        : `+${milliseconds.toFixed(milliseconds < 10 ? 3 : 2)} ms`;
    }
    return `+${formatCompact(relative)} cy`;
  };

  const resetZoom = () => setZoom(null);

  const zoomTo = (interval: TimelineLaneInterval) => {
    const duration = Math.max(1, interval.end - interval.start);
    const pad = duration * 0.65;
    setZoom([
      Math.max(props.begin, interval.start - pad),
      Math.min(props.end, interval.end + pad),
    ]);
  };

  const onPointerDown = (event: PointerEvent) => {
    if (event.button !== 0) return;
    const cycle = cycleAtClientX(event.clientX);
    if (cycle == null) return;
    (event.currentTarget as Element).setPointerCapture?.(event.pointerId);
    setBrush([cycle, cycle]);
  };

  const onPointerMove = (event: PointerEvent) => {
    const current = brush();
    if (!current) return;
    const cycle = cycleAtClientX(event.clientX);
    if (cycle != null) setBrush([current[0], cycle]);
  };

  const onPointerUp = (event: PointerEvent) => {
    const current = brush();
    setBrush(null);
    if (!current) return;
    const start = Math.min(current[0], current[1]);
    const end = Math.max(current[0], current[1]);
    const [min, max] = domain();
    if (end - start >= (max - min) * 0.01) {
      setZoom([Math.max(props.begin, start), Math.min(props.end, end)]);
    }
    (event.currentTarget as Element).releasePointerCapture?.(event.pointerId);
  };

  const onWheel = (event: WheelEvent) => {
    if (!event.ctrlKey && !event.metaKey) return;
    event.preventDefault();
    const center = cycleAtClientX(event.clientX);
    if (center == null) return;
    const [min, max] = domain();
    const span = max - min;
    const factor = event.deltaY > 0 ? 1.25 : 0.8;
    const nextSpan = Math.max(1, Math.min(props.end - props.begin, span * factor));
    const ratio = (center - min) / span;
    let nextMin = center - nextSpan * ratio;
    let nextMax = center + nextSpan * (1 - ratio);
    if (nextMin < props.begin) {
      nextMax += props.begin - nextMin;
      nextMin = props.begin;
    }
    if (nextMax > props.end) {
      nextMin -= nextMax - props.end;
      nextMax = props.end;
    }
    setZoom([Math.max(props.begin, nextMin), Math.min(props.end, nextMax)]);
  };

  return (
    <section class="panel timeline-panel timer-panel">
      <header class="timer-head">
        <div>
          <p class="eyebrow">Timer workbench</p>
          <h2>{props.title}</h2>
          <Show when={props.subtitle}>
            <p class="muted datatable-meta">{props.subtitle}</p>
          </Show>
        </div>
        <div class="timer-readout" aria-label="Timer query summary">
          <span>{plural(laneCount(), "thread")}</span>
          <span>{plural(props.intervals.length, "span")}</span>
          <span>{formatOffset(domain()[1])}</span>
        </div>
      </header>

      <div class="timer-toolbar">
        <span class="timer-toolbar-label">Shared time ruler</span>
        <span class="muted">drag to zoom · Ctrl/⌘ + wheel to scale · click a span to focus</span>
        <div class="timer-actions">
          <Show when={props.truncated}>
            <span class="pill status-partial">query capped</span>
          </Show>
          <Show when={zoom()}>
            <button type="button" class="btn ghost compact" onClick={resetZoom}>
              Fit window
            </button>
          </Show>
        </div>
      </div>

      <Show
        when={props.intervals.length > 0}
        fallback={<p class="chart-empty">{props.empty ?? "No intervals."}</p>}
      >
        <div class="timeline-wrap timer-wrap">
          <svg
            ref={svgRef}
            class="timeline-svg timer-svg"
            viewBox={`0 0 ${WIDTH} ${height()}`}
            role="img"
            aria-label={props.title}
            onPointerDown={onPointerDown}
            onPointerMove={onPointerMove}
            onPointerUp={onPointerUp}
            onWheel={onWheel}
          >
            <rect
              x={LEFT}
              y={OVERVIEW_TOP}
              width={plotWidth}
              height={OVERVIEW_HEIGHT}
              class="timer-overview-bg"
            />
            <For each={props.intervals}>
              {(interval, index) => {
                const left = overviewX(interval.start);
                const right = overviewX(interval.end);
                return (
                  <rect
                    x={Math.max(LEFT, left)}
                    y={OVERVIEW_TOP + 4 + (index() % 3) * 5}
                    width={Math.max(1, Math.min(LEFT + plotWidth, right) - Math.max(LEFT, left))}
                    height={3}
                    fill={colorFor(interval.label)}
                    opacity={0.65}
                  />
                );
              }}
            </For>
            <rect
              x={overviewX(domain()[0])}
              y={OVERVIEW_TOP}
              width={Math.max(2, overviewX(domain()[1]) - overviewX(domain()[0]))}
              height={OVERVIEW_HEIGHT}
              class="timer-overview-window"
            />

            <text x={LEFT - 12} y={OVERVIEW_TOP + 15} text-anchor="end" class="timer-ruler-label">
              window
            </text>
            <text x={LEFT - 12} y={RULER_TOP + 13} text-anchor="end" class="timer-ruler-label">
              time
            </text>

            <For each={ticks()}>
              {(tick) => (
                <g>
                  <line
                    x1={x(tick)}
                    x2={x(tick)}
                    y1={RULER_TOP}
                    y2={height() - BOTTOM}
                    class="timer-gridline"
                  />
                  <text x={x(tick)} y={RULER_TOP + 13} text-anchor="middle" class="timer-axis-label">
                    {formatOffset(tick)}
                  </text>
                </g>
              )}
            </For>
            <line
              x1={LEFT}
              x2={LEFT + plotWidth}
              y1={RULER_TOP + 19}
              y2={RULER_TOP + 19}
              class="timer-axis"
            />

            <For each={tracks()}>
              {(track, trackIndex) => {
                const y = TRACK_TOP + trackIndex() * (TRACK_HEIGHT + TRACK_GAP);
                const laneChanged = () =>
                  trackIndex() === 0 || tracks()[trackIndex() - 1]?.lane !== track.lane;
                return (
                  <g>
                    <rect
                      x={LEFT}
                      y={y}
                      width={plotWidth}
                      height={TRACK_HEIGHT}
                      class="timer-track-bg"
                      classList={{ "timer-track-start": laneChanged() }}
                    />
                    <text
                      x={LEFT - 12}
                      y={y + TRACK_HEIGHT / 2}
                      text-anchor="end"
                      dominant-baseline="middle"
                      class="timer-lane-label"
                      classList={{ "timer-lane-child": track.depth > 0 }}
                    >
                      {track.depth === 0 ? truncate(track.lane, 28) : `↳ depth ${track.depth + 1}`}
                    </text>
                    <For each={track.intervals}>
                      {(interval) => {
                        if (!visible().has(interval.id)) return null;
                        const [left, right] = barBounds(interval);
                        const width = right - left;
                        return (
                          <g>
                            <rect
                              x={left}
                              y={y + 3}
                              width={width}
                              height={TRACK_HEIGHT - 6}
                              rx={2}
                              fill={colorFor(interval.label)}
                              class="timer-bar"
                              classList={{ active: hover()?.id === interval.id }}
                              onMouseEnter={() => setHover(interval)}
                              onMouseLeave={() => setHover(null)}
                              onClick={(event) => {
                                event.stopPropagation();
                                zoomTo(interval);
                              }}
                            >
                              <title>{`${interval.label} · ${interval.durationLabel}`}</title>
                            </rect>
                            <Show when={width > 98}>
                              <text
                                x={left + 6}
                                y={y + TRACK_HEIGHT / 2}
                                dominant-baseline="middle"
                                class="timer-bar-label"
                              >
                                {truncate(interval.label, Math.max(8, Math.floor(width / 7)))}
                              </text>
                            </Show>
                          </g>
                        );
                      }}
                    </For>
                  </g>
                );
              }}
            </For>

            <Show when={brush()}>
              {(range) => {
                const start = Math.min(range()[0], range()[1]);
                const end = Math.max(range()[0], range()[1]);
                return (
                  <rect
                    x={x(start)}
                    y={RULER_TOP}
                    width={Math.max(1, x(end) - x(start))}
                    height={height() - RULER_TOP - BOTTOM}
                    class="timeline-brush"
                  />
                );
              }}
            </Show>
            <text x={LEFT} y={height() - 8} class="timer-axis-label">{formatOffset(domain()[0])}</text>
            <text x={LEFT + plotWidth} y={height() - 8} text-anchor="end" class="timer-axis-label">
              {formatOffset(domain()[1])}
            </text>
          </svg>
        </div>

        <Show when={hover()}>
          {(interval) => (
            <div class="timeline-tooltip timer-tooltip" aria-live="polite">
              <strong>{interval().label}</strong>
              <span>{interval().lane}</span>
              <span>{formatOffset(interval().start)} → {formatOffset(interval().end)}</span>
              <span class="timer-duration">{interval().durationLabel}</span>
              <span class="muted">click to isolate</span>
            </div>
          )}
        </Show>
      </Show>
    </section>
  );
}

function formatCompact(value: number): string {
  return new Intl.NumberFormat("en-US", {
    notation: "compact",
    maximumFractionDigits: 2,
  }).format(value);
}
