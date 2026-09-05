import type { AnalysisSelection } from "../analysis-range";
import type { ParseTiming } from "../api";
import type {
  CpuTimelineIndexInfo,
  UtraceDashboard,
  UtraceInventory,
} from "../types";
import type { UtraceSessionId } from "../wasm-worker-client";

export type ComparisonSide = "baseline" | "candidate";

export type EvidenceGrade =
  | "exact"
  | "exact_capture_wide"
  | "bounded_top_n"
  | "prefix_sampled"
  | "truncated_lower_bound"
  | "incompatible"
  | "unavailable";

export type ComparisonAlignment =
  | { kind: "statistical" }
  | {
      kind: "explicit_ranges";
      baseline: AnalysisSelection;
      candidate: AnalysisSelection;
    }
  | { kind: "relative_frame_ordinal"; acknowledged: true }
  | { kind: "relative_capture_time"; acknowledged: true };

export type ComparisonRangeMs = {
  startMs: number;
  endMs: number;
};

export type LoadedComparisonCapture = {
  file: File;
  dashboard: UtraceDashboard;
  inventory?: UtraceInventory;
  timelineIndex?: CpuTimelineIndexInfo;
  sessionId: UtraceSessionId;
  timing: ParseTiming;
};

export type CaptureLoadProgress = {
  bytesConsumed: number;
  totalBytes: number;
  phase: "reading" | "analyzing";
};

export type ComparisonCaptureState =
  | { status: "empty" }
  | { status: "loading"; file: File; progress: CaptureLoadProgress }
  | { status: "ready"; capture: LoadedComparisonCapture }
  | { status: "error"; file: File; message: string };

export type ComparabilityIssue = {
  id: string;
  severity: "blocking" | "warning" | "information";
  title: string;
  detail: string;
  affects: readonly ("frames" | "cpu" | "gpu" | "providers")[];
};

export type ComparabilityAssessment = {
  status: "comparable" | "review" | "incompatible";
  issues: ComparabilityIssue[];
};

export type FrameDistributionSummary = {
  evidence: EvidenceGrade;
  frameType: number;
  samplesMs: number[];
  count: number;
  averageMs: number;
  p50Ms: number;
  p90Ms: number;
  p99Ms: number;
  p999Ms: number;
  maxMs: number;
  budgetMs: number;
  budgetMissCount: number;
  budgetMissRate: number;
};

export type FrameDistributionComparison = {
  baseline: FrameDistributionSummary | null;
  candidate: FrameDistributionSummary | null;
  verdict:
    | { kind: "inconclusive"; reason: string }
    | { kind: "regression"; reason: string }
    | { kind: "improvement"; reason: string }
    | { kind: "stable"; reason: string };
};

export type TimerMatchConfidence = "source" | "unique_name" | "unmatched" | "ambiguous";

export type TimerDisposition =
  | "slower"
  | "faster"
  | "candidate_only"
  | "baseline_only"
  | "unchanged"
  | "insufficient_evidence";

export type TimerComparisonRow = {
  key: string;
  name: string;
  source?: string;
  disposition: TimerDisposition;
  matchConfidence: TimerMatchConfidence;
  evidence: EvidenceGrade;
  normalization: "game_frame" | "selected_second";
  baselineCount: number;
  candidateCount: number;
  baselineCountPerUnit: number | null;
  candidateCountPerUnit: number | null;
  baselineNormalizedMs: number | null;
  candidateNormalizedMs: number | null;
  deltaNormalizedMs: number | null;
  relativeDelta: number | null;
  baselineMeanUs: number | null;
  candidateMeanUs: number | null;
};

export type ProviderCoverage = {
  id: string;
  label: string;
  baseline: boolean;
  candidate: boolean;
  evidence: EvidenceGrade;
};
