import { describe, expect, it, vi } from "vitest";
import { liveFrameFromPatch, mergeLiveFrameTiming } from "./live-frame-timing";
import type { FrameTimingSummary } from "./types";

function frame(
  frameNumber: number,
  overrides: Partial<FrameTimingSummary> = {},
): FrameTimingSummary {
  return {
    frame_number: frameNumber,
    frame_type: 0,
    begin_cycle: frameNumber * 100,
    end_cycle: frameNumber * 200,
    duration_cycles: frameNumber * 100,
    duration_seconds: frameNumber / 1000,
    gpu_submitted_work_count: 0,
    gpu_submitted_work_cycles: 0,
    ...overrides,
  };
}

function legacyMerge(
  current: readonly FrameTimingSummary[],
  incoming: readonly FrameTimingSummary[],
  maxFrames: number,
): FrameTimingSummary[] {
  const byFrame = new Map(current.map((item) => [item.frame_number, item]));
  for (const item of incoming) byFrame.set(item.frame_number, item);
  const merged = [...byFrame.values()].sort(
    (left, right) => left.frame_number - right.frame_number,
  );
  return merged.length > maxFrames ? merged.slice(merged.length - maxFrames) : merged;
}

describe("mergeLiveFrameTiming", () => {
  it("keeps progressive GPU submitted-work totals with the CPU frame", () => {
    const liveFrame = liveFrameFromPatch({
      frame_number: 4,
      frame_type: 0,
      begin_cycle: 400,
      end_cycle: 500,
      duration_cycles: 100,
      gpu_submitted_work_count: 2,
      gpu_submitted_work_cycles: 75,
    });

    expect(liveFrame.gpu_submitted_work_count).toBe(2);
    expect(liveFrame.gpu_submitted_work_cycles).toBe(75);
  });

  it("appends a disjoint, newer frame window", () => {
    const merged = mergeLiveFrameTiming([frame(1), frame(2)], [frame(3), frame(4)], 10);

    expect(merged.map((item) => item.frame_number)).toEqual([1, 2, 3, 4]);
  });

  it("replaces a full overlap while keeping unchanged frame identities", () => {
    const current = [frame(1), frame(2)];
    const merged = mergeLiveFrameTiming(current, [frame(1), frame(2)], 10);

    expect(merged).toEqual(current);
    expect(merged[0]).toBe(current[0]);
    expect(merged[1]).toBe(current[1]);
  });

  it("replaces the overlapping suffix after the server window slides", () => {
    const current = [frame(1), frame(2), frame(3), frame(4)];
    const merged = mergeLiveFrameTiming(current, [frame(3), frame(4), frame(5), frame(6)], 10);

    expect(merged.map((item) => item.frame_number)).toEqual([1, 2, 3, 4, 5, 6]);
    expect(merged[2]).toBe(current[2]);
    expect(merged[3]).toBe(current[3]);
  });

  it("evicts the oldest history at the live chart cap", () => {
    const merged = mergeLiveFrameTiming(
      [frame(1), frame(2), frame(3), frame(4)],
      [frame(5), frame(6), frame(7)],
      5,
    );

    expect(merged.map((item) => item.frame_number)).toEqual([3, 4, 5, 6, 7]);
  });

  it("lets a changed server frame replace its previous value", () => {
    const previous = frame(4);
    const replacement = frame(4, { duration_seconds: 9 });
    const merged = mergeLiveFrameTiming([frame(3), previous], [replacement, frame(5)], 10);

    expect(merged[1]).toBe(replacement);
    expect(merged[1].duration_seconds).toBe(9);
  });

  it("falls back to the legacy merge for an unsorted patch", () => {
    const assertion = vi.spyOn(console, "assert").mockImplementation(() => undefined);
    const current = [frame(1), frame(2)];
    const incoming = [frame(4), frame(3)];

    expect(mergeLiveFrameTiming(current, incoming, 10)).toEqual(
      legacyMerge(current, incoming, 10),
    );
    expect(assertion).toHaveBeenCalled();
    assertion.mockRestore();
  });

  it("matches the legacy accumulation across sliding progressive snapshots", () => {
    let current: FrameTimingSummary[] = [];
    let legacy: FrameTimingSummary[] = [];
    const maxFrames = 200;

    for (let finalFrame = 25; finalFrame <= 1_000; finalFrame += 25) {
      const firstFrame = Math.max(1, finalFrame - 49);
      const snapshot = Array.from(
        { length: finalFrame - firstFrame + 1 },
        (_, index) => frame(firstFrame + index),
      );
      current = mergeLiveFrameTiming(current, snapshot, maxFrames);
      legacy = legacyMerge(legacy, snapshot, maxFrames);
    }

    expect(current).toEqual(legacy);
    expect(current.map((item) => item.frame_number)).toEqual(
      Array.from({ length: maxFrames }, (_, index) => 801 + index),
    );
  });
});
