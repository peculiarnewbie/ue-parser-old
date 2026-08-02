import { describe, expect, it } from "vitest";
import {
  TRACE_FRAME_TYPE_GAME,
  TRACE_FRAME_TYPE_RENDERING,
  filterFramesByType,
  frameTypeLabel,
} from "./frame-type";

describe("filterFramesByType", () => {
  const frames = [
    { frame_type: TRACE_FRAME_TYPE_GAME, id: 1 },
    { frame_type: TRACE_FRAME_TYPE_RENDERING, id: 2 },
    { frame_type: TRACE_FRAME_TYPE_GAME, id: 3 },
  ];

  it("defaults Insights comparison to Game only", () => {
    expect(filterFramesByType(frames, "game").map((frame) => frame.id)).toEqual([
      1, 3,
    ]);
  });

  it("can show Rendering or both", () => {
    expect(
      filterFramesByType(frames, "rendering").map((frame) => frame.id),
    ).toEqual([2]);
    expect(filterFramesByType(frames, "all")).toHaveLength(3);
  });
});

describe("frameTypeLabel", () => {
  it("names the Insights frame types", () => {
    expect(frameTypeLabel(0)).toBe("Game");
    expect(frameTypeLabel(1)).toBe("Rendering");
    expect(frameTypeLabel(9)).toBe("Type 9");
  });
});
