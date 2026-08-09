export type TimelineLaneInterval = {
  id: string;
  lane: string;
  label: string;
  sourceLabel?: string;
  start: number;
  end: number;
  durationLabel: string;
  threadId?: number;
  specId?: number;
  metadataId?: number;
};

export type TimelineIntervalInspection = {
  interval: TimelineLaneInterval;
  depth: number;
  parentId?: string;
  childIds: string[];
  selfDuration: number;
  occurrenceIds: string[];
  occurrenceIndex: number;
};

type MutableInspection = Omit<
  TimelineIntervalInspection,
  "selfDuration" | "occurrenceIds" | "occurrenceIndex"
>;

export function inspectTimelineIntervals(
  intervals: readonly TimelineLaneInterval[],
): ReadonlyMap<string, TimelineIntervalInspection> {
  const mutable = new Map<string, MutableInspection>();
  const byLane = groupBy(intervals, (interval) => interval.lane);

  for (const laneIntervals of byLane.values()) {
    const stack: MutableInspection[] = [];
    const ordered = [...laneIntervals].sort(compareIntervals);

    for (const interval of ordered) {
      while (
        stack.length > 0 &&
        !strictlyContains(stack.at(-1)!.interval, interval)
      ) {
        stack.pop();
      }

      const parent = stack.at(-1);
      const inspection: MutableInspection = {
        interval,
        depth: stack.length,
        parentId: parent?.interval.id,
        childIds: [],
      };
      mutable.set(interval.id, inspection);
      parent?.childIds.push(interval.id);
      stack.push(inspection);
    }
  }

  const occurrences = groupBy(intervals, occurrenceKey);
  const occurrenceById = new Map<
    string,
    { ids: string[]; index: number }
  >();
  for (const group of occurrences.values()) {
    const ids = [...group].sort(compareIntervals).map((item) => item.id);
    ids.forEach((id, index) => occurrenceById.set(id, { ids, index }));
  }

  return new Map(
    [...mutable].map(([id, inspection]) => {
      const occurrence = occurrenceById.get(id) ?? { ids: [id], index: 0 };
      return [
        id,
        {
          ...inspection,
          selfDuration: calculateSelfDuration(inspection, mutable),
          occurrenceIds: occurrence.ids,
          occurrenceIndex: occurrence.index,
        },
      ];
    }),
  );
}

function compareIntervals(
  left: TimelineLaneInterval,
  right: TimelineLaneInterval,
): number {
  return (
    left.start - right.start ||
    right.end - left.end ||
    left.id.localeCompare(right.id)
  );
}

function strictlyContains(
  parent: TimelineLaneInterval,
  child: TimelineLaneInterval,
): boolean {
  return (
    parent.start <= child.start &&
    parent.end >= child.end &&
    (parent.start < child.start || parent.end > child.end)
  );
}

function calculateSelfDuration(
  inspection: MutableInspection,
  all: ReadonlyMap<string, MutableInspection>,
): number {
  const duration = Math.max(
    0,
    inspection.interval.end - inspection.interval.start,
  );
  const childRanges = inspection.childIds
    .map((id) => all.get(id)?.interval)
    .filter((interval): interval is TimelineLaneInterval => interval != null)
    .map((interval) => ({
      start: Math.max(inspection.interval.start, interval.start),
      end: Math.min(inspection.interval.end, interval.end),
    }))
    .filter((range) => range.end > range.start)
    .sort((left, right) => left.start - right.start || left.end - right.end);

  let covered = 0;
  let currentStart: number | undefined;
  let currentEnd: number | undefined;
  for (const range of childRanges) {
    if (currentStart == null || currentEnd == null) {
      currentStart = range.start;
      currentEnd = range.end;
      continue;
    }
    if (range.start <= currentEnd) {
      currentEnd = Math.max(currentEnd, range.end);
      continue;
    }
    covered += currentEnd - currentStart;
    currentStart = range.start;
    currentEnd = range.end;
  }
  if (currentStart != null && currentEnd != null) {
    covered += currentEnd - currentStart;
  }
  return Math.max(0, duration - covered);
}

function occurrenceKey(interval: TimelineLaneInterval): string {
  return interval.specId != null
    ? `spec:${interval.specId}:${interval.label}`
    : `label:${interval.label}`;
}

function groupBy<T, K>(
  values: readonly T[],
  keyFor: (value: T) => K,
): Map<K, T[]> {
  const groups = new Map<K, T[]>();
  for (const value of values) {
    const key = keyFor(value);
    const group = groups.get(key);
    if (group) group.push(value);
    else groups.set(key, [value]);
  }
  return groups;
}
