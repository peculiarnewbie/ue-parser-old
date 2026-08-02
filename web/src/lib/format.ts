export function formatNumber(value: number, digits = 3): string {
  if (!Number.isFinite(value)) return "—";
  return new Intl.NumberFormat("en-US", { maximumFractionDigits: digits }).format(value);
}

export function formatCompact(value: number): string {
  if (!Number.isFinite(value)) return "—";
  return new Intl.NumberFormat("en-US", {
    notation: "compact",
    maximumFractionDigits: 1,
  }).format(value);
}

export function formatSeconds(value: number | undefined | null, digits = 4): string {
  if (value == null || !Number.isFinite(value)) return "—";
  return `${value.toFixed(digits)} s`;
}

export function formatMs(seconds: number | undefined | null, digits = 2): string {
  if (seconds == null || !Number.isFinite(seconds)) return "—";
  return `${(seconds * 1000).toFixed(digits)} ms`;
}

export function formatBytes(value: number | undefined | null): string {
  if (value == null || !Number.isFinite(value)) return "—";
  const abs = Math.abs(value);
  if (abs < 1024) return `${value} B`;
  if (abs < 1024 ** 2) return `${(value / 1024).toFixed(1)} KiB`;
  if (abs < 1024 ** 3) return `${(value / 1024 ** 2).toFixed(2)} MiB`;
  return `${(value / 1024 ** 3).toFixed(2)} GiB`;
}

export function formatCycles(value: number | undefined | null): string {
  if (value == null || !Number.isFinite(value)) return "—";
  return `${formatCompact(value)} cy`;
}

/** Prefer wall-clock ms when cycle_frequency is known; otherwise fall back to cycles. */
export function formatGpuCost(
  cycles: number | undefined | null,
  cycleFrequency: number | undefined | null,
  digits = 2,
): string {
  if (cycles == null || !Number.isFinite(cycles)) return "—";
  if (cycleFrequency != null && cycleFrequency > 0) {
    return `${((cycles / cycleFrequency) * 1000).toFixed(digits)} ms`;
  }
  return formatCycles(cycles);
}

export function gpuCostValue(
  cycles: number,
  cycleFrequency: number | undefined | null,
): number {
  if (cycleFrequency != null && cycleFrequency > 0) {
    return (cycles / cycleFrequency) * 1000;
  }
  return cycles;
}

export function gpuCostUnit(
  cycleFrequency: number | undefined | null,
): "ms" | "cycles" {
  return cycleFrequency != null && cycleFrequency > 0 ? "ms" : "cycles";
}

export function truncate(value: string, max: number): string {
  return value.length > max ? `${value.slice(0, max - 1)}…` : value;
}

export function percentile(sortedAscending: number[], p: number): number | null {
  if (sortedAscending.length === 0) return null;
  const index = Math.min(
    sortedAscending.length - 1,
    Math.max(0, Math.ceil((p / 100) * sortedAscending.length) - 1),
  );
  return sortedAscending[index] ?? null;
}
