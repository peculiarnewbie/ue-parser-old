import type { FrameSelection } from "./frame-selection";
import type { CorrelatedFrameSummary, CpuScopeSummary } from "./types";

export type AnalysisSelection =
  | { kind: "frames"; range: FrameSelection }
  | { kind: "cycles"; startCycle: number; endCycle: number };

export type AnalysisWindow = {
  /** Frames included in the current selection (or all frames when unset). */
  frames: CorrelatedFrameSummary[];
  startFrame: number | null;
  endFrame: number | null;
  startCycle: number | null;
  endCycle: number | null;
  /** True when the selection narrowed the capture. */
  active: boolean;
  selection: AnalysisSelection | null;
};

export function analysisWindowFromFrameSelection(
  allFrames: CorrelatedFrameSummary[],
  selection: FrameSelection | null,
): AnalysisWindow {
  return analysisWindowFromSelection(
    allFrames,
    selection == null ? null : { kind: "frames", range: selection },
  );
}

export function analysisWindowFromSelection(
  allFrames: CorrelatedFrameSummary[],
  selection: AnalysisSelection | null,
): AnalysisWindow {
  if (allFrames.length === 0) {
    return {
      frames: [],
      startFrame: selection?.kind === "frames" ? selection.range.startFrame : null,
      endFrame: selection?.kind === "frames" ? selection.range.endFrame : null,
      startCycle: selection?.kind === "cycles" ? selection.startCycle : null,
      endCycle: selection?.kind === "cycles" ? selection.endCycle : null,
      active: selection != null,
      selection,
    };
  }

  const capture = frameBounds(allFrames);
  const active = selectionIsNarrowerThanCapture(selection, capture);
  const frames =
    active && selection != null
      ? allFrames.filter((frame) => frameMatchesSelection(frame, selection))
      : allFrames;

  const selected = frameBounds(frames);

  return {
    frames,
    startFrame:
      selection?.kind === "frames" ? selection.range.startFrame : selected.startFrame,
    endFrame: selection?.kind === "frames" ? selection.range.endFrame : selected.endFrame,
    startCycle:
      active && selection?.kind === "cycles" ? selection.startCycle : selected.startCycle,
    endCycle: active && selection?.kind === "cycles" ? selection.endCycle : selected.endCycle,
    active,
    selection: active ? selection : null,
  };
}

export function frameSelectionForAnalysisSelection(
  frames: readonly {
    frame_number: number;
    begin_cycle: number;
    end_cycle: number;
  }[],
  selection: AnalysisSelection | null,
): FrameSelection | null {
  if (selection == null) return null;
  if (selection.kind === "frames") return selection.range;

  let startFrame: number | null = null;
  let endFrame: number | null = null;
  for (const frame of frames) {
    if (
      frame.end_cycle < selection.startCycle ||
      frame.begin_cycle > selection.endCycle
    ) {
      continue;
    }
    startFrame =
      startFrame == null ? frame.frame_number : Math.min(startFrame, frame.frame_number);
    endFrame = endFrame == null ? frame.frame_number : Math.max(endFrame, frame.frame_number);
  }
  return startFrame == null || endFrame == null ? null : { startFrame, endFrame };
}

type FrameBounds = {
  startFrame: number | null;
  endFrame: number | null;
  startCycle: number | null;
  endCycle: number | null;
};

function frameBounds(frames: readonly CorrelatedFrameSummary[]): FrameBounds {
  let startFrame: number | null = null;
  let endFrame: number | null = null;
  let startCycle: number | null = null;
  let endCycle: number | null = null;
  for (const frame of frames) {
    startFrame =
      startFrame == null ? frame.frame_number : Math.min(startFrame, frame.frame_number);
    endFrame = endFrame == null ? frame.frame_number : Math.max(endFrame, frame.frame_number);
    if (frame.cpu_begin_cycle != null) {
      startCycle =
        startCycle == null
          ? frame.cpu_begin_cycle
          : Math.min(startCycle, frame.cpu_begin_cycle);
    }
    if (frame.cpu_end_cycle != null) {
      endCycle =
        endCycle == null
          ? frame.cpu_end_cycle
          : Math.max(endCycle, frame.cpu_end_cycle);
    }
  }
  return { startFrame, endFrame, startCycle, endCycle };
}

function selectionIsNarrowerThanCapture(
  selection: AnalysisSelection | null,
  capture: FrameBounds,
): boolean {
  if (selection == null) return false;
  if (selection.kind === "frames") {
    return (
      capture.startFrame == null ||
      capture.endFrame == null ||
      selection.range.startFrame > capture.startFrame ||
      selection.range.endFrame < capture.endFrame
    );
  }
  return (
    capture.startCycle == null ||
    capture.endCycle == null ||
    selection.startCycle > capture.startCycle ||
    selection.endCycle < capture.endCycle
  );
}

