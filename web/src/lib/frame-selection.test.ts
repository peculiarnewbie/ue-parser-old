import { describe, expect, it } from "vitest";
import {
  brushForFrameSelection,
  frameSelectionFromBrush,
} from "./frame-selection";
import {
  analysisWindowFromFrameSelection,
  analysisWindowFromSelection,
  frameSelectionForAnalysisSelection,
} from "./analysis-range";
import type { CorrelatedFrameSummary } from "./types";

function frame(frameNumber: number): CorrelatedFrameSummary {
  return {
    frame_number: frameNumber,
    cpu_metadata_count: 1,
    cpu_metadata_cycles: 1,
    cpu_begin_cycle: frameNumber * 100,
    cpu_end_cycle: frameNumber * 100 + 99,
    gpu_queue_count: 0,
    gpu_work_count: 0,
    gpu_work_cycles: 0,
    gpu_breadcrumb_count: 0,
    gpu_breadcrumb_cycles: 0,
  };
}

describe("frame chart selection", () => {
  it("converts a rendered-point brush into stable capture frame numbers", () => {
    expect(
      frameSelectionFromBrush({
        frameNumbers: [100, 125, 175, 250],
        brush: { startIndex: 1, endIndex: 2 },
      }),
    ).toEqual({ startFrame: 125, endFrame: 175 });
  });

  it("treats a full-range brush as no selection", () => {
    expect(
      frameSelectionFromBrush({
        frameNumbers: [100, 125, 175, 250],
        brush: { startIndex: 0, endIndex: 3 },
      }),
    ).toBeNull();
  });

  it("pins an empty selection to the full rendered range", () => {
    expect(
      brushForFrameSelection({
        frameNumbers: [100, 125, 175, 250],
        selection: null,
      }),
    ).toEqual({ startIndex: 0, endIndex: 3 });
  });

  it("restores a stable frame selection after the chart is downsampled", () => {
    expect(
      brushForFrameSelection({
        frameNumbers: [100, 125, 175, 250],
        selection: { startFrame: 120, endFrame: 200 },
      }),
    ).toEqual({ startIndex: 1, endIndex: 2 });
  });

  it("filters the bounded detail dataset by frame number, not overview indexes", () => {
    const window = analysisWindowFromFrameSelection(
      [frame(900), frame(950), frame(1_000)],
      { startFrame: 100, endFrame: 960 },
    );

    expect(window.frames.map((item) => item.frame_number)).toEqual([900, 950]);
    expect(window.startFrame).toBe(100);
    expect(window.endFrame).toBe(960);
    expect(window.active).toBe(true);
  });

  it("uses an exact cycle selection while including every overlapping frame", () => {
    const window = analysisWindowFromSelection(
      [frame(10), frame(11), frame(12)],
      { kind: "cycles", startCycle: 1_050, endCycle: 1_225 },
    );

    expect(window.frames.map((item) => item.frame_number)).toEqual([10, 11, 12]);
    expect(window.startCycle).toBe(1_050);
    expect(window.endCycle).toBe(1_225);
    expect(window.selection).toEqual({
      kind: "cycles",
      startCycle: 1_050,
      endCycle: 1_225,
    });
  });

  it("maps an exact cycle selection back onto the shared chart brush", () => {
    expect(
      frameSelectionForAnalysisSelection(
        [
          { frame_number: 20, begin_cycle: 2_000, end_cycle: 2_099 },
          { frame_number: 21, begin_cycle: 2_100, end_cycle: 2_199 },
          { frame_number: 22, begin_cycle: 2_200, end_cycle: 2_299 },
        ],
        { kind: "cycles", startCycle: 2_075, endCycle: 2_225 },
      ),
    ).toEqual({ startFrame: 20, endFrame: 22 });
  });

  it("normalizes a capture-wide cycle selection to an inactive window", () => {
    const window = analysisWindowFromSelection(
      [frame(10), frame(11), frame(12)],
      { kind: "cycles", startCycle: 1_000, endCycle: 1_299 },
    );

    expect(window.active).toBe(false);
    expect(window.selection).toBeNull();
    expect(window.frames).toHaveLength(3);
  });
});
