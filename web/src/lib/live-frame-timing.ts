import type { FrameTimingSummary, UtraceProgressEvent } from "./types";

type FramePatch = Extract<
  Extract<UtraceProgressEvent, { type: "snapshot" }>["patch"],
  { type: "frames" }
>;

/** Converts the compact progressive frame patch into the chart's frame shape. */
export function liveFrameFromPatch(
  frame: FramePatch["frames"][number],
): FrameTimingSummary {
  return {
    frame_number: frame.frame_number,
    frame_type: frame.frame_type,
    begin_cycle: frame.begin_cycle,
    end_cycle: frame.end_cycle,
    duration_cycles: frame.duration_cycles,
    duration_seconds: frame.duration_seconds,
    gpu_submitted_work_count: frame.gpu_submitted_work_count ?? 0,
    gpu_submitted_work_cycles: frame.gpu_submitted_work_cycles ?? 0,
  };
}

/**
 * Merges the server's sorted, sliding frame window into the full live chart
 * history. The normal path replaces only the overlapping suffix and appends
 * the new tail; it never rebuilds a map or sorts the accumulated history.
 */
export function mergeLiveFrameTiming(
  current: readonly FrameTimingSummary[],
  incoming: readonly FrameTimingSummary[],
  maxFrames: number,
): FrameTimingSummary[] {
  if (incoming.length === 0) return capFrames(current, maxFrames);

  if (!isStrictlyAscending(incoming)) {
    if (import.meta.env.DEV) {
      console.assert(false, "Live frame patch was not sorted by frame number");
    }
    return mergeLiveFrameTimingFallback(current, incoming, maxFrames);
  }

  if (current.length === 0) return capFrames(incoming, maxFrames);

  const firstIncoming = incoming[0].frame_number;
  const firstReplaceIndex = lowerBoundByFrameNumber(current, firstIncoming);

  if (firstReplaceIndex === current.length) {
    return capFrames([...current, ...incoming], maxFrames);
  }

  const currentSuffix = current.slice(firstReplaceIndex);
  const lastIncoming = incoming[incoming.length - 1].frame_number;
  const lastCurrent = current[current.length - 1].frame_number;

  // A stale or discontinuous patch cannot safely replace a suffix. Retain the
  // old Map-based behavior for it rather than risking an incorrect chart.
  if (
    currentSuffix[0].frame_number !== firstIncoming ||
    lastIncoming < lastCurrent ||
    !matchesOverlappingSuffix(currentSuffix, incoming)
  ) {
    return mergeLiveFrameTimingFallback(current, incoming, maxFrames);
  }

  let changed = incoming.length !== currentSuffix.length;
  const replacement = incoming.map((next, index) => {
    const previous = currentSuffix[index];
    if (previous && framesHaveSameValues(previous, next)) return previous;
    changed = true;
    return next;
  });

  if (!changed && firstReplaceIndex === 0) return capFrames(current, maxFrames);

  return capFrames([...current.slice(0, firstReplaceIndex), ...replacement], maxFrames);
}

function isStrictlyAscending(frames: readonly FrameTimingSummary[]): boolean {
  for (let index = 1; index < frames.length; index += 1) {
    if (frames[index - 1].frame_number >= frames[index].frame_number) return false;
  }
  return true;
}

function lowerBoundByFrameNumber(
  frames: readonly FrameTimingSummary[],
  frameNumber: number,
): number {
  let low = 0;
  let high = frames.length;
  while (low < high) {
    const middle = low + Math.floor((high - low) / 2);
    if (frames[middle].frame_number < frameNumber) low = middle + 1;
    else high = middle;
  }
  return low;
}

function matchesOverlappingSuffix(
  currentSuffix: readonly FrameTimingSummary[],
  incoming: readonly FrameTimingSummary[],
): boolean {
  const overlapLength = Math.min(currentSuffix.length, incoming.length);
  for (let index = 0; index < overlapLength; index += 1) {
    if (currentSuffix[index].frame_number !== incoming[index].frame_number) {
      return false;
    }
  }
  return true;
}

function mergeLiveFrameTimingFallback(
  current: readonly FrameTimingSummary[],
  incoming: readonly FrameTimingSummary[],
  maxFrames: number,
): FrameTimingSummary[] {
  const byFrame = new Map(current.map((frame) => [frame.frame_number, frame]));
  for (const frame of incoming) byFrame.set(frame.frame_number, frame);
  return capFrames(
    [...byFrame.values()].sort((left, right) => left.frame_number - right.frame_number),
    maxFrames,
  );
}

function capFrames(
  frames: readonly FrameTimingSummary[],
  maxFrames: number,
): FrameTimingSummary[] {
  const cap = Math.max(0, Math.floor(maxFrames));
  return frames.length > cap ? frames.slice(frames.length - cap) : [...frames];
}

function framesHaveSameValues(
  left: FrameTimingSummary,
  right: FrameTimingSummary,
): boolean {
  return (
    left.frame_number === right.frame_number &&
    left.frame_type === right.frame_type &&
    left.begin_cycle === right.begin_cycle &&
    left.end_cycle === right.end_cycle &&
    left.duration_cycles === right.duration_cycles &&
    left.duration_seconds === right.duration_seconds &&
    left.gpu_submitted_work_count === right.gpu_submitted_work_count &&
    left.gpu_submitted_work_cycles === right.gpu_submitted_work_cycles
  );
}
