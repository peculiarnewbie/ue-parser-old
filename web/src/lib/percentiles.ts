export const FULL_PERCENTILES = [
  "avg",
  "p50",
  "p75",
  "p90",
  "p99",
  "p99.9",
  "max",
] as const;

export const SIMPLE_PERCENTILES = ["avg", "p50", "p99", "max"] as const;

export type PercentileKey = (typeof FULL_PERCENTILES)[number];

export type Percentiles = Record<PercentileKey, number>;

/** Same index rule as tools/www CSV perf: `floor(p * n)` on ascending sort. */
export function computePercentiles(values: number[]): Percentiles | null {
  if (values.length === 0) return null;
  const sorted = [...values].sort((a, b) => a - b);
  const at = (p: number) => {
    const index = Math.min(sorted.length - 1, Math.floor(p * sorted.length));
    return sorted[index] ?? 0;
  };
  const sum = values.reduce((acc, value) => acc + value, 0);
  return {
    avg: sum / values.length,
    p50: at(0.5),
    p75: at(0.75),
    p90: at(0.9),
    p99: at(0.99),
    "p99.9": at(0.999),
    max: sorted[sorted.length - 1] ?? 0,
  };
}

export type FrameMetricId =
  | "cpu_ms"
  | "gpu_ms"
  | "gpu_breadcrumb_ms"
  | "gpu_work_cycles"
  | "gpu_breadcrumb_cycles";

export type FrameMetricRow = {
  id: FrameMetricId;
  name: string;
  unit: "ms" | "cycles";
  budget: number;
  samples: number;
  percentiles: Percentiles;
};

const DEFAULT_BUDGETS_MS: Partial<Record<FrameMetricId, number>> = {
  cpu_ms: 33.33,
  gpu_ms: 33.33,
  gpu_breadcrumb_ms: 33.33,
};

export function budgetFor(metric: FrameMetricId): number {
  return DEFAULT_BUDGETS_MS[metric] ?? 0;
}

export function budgetClass(value: number, budget: number): string {
  if (!(budget > 0) || !Number.isFinite(value)) return "";
  const diff = (value - budget) / budget;
  if (diff > 0.3) return "budget-bad";
  if (diff > 0.05) return "budget-warn";
  if (diff < -0.3) return "budget-good";
  if (diff < -0.05) return "budget-ok";
  return "";
}
