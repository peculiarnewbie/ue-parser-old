import type { BrushRange } from "peculiar-charts";

/** A capture-stable selection, independent of whichever points a chart renders. */
export type FrameSelection = {
  startFrame: number;
  endFrame: number;
};

export function frameSelectionFromBrush({
  frameNumbers,
  brush,
}: {
  frameNumbers: readonly number[];
  brush: BrushRange;
}): FrameSelection | null {
  if (frameNumbers.length === 0) return null;
  const startIndex = clampIndex(brush.startIndex, frameNumbers.length);
  const endIndex = Math.max(startIndex, clampIndex(brush.endIndex, frameNumbers.length));
  if (startIndex === 0 && endIndex === frameNumbers.length - 1) return null;
  return {
    startFrame: frameNumbers[startIndex],
    endFrame: frameNumbers[endIndex],
  };
}

export function brushForFrameSelection({
  frameNumbers,
  selection,
}: {
  frameNumbers: readonly number[];
  selection: FrameSelection | null | undefined;
}): BrushRange | undefined {
  if (frameNumbers.length === 0) return undefined;
  // Controlled full-range keeps peculiar-charts' brush pinned as the series
  // grows. Leaving indexes undefined freezes the zoom at the first mount length.
  if (!selection) {
    return { startIndex: 0, endIndex: frameNumbers.length - 1 };
  }
  const startIndex = lowerBound(frameNumbers, selection.startFrame);
  const endIndex = upperBound(frameNumbers, selection.endFrame) - 1;
  if (startIndex >= frameNumbers.length || endIndex < startIndex) return undefined;
  return {
    startIndex,
    endIndex: Math.min(endIndex, frameNumbers.length - 1),
  };
}

function clampIndex(index: number, length: number): number {
  return Math.max(0, Math.min(index, length - 1));
}

function lowerBound(values: readonly number[], target: number): number {
  let low = 0;
  let high = values.length;
  while (low < high) {
    const middle = low + Math.floor((high - low) / 2);
    if (values[middle] < target) low = middle + 1;
    else high = middle;
  }
  return low;
}

function upperBound(values: readonly number[], target: number): number {
  let low = 0;
  let high = values.length;
  while (low < high) {
    const middle = low + Math.floor((high - low) / 2);
    if (values[middle] <= target) low = middle + 1;
    else high = middle;
  }
  return low;
}
