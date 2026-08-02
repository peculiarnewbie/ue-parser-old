import type { FrameSelection } from "./frame-selection";
import type { CorrelatedFrameSummary, CpuScopeSummary } from "./types";

export type AnalysisWindow = {
  /** Frames included in the current brush (or all frames when unset). */
  frames: CorrelatedFrameSummary[];
  startFrame: number | null;
  endFrame: number | null;
  startCycle: number | null;
  endCycle: number | null;
  /** True when the brush narrowed the capture. */
  active: boolean;
  selection: FrameSelection | null;
};

export function analysisWindowFromFrameSelection(
  allFrames: CorrelatedFrameSummary[],
  selection: FrameSelection | null,
): AnalysisWindow {
  if (allFrames.length === 0) {
    return {
      frames: [],
      startFrame: selection?.startFrame ?? null,
      endFrame: selection?.endFrame ?? null,
      startCycle: null,
      endCycle: null,
      active: selection != null,
      selection,
    };
  }

  const frames = selection
    ? allFrames.filter(
        (frame) =>
          frame.frame_number >= selection.startFrame &&
          frame.frame_number <= selection.endFrame,
      )
    : allFrames;
  const active =
    selection != null &&
    (selection.startFrame > allFrames[0].frame_number ||
      selection.endFrame < allFrames[allFrames.length - 1].frame_number);

  let startCycle: number | null = null;
  let endCycle: number | null = null;
  for (const frame of frames) {
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

  return {
    frames,
    startFrame: selection?.startFrame ?? frames[0]?.frame_number ?? null,
    endFrame: selection?.endFrame ?? frames[frames.length - 1]?.frame_number ?? null,
    startCycle,
    endCycle,
    active,
    selection: active ? selection : null,
  };
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
