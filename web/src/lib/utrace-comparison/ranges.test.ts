import { describe, expect, it } from "vitest";
import type { LoadedComparisonCapture } from "./model";
import { cycleRange } from "./ranges";

const capture = { timelineIndex: { begin_cycle: 100, end_cycle: 200, cycle_frequency: 1_000 } } as LoadedComparisonCapture;

describe("comparison cycle ranges", () => {
  it("converts local milliseconds and retains the exact capture endpoint", () => {
    expect(cycleRange(capture, { startMs: 10, endMs: 100 })).toEqual({ start_cycle: 110, end_cycle: 200 });
  });
  it("rejects empty, out-of-bounds, nonfinite, and sub-cycle ranges", () => {
    for (const range of [{ startMs: 1, endMs: 1 }, { startMs: -1, endMs: 10 },
      { startMs: 0, endMs: 100.1 }, { startMs: 0, endMs: NaN }, { startMs: 0, endMs: 0.1 }]) {
      expect(() => cycleRange(capture, range)).toThrow();
    }
  });
  it("rejects unsafe source coordinates before doing arithmetic", () => {
    const unsafe = { ...capture, timelineIndex: { ...capture.timelineIndex!, begin_cycle: 2 ** 53, end_cycle: 2 ** 53 + 100 } };
    expect(() => cycleRange(unsafe, { startMs: 0, endMs: 10 })).toThrow(/exact integer range/);
  });
});
