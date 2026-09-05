import { describe, expect, it } from "vitest";
import type { CpuScopeSummary, UtraceDashboard, UtraceTimerStatsQuery } from "../types";
import { compareCpuTimerRanges, compareCpuTimers } from "./timers";

type ScopeInput = CpuScopeSummary & { file?: string; line?: number };

function dashboardWithScopes(
  scopes: ScopeInput[],
  frameCount = 10,
): UtraceDashboard {
  return {
    schema_version: 1,
    status: "ok",
    path: "test.utrace",
    dashboard: {
      prologue: { cycle_frequency: 1_000_000 },
      frame_timing: {
        total_frame_count: frameCount,
        frames: Array.from({ length: frameCount }, (_, frame_number) => ({
          frame_number,
          frame_type: 0,
        })),
      },
      cpu: {
        batches: { count: 1 },
        specs: scopes.map((scope) => ({
          id: scope.spec_id,
          name: scope.name,
          file: scope.file,
          line: scope.line,
        })),
        scopes,
      },
    },
  } as unknown as UtraceDashboard;
}

function timerStats(input: {
  durationSeconds: number;
  specId: number;
  name: string;
  totalSeconds: number;
  truncated?: boolean;
}): UtraceTimerStatsQuery {
  return {
    schema_version: 1,
    status: "ok",
    path: "test.utrace",
    timer_stats: {
      index: {
        source_bytes: 0,
        source_fingerprint: 0,
        total_interval_count: 1,
        indexed_interval_count: 1,
        truncated: false,
      },
      begin_cycle: 0,
      end_cycle: input.durationSeconds * 1_000_000,
      duration_seconds: input.durationSeconds,
      interval_count: 1,
      distinct_timer_count: 1,
      truncated: input.truncated ?? false,
      timers: [{
        spec_id: input.specId,
        name: input.name,
        overlap_count: 1,
        begin_count: 1,
        clipped_inclusive_cycles: input.totalSeconds * 1_000_000,
        clipped_inclusive_seconds: input.totalSeconds,
      }],
    },
  };
}

