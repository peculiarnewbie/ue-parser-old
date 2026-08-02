import type { CorrelatedFrameSummary } from "./types";

export type FrameLabelMode = "relative" | "capture";

export type FrameDisplay = {
  ordinal: number;
  elapsedSeconds?: number;
};

export type FrameDisplayMap = ReadonlyMap<number, FrameDisplay>;

export function buildFrameDisplayMap(input: {
  frames: CorrelatedFrameSummary[];
  cycleFrequency?: number;
}): FrameDisplayMap {
  const ordered = input.frames
    .map((frame, sourceIndex) => ({ frame, sourceIndex }))
    .sort((left, right) => {
      const leftCycle = left.frame.cpu_begin_cycle;
      const rightCycle = right.frame.cpu_begin_cycle;
      if (leftCycle != null && rightCycle != null) return leftCycle - rightCycle;
      if (leftCycle != null) return -1;
      if (rightCycle != null) return 1;
      return left.frame.frame_number - right.frame.frame_number || left.sourceIndex - right.sourceIndex;
    });
  const firstCycle = ordered.find((entry) => entry.frame.cpu_begin_cycle != null)?.frame
    .cpu_begin_cycle;
  const canMeasureTime =
    firstCycle != null && input.cycleFrequency != null && input.cycleFrequency > 0;
  const display = new Map<number, FrameDisplay>();

  for (const [ordinal, entry] of ordered.entries()) {
    const cycle = entry.frame.cpu_begin_cycle;
    display.set(entry.frame.frame_number, {
      ordinal,
      elapsedSeconds:
        canMeasureTime && cycle != null
          ? (cycle - firstCycle) / input.cycleFrequency!
          : undefined,
    });
  }
  return display;
}

export function formatFrameLabel(input: {
  frameNumber: number;
  display: FrameDisplay | undefined;
  mode: FrameLabelMode;
}): string {
  return input.mode === "relative" && input.display != null
    ? String(input.display.ordinal)
    : String(input.frameNumber);
}

export function formatCaptureElapsed(seconds: number | undefined): string {
  if (seconds == null || !Number.isFinite(seconds)) return "—";
  const milliseconds = Math.max(0, Math.round(seconds * 1000));
  const hours = Math.floor(milliseconds / 3_600_000);
  const minutes = Math.floor((milliseconds % 3_600_000) / 60_000);
  const wholeSeconds = Math.floor((milliseconds % 60_000) / 1000);
  const remainder = milliseconds % 1000;
  return [hours, minutes, wholeSeconds]
    .map((part) => String(part).padStart(2, "0"))
    .join(":")
    .concat(`.${String(remainder).padStart(3, "0")}`);
}
