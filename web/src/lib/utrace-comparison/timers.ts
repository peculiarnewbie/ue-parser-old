import type {
  CpuScopeSummary,
  TraceDashboardBody,
  UtraceDashboard,
  UtraceTimerStatsQuery,
} from "../types";
import type {
  TimerComparisonRow,
  TimerDisposition,
  TimerMatchConfidence,
} from "./model";

type TimerRecord = {
  key: string;
  name: string;
  source?: string;
  sourceQualified: boolean;
  ambiguous: boolean;
  count: number;
  totalSeconds: number | null;
};

function normalizedSource(file: string, line: number | undefined): string {
  const segments = file.replaceAll("\\", "/").toLowerCase().split("/");
  const sourceIndex = segments.lastIndexOf("source");
  const suffix = segments
    .slice(sourceIndex >= 0 ? sourceIndex : -3)
    .join("/");
  return line == null ? suffix : `${suffix}:${line}`;
}

function secondsForScope(
  scope: CpuScopeSummary,
  cycleFrequency: number | undefined,
): number | null {
  if (scope.total_seconds != null && Number.isFinite(scope.total_seconds)) {
    return scope.total_seconds;
  }
  if (cycleFrequency == null || !(cycleFrequency > 0)) return null;
  return scope.total_cycles / cycleFrequency;
}

function timerRecords(body: TraceDashboardBody): TimerRecord[] {
  const specs = new Map((body.cpu.specs ?? []).map((spec) => [spec.id, spec]));
  // Metadata variants of one local spec belong to the same source timer.
  const scopes = new Map<string, CpuScopeSummary>();
  for (const scope of body.cpu.scopes) {
    const key = `${scope.spec_id}\0${scope.name ?? ""}`;
    const previous = scopes.get(key);
    const seconds = secondsForScope(scope, body.prologue?.cycle_frequency);
    const previousSeconds = previous && secondsForScope(previous, body.prologue?.cycle_frequency);
    scopes.set(key, previous ? {
      ...previous,
      count: previous.count + scope.count,
      total_cycles: previous.total_cycles + scope.total_cycles,
      total_seconds: seconds != null && previousSeconds != null ? previousSeconds + seconds : undefined,
    } : scope);
  }
  const raw = [...scopes.values()].map((scope) => {
    const localSpec = specs.get(scope.spec_id);
    // Plain and metadata specs have separate local catalogs.
    const spec = !scope.name || localSpec?.name === scope.name ? localSpec : undefined;
    const name = scope.name || spec?.name || `#${scope.spec_id}`;
    const source = spec?.file ? normalizedSource(spec.file, spec.line) : undefined;
    return {
      key: source ? `${name}\0${source}` : name,
      name,
      source,
      sourceQualified: source != null,
      ambiguous: (!scope.name && !spec?.name) || (!spec && name === `#${scope.spec_id}`),
      count: scope.count,
      totalSeconds: secondsForScope(scope, body.prologue?.cycle_frequency),
    };
  });
  const nameCounts = new Map<string, number>();
  for (const record of raw) {
    nameCounts.set(record.name, (nameCounts.get(record.name) ?? 0) + 1);
  }
  const grouped = new Map<string, TimerRecord>();
  for (const record of raw) {
    const ambiguous = record.ambiguous || (!record.sourceQualified && (nameCounts.get(record.name) ?? 0) > 1);
    const key = ambiguous ? `${record.key}\0ambiguous` : record.key;
    const previous = grouped.get(key);
    if (previous) {
      previous.count += record.count;
      previous.totalSeconds = previous.totalSeconds == null || record.totalSeconds == null
        ? null
        : previous.totalSeconds + record.totalSeconds;
      previous.ambiguous ||= ambiguous;
    } else {
      grouped.set(key, { ...record, key, ambiguous });
    }
  }
  return [...grouped.values()];
}

function disposition(input: {
  baseline: TimerRecord | undefined;
  candidate: TimerRecord | undefined;
  deltaNormalizedMs: number | null;
  relativeDelta: number | null;
  confidence: TimerMatchConfidence;
  exhaustive: boolean;
}): TimerDisposition {
  if (input.confidence === "ambiguous" || !input.exhaustive) return "insufficient_evidence";
  if (!input.baseline) return input.exhaustive ? "candidate_only" : "insufficient_evidence";
  if (!input.candidate) return input.exhaustive ? "baseline_only" : "insufficient_evidence";
  if (input.deltaNormalizedMs == null) return "insufficient_evidence";
  if (
    Math.abs(input.deltaNormalizedMs) < 0.01 &&
    Math.abs(input.relativeDelta ?? 0) < 0.01
  ) return "unchanged";
  return input.deltaNormalizedMs > 0 ? "slower" : "faster";
}

function perUnit(value: number | null, unitCount: number): number | null {
  return value == null || unitCount <= 0 ? null : value / unitCount;
}

