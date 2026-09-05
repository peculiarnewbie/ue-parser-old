import { computePercentiles } from "../percentiles";
import { filterFramesByType } from "../frame-type";
import type { FrameTimingSummary, UtraceDashboard } from "../types";
import type {
  FrameDistributionComparison,
  FrameDistributionSummary,
} from "./model";

function frameDurationMs(
  frame: FrameTimingSummary,
  cycleFrequency: number | undefined,
): number | null {
  if (frame.duration_seconds != null && Number.isFinite(frame.duration_seconds)) {
    return frame.duration_seconds * 1000;
  }
  if (cycleFrequency == null || !(cycleFrequency > 0)) return null;
  return (frame.duration_cycles / cycleFrequency) * 1000;
}

export function summarizeFrameDistribution(input: {
  dashboard: UtraceDashboard;
  frameType: number;
  budgetMs: number;
}): FrameDistributionSummary | null {
  const body = input.dashboard.dashboard;
  const frameType = input.frameType === 0 ? "game" : "rendering";
  const frames = filterFramesByType(
    body.frame_timing?.frames ?? [],
    frameType,
  );
  const samplesMs = frames.flatMap((frame) => {
    const value = frameDurationMs(frame, body.prologue?.cycle_frequency);
    return value != null && value >= 0 && Number.isFinite(value) ? [value] : [];
  });
  const percentiles = computePercentiles(samplesMs);
  if (!percentiles) return null;
  const budgetMissCount = samplesMs.filter((value) => value > input.budgetMs).length;
  return {
    evidence: (body.frame_timing?.total_frame_count ?? 0) > (body.frame_timing?.frames.length ?? 0) || samplesMs.length < frames.length
      ? "prefix_sampled" : "exact",
    frameType: input.frameType,
    samplesMs,
    count: samplesMs.length,
    averageMs: percentiles.avg,
    p50Ms: percentiles.p50,
    p90Ms: percentiles.p90,
    p99Ms: percentiles.p99,
    p999Ms: percentiles["p99.9"],
    maxMs: percentiles.max,
    budgetMs: input.budgetMs,
    budgetMissCount,
    budgetMissRate: budgetMissCount / samplesMs.length,
  };
}

export function compareFrameDistributions(input: {
  baseline: UtraceDashboard;
  candidate: UtraceDashboard;
  frameType: number;
  budgetMs: number;
  materialityMs?: number;
  materialityRatio?: number;
}): FrameDistributionComparison {
  const baseline = summarizeFrameDistribution({
    dashboard: input.baseline,
    frameType: input.frameType,
    budgetMs: input.budgetMs,
  });
  const candidate = summarizeFrameDistribution({
    dashboard: input.candidate,
    frameType: input.frameType,
    budgetMs: input.budgetMs,
  });
  if (!baseline || !candidate) {
    return {
      baseline,
      candidate,
      verdict: {
        kind: "inconclusive",
        reason: "Both captures need completed frames with a usable timebase.",
      },
    };
  }

  if (baseline.evidence !== "exact" || candidate.evidence !== "exact") {
    return { baseline, candidate, verdict: {
      kind: "inconclusive",
      reason: "At least one frame population is incomplete; retained samples cannot establish a capture-wide verdict.",
    } };
  }
  const p99Delta = candidate.p99Ms - baseline.p99Ms;
  const missDelta = candidate.budgetMissRate - baseline.budgetMissRate;
  const materialityMs = input.materialityMs ?? 0.5;
  const materialityRatio = input.materialityRatio ?? 0.05;
  if (
    p99Delta >= materialityMs &&
    p99Delta >= baseline.p99Ms * materialityRatio
  ) {
    return {
      baseline,
      candidate,
      verdict: {
        kind: "regression",
        reason: `Candidate p99 is ${p99Delta.toFixed(2)} ms slower${missDelta > 0 ? " and misses the frame budget more often" : ""}.`,
      },
    };
  }
  if (
    p99Delta <= -materialityMs &&
    p99Delta <= -baseline.p99Ms * materialityRatio
  ) {
    return {
      baseline,
      candidate,
      verdict: {
        kind: "improvement",
        reason: `Candidate p99 is ${Math.abs(p99Delta).toFixed(2)} ms faster.`,
      },
    };
  }
  return {
    baseline,
    candidate,
    verdict: {
      kind: "stable",
      reason: `The p99 shift is below the current ${materialityMs} ms / ${materialityRatio * 100}% materiality threshold.`,
    },
  };
}

export function quantileProfile(
  samples: readonly number[],
  quantiles: readonly number[],
): number[] {
  if (samples.length === 0) return quantiles.map(() => 0);
  const sorted = [...samples].sort((left, right) => left - right);
  return quantiles.map((quantile) => {
    const index = Math.min(
      sorted.length - 1,
      Math.max(0, Math.floor(quantile * sorted.length)),
    );
    return sorted[index] ?? 0;
  });
}