function frameMatchesSelection(
  frame: CorrelatedFrameSummary,
  selection: AnalysisSelection,
): boolean {
  if (selection.kind === "frames") {
    return (
      frame.frame_number >= selection.range.startFrame &&
      frame.frame_number <= selection.range.endFrame
    );
  }
  return (
    frame.cpu_begin_cycle != null &&
    frame.cpu_end_cycle != null &&
    frame.cpu_end_cycle >= selection.startCycle &&
    frame.cpu_begin_cycle <= selection.endCycle
  );
}

export function cycleInWindow(
  cycle: number | null | undefined,
  window: AnalysisWindow,
): boolean {
  if (!window.active || window.startCycle == null || window.endCycle == null) {
    return true;
  }
  if (cycle == null) return false;
  return cycle >= window.startCycle && cycle <= window.endCycle;
}

export function intervalOverlapsWindow(
  start: number,
  end: number,
  window: AnalysisWindow,
): boolean {
  if (!window.active || window.startCycle == null || window.endCycle == null) {
    return true;
  }
  return end >= window.startCycle && start <= window.endCycle;
}

export function frameInWindow(
  frameNumber: number,
  window: AnalysisWindow,
): boolean {
  if (!window.active || window.startFrame == null || window.endFrame == null) {
    return true;
  }
  return frameNumber >= window.startFrame && frameNumber <= window.endFrame;
}

/** Merge per-frame top scopes into a rough range rollup. */
export function aggregateTopScopes(
  frames: CorrelatedFrameSummary[],
  limit = 40,
): CpuScopeSummary[] {
  const map = new Map<
    string,
    { spec_id: number; name: string; count: number; total_cycles: number; total_seconds: number }
  >();
  for (const frame of frames) {
    for (const scope of frame.top_cpu_scopes ?? []) {
      const key = `${scope.spec_id}:${scope.name}`;
      const row = map.get(key) ?? {
        spec_id: scope.spec_id,
        name: scope.name,
        count: 0,
        total_cycles: 0,
        total_seconds: 0,
      };
      row.count += scope.count;
      row.total_cycles += scope.total_cycles;
      row.total_seconds += scope.total_seconds ?? 0;
      map.set(key, row);
    }
  }
  return [...map.values()]
    .map((row) => ({
      spec_id: row.spec_id,
      name: row.name,
      count: row.count,
      total_cycles: row.total_cycles,
      total_seconds: row.total_seconds > 0 ? row.total_seconds : undefined,
    }))
    .sort(
      (a, b) =>
        (b.total_seconds ?? b.total_cycles) - (a.total_seconds ?? a.total_cycles),
    )
    .slice(0, limit);
}

export function aggregateTopBreadcrumbs(
  frames: CorrelatedFrameSummary[],
  limit = 40,
): { name: string; count: number; total_cycles: number }[] {
  const map = new Map<string, { name: string; count: number; total_cycles: number }>();
  for (const frame of frames) {
    for (const crumb of frame.top_gpu_breadcrumbs ?? []) {
      const row = map.get(crumb.name) ?? {
        name: crumb.name,
        count: 0,
        total_cycles: 0,
      };
      row.count += crumb.count;
      row.total_cycles += crumb.total_cycles;
      map.set(crumb.name, row);
    }
  }
  return [...map.values()]
    .sort((a, b) => b.total_cycles - a.total_cycles)
    .slice(0, limit);
}

export function cyclesToMs(
  cycles: number,
  cycleFrequency: number | undefined | null,
): number | null {
  if (cycleFrequency == null || cycleFrequency <= 0) return null;
  return (cycles / cycleFrequency) * 1000;
}

export function createDebouncedSetter<T>(
  setValue: (value: T) => void,
  delayMs: number,
): { push: (value: T) => void; flush: (value?: T) => void; cancel: () => void } {
  let timer: ReturnType<typeof setTimeout> | null = null;
  let pending: T | undefined;
  let hasPending = false;

  const cancel = () => {
    if (timer != null) {
      clearTimeout(timer);
      timer = null;
    }
  };

  const flush = (value?: T) => {
    cancel();
    if (value !== undefined) {
      setValue(value);
      hasPending = false;
      return;
    }
    if (hasPending) {
      setValue(pending as T);
      hasPending = false;
    }
  };

  const push = (value: T) => {
    pending = value;
    hasPending = true;
    cancel();
    timer = setTimeout(() => {
      timer = null;
      if (hasPending) {
        setValue(pending as T);
        hasPending = false;
      }
    }, delayMs);
  };

  return { push, flush, cancel };
}
