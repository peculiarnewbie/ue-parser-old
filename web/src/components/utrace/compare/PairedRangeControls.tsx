import { Show } from "solid-js";
import { formatNumber } from "../../../lib/format";
import type {
  ComparisonRangeMs,
  LoadedComparisonCapture,
} from "../../../lib/utrace-comparison/model";

export function PairedRangeControls(props: {
  baseline: LoadedComparisonCapture;
  candidate: LoadedComparisonCapture;
  baselineRange: ComparisonRangeMs;
  candidateRange: ComparisonRangeMs;
  baselineDurationMs: number;
  candidateDurationMs: number;
  busy: boolean;
  error?: string;
  onBaselineRange: (range: ComparisonRangeMs) => void;
  onCandidateRange: (range: ComparisonRangeMs) => void;
  onAnalyze: () => void;
}) {
  const RangeFields = (fields: {
    side: "baseline" | "candidate";
    range: ComparisonRangeMs;
    duration: number;
    filename: string;
    update: (range: ComparisonRangeMs) => void;
  }) => (
    <fieldset>
      <legend><span>{fields.side === "baseline" ? "A" : "B"}</span>{fields.side}</legend>
      <p title={fields.filename}>{fields.filename}</p>
      <div>
        <label>
          Start
          <span><input
            type="number"
            aria-label={`${fields.side} range start`}
            min="0"
            max={fields.duration}
            step="1"
            value={fields.range.startMs}
            onInput={(event) => fields.update({ ...fields.range, startMs: event.currentTarget.valueAsNumber || 0 })}
          /> ms</span>
        </label>
        <label>
          End
          <span><input
            type="number"
            aria-label={`${fields.side} range end`}
            min="0"
            max={fields.duration}
            step="1"
            value={fields.range.endMs}
            onInput={(event) => fields.update({ ...fields.range, endMs: event.currentTarget.valueAsNumber || 0 })}
          /> ms</span>
        </label>
      </div>
      <small>{formatNumber(fields.range.endMs - fields.range.startMs, 1)} ms selected · capture {formatNumber(fields.duration, 1)} ms</small>
    </fieldset>
  );

  return (
    <section class="compare-range-editor">
      <header>
        <div>
          <p class="eyebrow">Explicit alignment</p>
          <h3>Compare independent local ranges</h3>
        </div>
        <p>Ranges may have different lengths. Timer impact is normalized per selected second; cycles are never synchronized.</p>
      </header>
      <div class="compare-range-fields">
        <RangeFields side="baseline" range={props.baselineRange} duration={props.baselineDurationMs} filename={props.baseline.file.name} update={props.onBaselineRange} />
        <span class="compare-range-not-equal">≠</span>
        <RangeFields side="candidate" range={props.candidateRange} duration={props.candidateDurationMs} filename={props.candidate.file.name} update={props.onCandidateRange} />
      </div>
      <footer>
        <Show when={props.error}><p class="compare-range-error">{props.error}</p></Show>
        <button class="btn primary" type="button" disabled={props.busy} onClick={props.onAnalyze}>
          {props.busy ? "Aggregating exact intervals…" : "Analyze paired ranges"}
        </button>
      </footer>
    </section>
  );
}