function relative(baseline: number | null, candidate: number | null): number | null {
  return baseline == null || candidate == null || baseline === 0
    ? null
    : (candidate - baseline) / baseline;
}

function buildRow(input: {
  baseline?: TimerRecord;
  candidate?: TimerRecord;
  baselineUnits: number;
  candidateUnits: number;
  normalization: TimerComparisonRow["normalization"];
  confidence: TimerMatchConfidence;
  exhaustive: boolean;
  evidence: TimerComparisonRow["evidence"];
}): TimerComparisonRow {
  const identity = input.candidate ?? input.baseline!;
  const baselineSecondsPerUnit = perUnit(input.baseline?.totalSeconds ?? null, input.baselineUnits);
  const candidateSecondsPerUnit = perUnit(input.candidate?.totalSeconds ?? null, input.candidateUnits);
  const baselineNormalizedMs = baselineSecondsPerUnit == null ? null : baselineSecondsPerUnit * 1000;
  const candidateNormalizedMs = candidateSecondsPerUnit == null ? null : candidateSecondsPerUnit * 1000;
  const deltaNormalizedMs = baselineNormalizedMs == null || candidateNormalizedMs == null
    ? null
    : candidateNormalizedMs - baselineNormalizedMs;
  const relativeDelta = relative(baselineNormalizedMs, candidateNormalizedMs);
  return {
    key: `${identity.key}\0${input.confidence}`,
    name: identity.name,
    source: identity.source,
    disposition: disposition({
      baseline: input.baseline,
      candidate: input.candidate,
      deltaNormalizedMs,
      relativeDelta,
      confidence: input.confidence,
      exhaustive: input.exhaustive,
    }),
    matchConfidence: input.confidence,
    evidence: input.evidence,
    normalization: input.normalization,
    baselineCount: input.baseline?.count ?? 0,
    candidateCount: input.candidate?.count ?? 0,
    baselineCountPerUnit: perUnit(input.baseline?.count ?? null, input.baselineUnits),
    candidateCountPerUnit: perUnit(input.candidate?.count ?? null, input.candidateUnits),
    baselineNormalizedMs,
    candidateNormalizedMs,
    deltaNormalizedMs,
    relativeDelta,
    baselineMeanUs: input.baseline?.totalSeconds == null || !input.baseline.count
      ? null
      : (input.baseline.totalSeconds / input.baseline.count) * 1_000_000,
    candidateMeanUs: input.candidate?.totalSeconds == null || !input.candidate.count
      ? null
      : (input.candidate.totalSeconds / input.candidate.count) * 1_000_000,
  };
}

function compareTimerRecords(input: {
  baseline: TimerRecord[];
  candidate: TimerRecord[];
  baselineUnits: number;
  candidateUnits: number;
  normalization: TimerComparisonRow["normalization"];
  exhaustive: boolean;
  evidence: TimerComparisonRow["evidence"];
}): TimerComparisonRow[] {
  const left = input.baseline;
  const right = input.candidate;
  const leftByKey = new Map(left.map((record) => [record.key, record]));
  const rightByKey = new Map(right.map((record) => [record.key, record]));
  const consumedLeft = new Set<string>();
  const consumedRight = new Set<string>();
  const rows: TimerComparisonRow[] = [];

  const rowInput = {
    baselineUnits: input.baselineUnits,
    candidateUnits: input.candidateUnits,
    normalization: input.normalization,
    exhaustive: input.exhaustive,
    evidence: input.evidence,
  };

  for (const [key, baselineRecord] of leftByKey) {
    const candidateRecord = rightByKey.get(key);
    if (!candidateRecord) continue;
    consumedLeft.add(key);
    consumedRight.add(key);
    rows.push(buildRow({
      ...rowInput,
      baseline: baselineRecord,
      candidate: candidateRecord,
      confidence: baselineRecord.ambiguous || candidateRecord.ambiguous ? "ambiguous" : baselineRecord.sourceQualified ? "source" : "unique_name",
    }));
  }

  const leftNames = new Map<string, TimerRecord[]>();
  const rightNames = new Map<string, TimerRecord[]>();
  for (const [records, names] of [[left, leftNames], [right, rightNames]] as const) {
    for (const record of records) {
      const existing = names.get(record.name);
      if (existing) existing.push(record);
      else names.set(record.name, [record]);
    }
  }
  for (const [name, baselineRecords] of leftNames) {
    const candidateRecords = rightNames.get(name);
    if (baselineRecords.length !== 1 || candidateRecords?.length !== 1) continue;
    const baselineRecord = baselineRecords[0]!;
    const candidateRecord = candidateRecords[0]!;
    if (consumedLeft.has(baselineRecord.key) || consumedRight.has(candidateRecord.key)) continue;
    consumedLeft.add(baselineRecord.key);
    consumedRight.add(candidateRecord.key);
    rows.push(buildRow({
      ...rowInput,
      baseline: baselineRecord,
      candidate: candidateRecord,
      confidence: baselineRecord.ambiguous || candidateRecord.ambiguous ? "ambiguous" : "unique_name",
    }));
  }

  for (const record of left.filter((item) => !consumedLeft.has(item.key))) {
    rows.push(buildRow({
      ...rowInput,
      baseline: record,
      confidence: record.ambiguous || rightNames.has(record.name) ? "ambiguous" : "unmatched",
    }));
  }
  for (const record of right.filter((item) => !consumedRight.has(item.key))) {
    rows.push(buildRow({
      ...rowInput,
      candidate: record,
      confidence: record.ambiguous || leftNames.has(record.name) ? "ambiguous" : "unmatched",
    }));
  }

  return rows.sort((leftRow, rightRow) => {
    const leftImpact = Math.abs(leftRow.deltaNormalizedMs ?? leftRow.candidateNormalizedMs ?? leftRow.baselineNormalizedMs ?? 0);
    const rightImpact = Math.abs(rightRow.deltaNormalizedMs ?? rightRow.candidateNormalizedMs ?? rightRow.baselineNormalizedMs ?? 0);
    return rightImpact - leftImpact || leftRow.name.localeCompare(rightRow.name);
  });
}

