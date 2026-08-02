export function downsampleMinMax<T>({
  values,
  maxPoints,
  metrics,
}: {
  values: readonly T[];
  maxPoints: number;
  metrics: (value: T) => readonly number[];
}): T[] {
  const pointBudget = Math.max(0, Math.floor(maxPoints));
  if (values.length <= pointBudget) return [...values];
  if (pointBudget === 0 || values.length === 0) return [];
  if (pointBudget === 1) return [values[0]];

  const metricCount = Math.max(1, metrics(values[0]).length);
  if (pointBudget < 2 + metricCount * 2) {
    return [values[0], values[values.length - 1]];
  }
  const bucketCount = Math.floor((pointBudget - 2) / (metricCount * 2));
  const selected = new Set<number>([0, values.length - 1]);

  for (let bucket = 0; bucket < bucketCount; bucket += 1) {
    const start = Math.floor((bucket * values.length) / bucketCount);
    const end = Math.floor(((bucket + 1) * values.length) / bucketCount);
    const minimums = new Array<number>(metricCount).fill(start);
    const maximums = new Array<number>(metricCount).fill(start);
    const firstMetrics = metrics(values[start]);

    for (let index = start + 1; index < end; index += 1) {
      const pointMetrics = metrics(values[index]);
      for (let metric = 0; metric < metricCount; metric += 1) {
        const currentMinimum = metrics(values[minimums[metric]])[metric];
        const currentMaximum = metrics(values[maximums[metric]])[metric];
        const candidate = pointMetrics[metric] ?? 0;
        if (candidate < currentMinimum) minimums[metric] = index;
        if (candidate > currentMaximum) maximums[metric] = index;
      }
    }

    for (let metric = 0; metric < metricCount; metric += 1) {
      if (Number.isFinite(firstMetrics[metric] ?? 0)) {
        selected.add(minimums[metric]);
        selected.add(maximums[metric]);
      }
    }
  }

  return [...selected].sort((left, right) => left - right).map((index) => values[index]);
}