describe("CPU timer comparison", () => {
  const scope = (spec_id: number, file?: string): ScopeInput => ({
    spec_id, file, name: "Work", count: 1, total_cycles: 1_000,
  });

  it("preserves ambiguity when a grouped name meets one source-qualified timer", () => {
    const rows = compareCpuTimers(dashboardWithScopes([scope(1), scope(2)]), dashboardWithScopes([scope(3, "Source/Work.cpp")]));
    expect(rows).toHaveLength(1);
    expect(rows[0]?.matchConfidence).toBe("ambiguous");
    expect(rows[0]?.disposition).toBe("insufficient_evidence");
  });

  it("does not use uniqueness among leftovers after source matches", () => {
    const rows = compareCpuTimers(
      dashboardWithScopes([scope(1, "Source/A.cpp"), scope(2, "Source/B.cpp")]),
      dashboardWithScopes([scope(3, "Source/A.cpp"), scope(4, "Source/C.cpp")]),
    );
    expect(rows.filter((row) => row.matchConfidence === "source")).toHaveLength(1);
    expect(rows.filter((row) => row.matchConfidence === "ambiguous")).toHaveLength(2);
    expect(rows.some((row) => row.matchConfidence === "unique_name")).toBe(false);
  });

  it("does not treat an unavailable CPU provider as proof of new work", () => {
    const baseline = dashboardWithScopes([]);
    baseline.dashboard.cpu.batches = undefined;
    const [row] = compareCpuTimers(baseline, dashboardWithScopes([scope(1)]));
    expect(row?.disposition).toBe("insufficient_evidence");
    expect(row?.evidence).toBe("unavailable");
  });

  it("does not match capture-local fallback IDs across traces", () => {
    const baseline = dashboardWithScopes([{ ...scope(1), name: "#1" }]);
    const candidate = dashboardWithScopes([{ ...scope(1), name: "#1" }]);
    baseline.dashboard.cpu.specs = [];
    candidate.dashboard.cpu.specs = [];
    expect(compareCpuTimers(baseline, candidate)[0]?.matchConfidence).toBe("ambiguous");
  });

  it("keeps plain and metadata names distinct when their local IDs collide", () => {
    const baseline = dashboardWithScopes([scope(1, "Source/Plain.cpp")]);
    const stats = timerStats({ durationSeconds: 1, specId: 1, name: "Work", totalSeconds: 0.1 });
    stats.timer_stats.timers.push({ ...stats.timer_stats.timers[0]!, metadata_id: 5, name: "MetadataWork" });
    const rows = compareCpuTimerRanges({ baselineDashboard: baseline, candidateDashboard: baseline, baselineStats: stats, candidateStats: stats });
    expect(rows).toHaveLength(2);
    expect(rows.find((row) => row.name === "MetadataWork")?.source).toBeUndefined();
    expect(rows.find((row) => row.name === "Work")?.source).toBe("source/plain.cpp");
  });

  it("does not classify relative changes from an incomplete CPU index", () => {
    const baseline = dashboardWithScopes([scope(1)]);
    const candidate = dashboardWithScopes([scope(2)]);
    const baselineStats = timerStats({ durationSeconds: 1, specId: 1, name: "Work", totalSeconds: 0.1 });
    baselineStats.timer_stats.index.truncated = true;
    const [row] = compareCpuTimerRanges({ baselineDashboard: baseline, candidateDashboard: candidate, baselineStats,
      candidateStats: timerStats({ durationSeconds: 1, specId: 2, name: "Work", totalSeconds: 0.2 }) });
    expect(row?.evidence).toBe("truncated_lower_bound");
    expect(row?.disposition).toBe("insufficient_evidence");
  });

  it("groups metadata variants of the same spec without introducing name ambiguity", () => {
    const baseline = dashboardWithScopes([scope(1)]);
    const candidate = dashboardWithScopes([scope(2)]);
    const baselineStats = timerStats({ durationSeconds: 1, specId: 1, name: "Work", totalSeconds: 0.1 });
    baselineStats.timer_stats.timers.push({ ...baselineStats.timer_stats.timers[0]!, metadata_id: 5 });
    const [row] = compareCpuTimerRanges({ baselineDashboard: baseline, candidateDashboard: candidate,
      baselineStats, candidateStats: timerStats({ durationSeconds: 1, specId: 2, name: "Work", totalSeconds: 0.2 }) });
    expect(row?.matchConfidence).toBe("unique_name");
    expect(row?.baselineNormalizedMs).toBeCloseTo(200);
    expect(row?.disposition).toBe("unchanged");
  });

  it("matches source timers across different checkout roots", () => {
    const baseline = dashboardWithScopes([{
      spec_id: 1,
      name: "FEngineLoop::Tick",
      file: "C:/baseline/Source/Runtime/Launch/LaunchEngineLoop.cpp",
      line: 100,
      count: 100,
      total_cycles: 1_000_000,
    }]);
    const candidate = dashboardWithScopes([{
      spec_id: 99,
      name: "FEngineLoop::Tick",
      file: "D:/candidate/Source/Runtime/Launch/LaunchEngineLoop.cpp",
      line: 100,
      count: 100,
      total_cycles: 1_500_000,
    }]);

    const [row] = compareCpuTimers(baseline, candidate);
    expect(row?.matchConfidence).toBe("source");
    expect(row?.disposition).toBe("slower");
    expect(row?.deltaNormalizedMs).toBeCloseTo(50);
  });

  it("normalizes capture-wide totals by completed game frames", () => {
    const baseline = dashboardWithScopes([{
      spec_id: 1,
      name: "Tick",
      count: 10,
      total_cycles: 1_000_000,
    }], 10);
    const candidate = dashboardWithScopes([{
      spec_id: 2,
      name: "Tick",
      count: 20,
      total_cycles: 2_000_000,
    }], 20);

    const [row] = compareCpuTimers(baseline, candidate);
    expect(row?.baselineNormalizedMs).toBeCloseTo(100);
    expect(row?.candidateNormalizedMs).toBeCloseTo(100);
    expect(row?.disposition).toBe("unchanged");
  });

  it("keeps exhaustive candidate-only timers explicit", () => {
    const rows = compareCpuTimers(
      dashboardWithScopes([]),
      dashboardWithScopes([{
        spec_id: 3,
        name: "NewWork",
        count: 4,
        total_cycles: 40_000,
      }]),
    );

    expect(rows).toHaveLength(1);
    expect(rows[0]?.disposition).toBe("candidate_only");
    expect(rows[0]?.evidence).toBe("exact_capture_wide");
  });

  it("normalizes explicit ranges by each independently selected second", () => {
    const baseline = dashboardWithScopes([{
      spec_id: 1,
      name: "Tick",
      count: 1,
      total_cycles: 1,
    }]);
    const candidate = dashboardWithScopes([{
      spec_id: 2,
      name: "Tick",
      count: 1,
      total_cycles: 1,
    }]);
    const rows = compareCpuTimerRanges({
      baselineDashboard: baseline,
      candidateDashboard: candidate,
      baselineStats: timerStats({ durationSeconds: 2, specId: 1, name: "Tick", totalSeconds: 1 }),
      candidateStats: timerStats({ durationSeconds: 1, specId: 2, name: "Tick", totalSeconds: 0.75 }),
    });

    expect(rows[0]?.normalization).toBe("selected_second");
    expect(rows[0]?.baselineNormalizedMs).toBeCloseTo(500);
    expect(rows[0]?.candidateNormalizedMs).toBeCloseTo(750);
    expect(rows[0]?.deltaNormalizedMs).toBeCloseTo(250);
  });

  it("does not call a timer new when either range identity set is truncated", () => {
    const baseline = dashboardWithScopes([]);
    const candidate = dashboardWithScopes([{
      spec_id: 2,
      name: "MaybeNew",
      count: 1,
      total_cycles: 1,
    }]);
    const rows = compareCpuTimerRanges({
      baselineDashboard: baseline,
      candidateDashboard: candidate,
      baselineStats: {
        ...timerStats({ durationSeconds: 1, specId: 99, name: "Other", totalSeconds: 0.1, truncated: true }),
        timer_stats: {
          ...timerStats({ durationSeconds: 1, specId: 99, name: "Other", totalSeconds: 0.1, truncated: true }).timer_stats,
          timers: [],
          distinct_timer_count: 20_000,
        },
      },
      candidateStats: timerStats({ durationSeconds: 1, specId: 2, name: "MaybeNew", totalSeconds: 0.1 }),
    });

    expect(rows[0]?.disposition).toBe("insufficient_evidence");
    expect(rows[0]?.evidence).toBe("bounded_top_n");
  });
});