export function compareCpuTimers(
  baseline: UtraceDashboard,
  candidate: UtraceDashboard,
): TimerComparisonRow[] {
  const baselineFrames = baseline.dashboard.frame_timing?.frames.filter((frame) => frame.frame_type === 0).length ?? 0;
  const candidateFrames = candidate.dashboard.frame_timing?.frames.filter((frame) => frame.frame_type === 0).length ?? 0;
  const available = [baseline, candidate].every((capture) =>
    capture.dashboard.cpu.scopes.length > 0 || (capture.dashboard.cpu.batches?.count ?? 0) > 0);
  const complete = [baseline, candidate].every((capture) => {
    const body = capture.dashboard;
    return cpuDataComplete(body) &&
      body.frame_timing?.total_frame_count === body.frame_timing?.frames.length;
  });
  return compareTimerRecords({
    baseline: timerRecords(baseline.dashboard),
    candidate: timerRecords(candidate.dashboard),
    baselineUnits: baselineFrames,
    candidateUnits: candidateFrames,
    normalization: "game_frame",
    exhaustive: available && complete,
    evidence: !available ? "unavailable" : complete ? "exact_capture_wide" : "truncated_lower_bound",
  });
}

function cpuDataComplete(body: TraceDashboardBody): boolean {
  const batches = body.cpu.batches;
  return !body.dispatch?.gap_count && !batches?.unresolved_specs && !batches?.unmatched_ends &&
    !batches?.unterminated_scopes && !batches?.implausible_duration_count;
}

function rangeTimerRecords(
  body: TraceDashboardBody,
  query: UtraceTimerStatsQuery,
): TimerRecord[] {
  const scopes: CpuScopeSummary[] = query.timer_stats.timers.map((timer) => ({
    spec_id: timer.spec_id,
    name: timer.name,
    count: timer.overlap_count,
    total_cycles: timer.clipped_inclusive_cycles,
    total_seconds: timer.clipped_inclusive_seconds,
  }));
  return timerRecords({ ...body, cpu: { ...body.cpu, scopes } });
}

export function compareCpuTimerRanges(input: {
  baselineDashboard: UtraceDashboard;
  candidateDashboard: UtraceDashboard;
  baselineStats: UtraceTimerStatsQuery;
  candidateStats: UtraceTimerStatsQuery;
}): TimerComparisonRow[] {
  const baselineSeconds = input.baselineStats.timer_stats.duration_seconds ?? 0;
  const candidateSeconds = input.candidateStats.timer_stats.duration_seconds ?? 0;
  const incompleteIndex = input.baselineStats.timer_stats.index.truncated || input.candidateStats.timer_stats.index.truncated ||
    !cpuDataComplete(input.baselineDashboard.dashboard) || !cpuDataComplete(input.candidateDashboard.dashboard);
  const exhaustive = !incompleteIndex && !input.baselineStats.timer_stats.truncated && !input.candidateStats.timer_stats.truncated;
  return compareTimerRecords({
    baseline: rangeTimerRecords(input.baselineDashboard.dashboard, input.baselineStats),
    candidate: rangeTimerRecords(input.candidateDashboard.dashboard, input.candidateStats),
    baselineUnits: baselineSeconds,
    candidateUnits: candidateSeconds,
    normalization: "selected_second",
    exhaustive,
    evidence: incompleteIndex ? "truncated_lower_bound" : exhaustive ? "exact" : "bounded_top_n",
  });
}
