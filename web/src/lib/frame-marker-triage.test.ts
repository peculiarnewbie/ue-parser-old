import { describe, expect, it } from "vitest";
import { triageFrameMarkers } from "./frame-marker-triage";
import type { FrameTimingSummary } from "./types";

function marker(
  frameNumber: number,
  durationSeconds: number,
  gpuSubmittedWorkCycles = 0,
): FrameTimingSummary {
  return {
    frame_number: frameNumber,
    frame_type: 0,
    begin_cycle: frameNumber * 100_000,
    end_cycle: frameNumber * 100_000 + Math.round(durationSeconds * 10_000_000),
    duration_cycles: Math.round(durationSeconds * 10_000_000),
    duration_seconds: durationSeconds,
    gpu_submitted_work_count: 0,
    gpu_submitted_work_cycles: gpuSubmittedWorkCycles,
  };
}

describe("frame-marker triage", () => {
  it("chooses the worst plotted marker, not an unrelated correlated CPU frame", () => {
    // Reproduction values from basic-cpu-frame.utrace: marker 660 is 5.2504 ms,
    // while marker 1879 is the 21.9207 ms chart spike.
    const result = triageFrameMarkers({
      frames: [marker(660, 0.0052504), marker(1879, 0.0219207)],
      cycleFrequency: 10_000_000,
      budgetMs: 16.67,
    });

    expect(result.worst?.frame.frame_number).toBe(1879);
    expect(result.worst?.frameMs).toBeCloseTo(21.9207, 4);
    expect(result.overBudget.map((entry) => entry.frame.frame_number)).toEqual([1879]);
  });

  it("does not call submitted GPU work a frame-duration hitch", () => {
    const result = triageFrameMarkers({
      frames: [
        marker(22, 0.005, 3_000_000),
        marker(23, 0.015),
      ],
      cycleFrequency: 10_000_000,
      budgetMs: 16.67,
    });

    expect(result.worst?.frame.frame_number).toBe(23);
    expect(result.overBudget).toEqual([]);
  });
});
