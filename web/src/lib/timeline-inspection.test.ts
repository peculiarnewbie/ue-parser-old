import { describe, expect, it } from "vitest";
import {
  inspectTimelineIntervals,
  type TimelineLaneInterval,
} from "./timeline-inspection";

describe("inspectTimelineIntervals", () => {
  it("derives nesting and self time from direct child coverage", () => {
    const inspections = inspectTimelineIntervals([
      interval({ id: "root", label: "Tick", start: 0, end: 100 }),
      interval({ id: "child-a", label: "A", start: 10, end: 40 }),
      interval({ id: "grandchild", label: "A.1", start: 20, end: 30 }),
      interval({ id: "child-b", label: "B", start: 35, end: 80 }),
    ]);

    expect(inspections.get("root")).toMatchObject({
      depth: 0,
      childIds: ["child-a", "child-b"],
      selfDuration: 30,
    });
    expect(inspections.get("child-a")).toMatchObject({
      depth: 1,
      parentId: "root",
      childIds: ["grandchild"],
      selfDuration: 20,
    });
    expect(inspections.get("grandchild")).toMatchObject({
      depth: 2,
      parentId: "child-a",
      selfDuration: 10,
    });
  });

  it("groups same rendered timer occurrences across threads", () => {
    const inspections = inspectTimelineIntervals([
      interval({ id: "later", label: "Task", start: 40, end: 50, specId: 7 }),
      interval({ id: "first", label: "Task", start: 10, end: 20, specId: 7 }),
      interval({
        id: "other-label",
        label: "Task 42",
        start: 5,
        end: 8,
        specId: 7,
      }),
    ]);

    expect(inspections.get("first")).toMatchObject({
      occurrenceIds: ["first", "later"],
      occurrenceIndex: 0,
    });
    expect(inspections.get("later")).toMatchObject({
      occurrenceIds: ["first", "later"],
      occurrenceIndex: 1,
    });
    expect(inspections.get("other-label")?.occurrenceIds).toEqual([
      "other-label",
    ]);
  });
});

function interval(
  input: Pick<TimelineLaneInterval, "id" | "label" | "start" | "end"> &
    Partial<TimelineLaneInterval>,
): TimelineLaneInterval {
  return {
    lane: "GameThread",
    durationLabel: `${input.end - input.start} cy`,
    ...input,
  };
}
