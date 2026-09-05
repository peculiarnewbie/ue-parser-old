import type { ComparisonRangeMs, LoadedComparisonCapture } from "./model";

export function captureDurationMs(capture: LoadedComparisonCapture): number {
  const index = capture.timelineIndex;
  if (index?.begin_cycle == null || index.end_cycle == null || !index.cycle_frequency) return 0;
  return ((index.end_cycle - index.begin_cycle) / index.cycle_frequency) * 1_000;
}

export function cycleRange(
  capture: LoadedComparisonCapture,
  range: ComparisonRangeMs,
): { start_cycle: number; end_cycle: number } {
  const index = capture.timelineIndex;
  const frequency = index?.cycle_frequency;
  const begin = index?.begin_cycle;
  const end = index?.end_cycle;
  if (frequency == null || !Number.isFinite(frequency) || frequency <= 0 || begin == null || end == null) {
    throw new Error("This capture has no safe retained CPU timebase for range aggregation.");
  }
  if (!Number.isSafeInteger(begin) || !Number.isSafeInteger(end)) {
    throw new Error("This capture's 64-bit cycle coordinates exceed JavaScript's exact integer range.");
  }
  if (!Number.isFinite(range.startMs) || !Number.isFinite(range.endMs) ||
    range.startMs < 0 || range.endMs <= range.startMs || range.endMs > captureDurationMs(capture)) {
    throw new Error("Each selected range must be non-empty and stay inside its capture.");
  }
  const start_cycle = begin + Math.round((range.startMs / 1_000) * frequency);
  const end_cycle = Math.min(end, begin + Math.round((range.endMs / 1_000) * frequency));
  if (start_cycle >= end_cycle) {
    throw new Error("Each selected range must span at least one CPU cycle.");
  }
  return { start_cycle, end_cycle };
}
