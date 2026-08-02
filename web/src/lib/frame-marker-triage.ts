import { cyclesToMs } from "./analysis-range";
import type { FrameTimingSummary } from "./types";

export type FrameMarkerCost = {
  frame: FrameTimingSummary;
  frameMs: number;
};

export type FrameMarkerTriage = {
  worst: FrameMarkerCost | null;
  overBudget: FrameMarkerCost[];
  overBudgetPercent: number;
};

/**
 * Performance triage for the same frame-marker timings plotted by the chart.
 * Correlated CPU frames deliberately use a separate model and must not choose
 * this chart's "worst marker". GPU submitted work is intentionally excluded
 * from hitch ranking: it is work submitted during a marker, not GPU frame time.
 */
export function triageFrameMarkers(input: {
  frames: readonly FrameTimingSummary[];
  cycleFrequency?: number;
  budgetMs: number;
}): FrameMarkerTriage {
  const costs = input.frames.map((frame) => {
    const frameMs =
      frame.duration_seconds != null
        ? frame.duration_seconds * 1000
        : cyclesToMs(frame.duration_cycles, input.cycleFrequency) ?? 0;
    return { frame, frameMs };
  });
  const overBudget = costs.filter((cost) => cost.frameMs > input.budgetMs);
  const worst = costs.reduce<FrameMarkerCost | null>(
    (current, cost) =>
      current == null || cost.frameMs > current.frameMs ? cost : current,
    null,
  );

  return {
    worst,
    overBudget,
    overBudgetPercent: costs.length > 0 ? (overBudget.length / costs.length) * 100 : 0,
  };
}
