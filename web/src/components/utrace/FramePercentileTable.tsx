import { For, Show, createMemo, createSignal } from "solid-js";
import { cyclesToMs } from "../../lib/analysis-range";
import {
  FULL_PERCENTILES,
  SIMPLE_PERCENTILES,
  type FrameMetricId,
  type FrameMetricRow,
  type PercentileKey,
  budgetClass,
  budgetFor,
  computePercentiles,
} from "../../lib/percentiles";
import { formatNumber } from "../../lib/format";
import type { CorrelatedFrameSummary } from "../../lib/types";

export function buildFrameMetricRows(
  frames: CorrelatedFrameSummary[],
  cycleFrequency: number | undefined,
): FrameMetricRow[] {
  if (frames.length === 0) return [];

  const cpuMs = frames
    .map((frame) => (frame.cpu_metadata_seconds ?? 0) * 1000)
    .filter((value) => value > 0);
  const canMs = cycleFrequency != null && cycleFrequency > 0;
  const gpuMs = canMs
    ? frames
        .map((frame) => cyclesToMs(frame.gpu_work_cycles, cycleFrequency) ?? 0)
        .filter((value) => value > 0)
    : [];
  const crumbMs = canMs
    ? frames
        .map(
          (frame) =>
            cyclesToMs(frame.gpu_breadcrumb_cycles, cycleFrequency) ?? 0,
        )
        .filter((value) => value > 0)
    : [];
  const gpuCycles = frames
    .map((frame) => frame.gpu_work_cycles)
    .filter((value) => value > 0);
  const crumbCycles = frames
    .map((frame) => frame.gpu_breadcrumb_cycles)
    .filter((value) => value > 0);

  const rows: FrameMetricRow[] = [];
  const push = (
    id: FrameMetricId,
    name: string,
    unit: "ms" | "cycles",
    values: number[],
  ) => {
    const percentiles = computePercentiles(values);
    if (!percentiles) return;
    rows.push({
      id,
      name,
      unit,
      budget: budgetFor(id),
      samples: values.length,
      percentiles,
    });
  };

  push("cpu_ms", "Correlated CPU time", "ms", cpuMs);
  if (canMs) {
    push("gpu_ms", "GPU work (sum of scopes)", "ms", gpuMs);
    push("gpu_breadcrumb_ms", "GPU breadcrumbs (sum of scopes)", "ms", crumbMs);
  } else {
    push("gpu_work_cycles", "GPU work (sum of scopes)", "cycles", gpuCycles);
    push(
      "gpu_breadcrumb_cycles",
      "GPU breadcrumbs (sum of scopes)",
      "cycles",
      crumbCycles,
    );
  }
  return rows;
}

export function FramePercentileTable(props: {
  frames: CorrelatedFrameSummary[];
  cycleFrequency?: number;
  selectedMetric?: FrameMetricId | null;
  onSelectMetric?: (metric: FrameMetricId) => void;
}) {
  const [full, setFull] = createSignal(false);
  const [sortKey, setSortKey] = createSignal<PercentileKey>("p99");
  const [sortDir, setSortDir] = createSignal<"asc" | "desc">("desc");

  const rows = createMemo(() =>
    buildFrameMetricRows(props.frames, props.cycleFrequency),
  );

  const columns = createMemo(() =>
    full() ? [...FULL_PERCENTILES] : [...SIMPLE_PERCENTILES],
  );

  const sorted = createMemo(() => {
    const key = sortKey();
    const dir = sortDir() === "asc" ? 1 : -1;
    return [...rows()].sort(
      (a, b) => (a.percentiles[key] - b.percentiles[key]) * dir,
    );
  });

  const toggleSort = (key: PercentileKey) => {
    if (sortKey() === key) {
      setSortDir((prev) => (prev === "asc" ? "desc" : "asc"));
      return;
    }
    setSortKey(key);
    setSortDir("desc");
  };

  const subtitle = createMemo(() => {
    const count = props.frames.length;
    const chartHint = props.onSelectMetric != null
      ? " · click a row to set the chart Y metric"
      : "";
    return `${count} correlated CPU frames${chartHint}`;
  });

  return (
    <section class="panel percentile-panel">
      <header class="datatable-head">
        <div>
          <p class="eyebrow">Summary</p>
          <h2>Correlated CPU-frame percentiles</h2>
          <p class="muted datatable-meta">{subtitle()}</p>
        </div>
        <div class="panel-head-actions">
          <div class="toggle-group" role="group" aria-label="Percentile depth">
            <button
              type="button"
              class="toggle-btn"
              classList={{ active: !full() }}
              onClick={() => setFull(false)}
            >
              Normal
            </button>
            <button
              type="button"
              class="toggle-btn"
              classList={{ active: full() }}
              onClick={() => setFull(true)}
            >
              Full
            </button>
          </div>
        </div>
      </header>

      <Show
        when={sorted().length > 0}
        fallback={
          <p class="chart-empty">No correlated CPU-frame costs in this window.</p>
        }
      >
        <div class="percentile-table-wrap">
          <table class="percentile-table">
            <thead>
              <tr>
                <th scope="col">Metric</th>
                <th scope="col">n</th>
                <For each={columns()}>
                  {(key) => (
                    <th scope="col">
                      <button
                        type="button"
                        class="th-sort"
                        classList={{ active: sortKey() === key }}
                        onClick={() => toggleSort(key)}
                      >
                        {key}
                        <Show when={sortKey() === key}>
                          <span aria-hidden="true">
                            {sortDir() === "asc" ? " ↑" : " ↓"}
                          </span>
                        </Show>
                      </button>
                    </th>
                  )}
                </For>
                <th scope="col">Budget</th>
              </tr>
            </thead>
            <tbody>
              <For each={sorted()}>
                {(row) => (
                  <tr
                    classList={{
                      clickable: props.onSelectMetric != null,
                      selected: props.selectedMetric === row.id,
                    }}
                    onClick={() => props.onSelectMetric?.(row.id)}
                  >
                    <th scope="row">
                      {row.name}
                      <span class="muted unit"> {row.unit}</span>
                    </th>
                    <td>{row.samples}</td>
                    <For each={columns()}>
                      {(key) => (
                        <td
                          class={budgetClass(
                            row.percentiles[key],
                            row.budget,
                          )}
                        >
                          {formatNumber(row.percentiles[key], row.unit === "ms" ? 2 : 1)}
                        </td>
                      )}
                    </For>
                    <td class="muted">
                      {row.budget > 0 ? formatNumber(row.budget, 2) : "—"}
                    </td>
                  </tr>
                )}
              </For>
            </tbody>
          </table>
        </div>
      </Show>
    </section>
  );
}

export function metricIdToFrameYMetric(
  id: FrameMetricId,
):
  | "cpu_ms"
  | "gpu_ms"
  | "gpu_breadcrumb_ms"
  | "gpu_work_cycles"
  | "gpu_breadcrumb_cycles"
  | "cpu_gpu_ms" {
  return id;
}
