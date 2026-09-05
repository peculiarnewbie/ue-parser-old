import type { TraceDashboardBody, UtraceDashboard } from "../types";
import type {
  ComparabilityAssessment,
  ComparabilityIssue,
  EvidenceGrade,
  ProviderCoverage,
} from "./model";

function normalizeIdentity(value: string | undefined): string {
  return value?.trim().toLocaleLowerCase() ?? "";
}

function different(
  baseline: string | undefined,
  candidate: string | undefined,
): boolean {
  const left = normalizeIdentity(baseline);
  const right = normalizeIdentity(candidate);
  return left !== "" && right !== "" && left !== right;
}

export function assessComparability(
  baseline: UtraceDashboard,
  candidate: UtraceDashboard,
): ComparabilityAssessment {
  const left = baseline.dashboard;
  const right = candidate.dashboard;
  const issues: ComparabilityIssue[] = [];
  const warn = (
    id: string,
    title: string,
    detail: string,
    affects: ComparabilityIssue["affects"],
  ) => issues.push({ id, title, detail, affects, severity: "warning" });

  if (!left.frame_timing?.frames.length || !right.frame_timing?.frames.length) {
    issues.push({
      id: "missing-frames",
      severity: "blocking",
      title: "Frame comparison unavailable",
      detail: "Both captures need completed frame timing events.",
      affects: ["frames"],
    });
  }
  const leftFrequency = left.prologue?.cycle_frequency;
  const rightFrequency = right.prologue?.cycle_frequency;
  if (
    leftFrequency == null ||
    !(leftFrequency > 0) ||
    rightFrequency == null ||
    !(rightFrequency > 0)
  ) {
    issues.push({
      id: "missing-timebase",
      severity: "blocking",
      title: "Timer conversion unavailable",
      detail: "Both captures need a valid cycle frequency for millisecond deltas.",
      affects: ["cpu"],
    });
  }
  if (different(left.session?.project_name, right.session?.project_name)) {
    warn(
      "project",
      "Different projects",
      `${left.session?.project_name} vs ${right.session?.project_name}`,
      ["frames", "cpu", "gpu", "providers"],
    );
  }
  if (different(left.session?.platform, right.session?.platform)) {
    warn(
      "platform",
      "Different platforms",
      `${left.session?.platform} vs ${right.session?.platform}`,
      ["frames", "cpu", "gpu"],
    );
  }
  if (different(left.session?.configuration, right.session?.configuration)) {
    warn(
      "configuration",
      "Different build configurations",
      `${left.session?.configuration} vs ${right.session?.configuration}`,
      ["frames", "cpu", "gpu"],
    );
  }
  if (different(left.session?.build_version, right.session?.build_version)) {
    warn(
      "build",
      "Different builds",
      `${left.session?.build_version} vs ${right.session?.build_version}`,
      ["cpu", "gpu", "providers"],
    );
  }

  const leftCount = left.frame_timing?.frames.length ?? 0;
  const rightCount = right.frame_timing?.frames.length ?? 0;
  if (leftCount > 0 && rightCount > 0) {
    const ratio = Math.max(leftCount, rightCount) / Math.min(leftCount, rightCount);
    if (ratio >= 2) {
      warn(
        "capture-length",
        "Unequal capture lengths",
        `${leftCount.toLocaleString()} vs ${rightCount.toLocaleString()} retained frames; totals should not be compared directly.`,
        ["frames", "cpu", "gpu", "providers"],
      );
    }
  }

  return {
    status: issues.some((issue) => issue.severity === "blocking")
      ? "incompatible"
      : issues.length > 0
        ? "review"
        : "comparable",
    issues,
  };
}

function observedProviders(body: TraceDashboardBody): Set<string> {
  const providers = new Set<string>();
  if (body.frame_timing?.frames.length) providers.add("frames");
  if (body.cpu.scopes.length) providers.add("cpu");
  if (body.gpu.queues.length || body.gpu.work?.intervals) providers.add("gpu");
  if (body.counters.counters.length) providers.add("counters");
  if (body.stats.stats.length || body.stats.sample_events) providers.add("stats");
  if (body.memory.allocs.count || body.memory.llm.sample_events) providers.add("memory");
  if (body.io_store.requests_created || body.platform_file.file_count) providers.add("io");
  if (body.tasks.created || body.tasks.started) providers.add("tasks");
  if (body.annotations.bookmarks.events || body.annotations.regions.completed) providers.add("annotations");
  return providers;
}

const PROVIDER_LABELS: Readonly<Record<string, string>> = {
  frames: "Frames",
  cpu: "CPU timers",
  gpu: "GPU",
  counters: "Counters",
  stats: "Stats",
  memory: "Memory",
  io: "I/O",
  tasks: "Tasks",
  annotations: "Annotations",
};

export function compareProviderCoverage(
  baseline: UtraceDashboard,
  candidate: UtraceDashboard,
): ProviderCoverage[] {
  const left = observedProviders(baseline.dashboard);
  const right = observedProviders(candidate.dashboard);
  return Object.entries(PROVIDER_LABELS).map(([id, label]) => {
    const inBaseline = left.has(id);
    const inCandidate = right.has(id);
    const evidence: EvidenceGrade = inBaseline && inCandidate ? "exact" : "unavailable";
    return { id, label, baseline: inBaseline, candidate: inCandidate, evidence };
  });
}

export function captureQualityNotes(dashboard: UtraceDashboard): string[] {
  const body = dashboard.dashboard;
  const notes: string[] = [];
  const batches = body.cpu.batches;
  if ((body.frame_timing?.total_frame_count ?? 0) > (body.frame_timing?.frames.length ?? 0)) notes.push("frame timing population is incomplete");
  if (batches?.unresolved_specs) notes.push(`${batches.unresolved_specs} unresolved CPU scopes`);
  if (batches?.unmatched_ends) notes.push(`${batches.unmatched_ends} unmatched CPU ends`);
  if (batches?.unterminated_scopes) notes.push(`${batches.unterminated_scopes} unterminated CPU scopes`);
  if (batches?.implausible_duration_count) notes.push(`${batches.implausible_duration_count} implausible CPU durations`);
  if (body.dispatch?.gap_count) notes.push(`${body.dispatch.gap_count} transport gaps`);
  if (body.frame_correlation.truncated) notes.push("frame correlation is truncated");
  if (body.gpu.frames_truncated) notes.push("GPU frame rows are truncated");
  if (body.callstacks.truncated) notes.push("callstacks are truncated");
  return notes;
}
