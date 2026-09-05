import { For, Show, createMemo } from "solid-js";
import { LineSeriesChart } from "../../Charts";
import { formatNumber } from "../../../lib/format";
import {
  assessComparability,
  captureQualityNotes,
  compareProviderCoverage,
} from "../../../lib/utrace-comparison/compatibility";
import type { LoadedComparisonCapture } from "../../../lib/utrace-comparison/model";
import {
  compareFrameDistributions,
  quantileProfile,
} from "../../../lib/utrace-comparison/statistics";

const QUANTILES = [0, 0.1, 0.25, 0.5, 0.75, 0.9, 0.95, 0.99, 0.999] as const;

function formatDelta(value: number, unit = "ms"): string {
  const sign = value > 0 ? "+" : "";
  return `${sign}${formatNumber(value, 2)}${unit}`;
}

function ComparisonMetric(props: {
  label: string;
  baseline: number;
  candidate: number;
  format: (value: number) => string;
  lowerIsBetter?: boolean;
}) {
  const delta = () => props.candidate - props.baseline;
  const direction = () => {
    if (Math.abs(delta()) < 0.000_001) return "neutral";
    const worse = props.lowerIsBetter !== false ? delta() > 0 : delta() < 0;
    return worse ? "worse" : "better";
  };
  return (
    <article class="compare-metric" data-direction={direction()}>
      <p>{props.label}</p>
      <div class="compare-metric-values">
        <span><i>A</i>{props.format(props.baseline)}</span>
        <span><i>B</i>{props.format(props.candidate)}</span>
      </div>
      <strong>{formatDelta(delta(), props.label === "Budget miss" ? " pp" : " ms")}</strong>
    </article>
  );
}

export function ComparisonOverview(props: {
  baseline: LoadedComparisonCapture;
  candidate: LoadedComparisonCapture;
  frameType: number;
  budgetMs: number;
}) {
  const assessment = createMemo(() => assessComparability(
    props.baseline.dashboard,
    props.candidate.dashboard,
  ));
  const comparison = createMemo(() => compareFrameDistributions({
    baseline: props.baseline.dashboard,
    candidate: props.candidate.dashboard,
    frameType: props.frameType,
    budgetMs: props.budgetMs,
  }));
  const providerCoverage = createMemo(() => compareProviderCoverage(
    props.baseline.dashboard,
    props.candidate.dashboard,
  ));
  const profileRows = createMemo(() => {
    const baseline = comparison().baseline;
    const candidate = comparison().candidate;
    if (!baseline || !candidate) return [];
    const baselineProfile = quantileProfile(baseline.samplesMs, QUANTILES);
    const candidateProfile = quantileProfile(candidate.samplesMs, QUANTILES);
    return QUANTILES.map((quantile, index) => ({
      quantile: quantile === 0.999 ? "p99.9" : quantile === 0 ? "min" : `p${quantile * 100}`,
      baseline: baselineProfile[index] ?? 0,
      candidate: candidateProfile[index] ?? 0,
    }));
  });
  const quality = createMemo(() => ({
    baseline: captureQualityNotes(props.baseline.dashboard),
    candidate: captureQualityNotes(props.candidate.dashboard),
  }));

  return (
    <div class="compare-results">
      <section class={`compare-verdict compare-verdict-${comparison().verdict.kind}`}>
        <div class="compare-verdict-signal" aria-hidden="true" />
        <div>
          <p class="eyebrow">Distribution verdict</p>
          <h2>{comparison().verdict.kind === "regression" ? "Regression signal" : comparison().verdict.kind === "improvement" ? "Candidate improved" : comparison().verdict.kind === "stable" ? "No material tail shift" : "Comparison inconclusive"}</h2>
          <p>{comparison().verdict.reason}</p>
        </div>
        <div class="compare-verdict-method">
          <span>Decision rule</span>
          <strong>p99 · ≥0.5 ms · ≥5%</strong>
          <small>Effect threshold, not a significance test</small>
        </div>
      </section>

      <Show when={assessment().status !== "comparable" || quality().baseline.length || quality().candidate.length}>
        <section class={`compare-evidence-strip compare-evidence-${assessment().status}`}>
          <div>
            <span class="compare-evidence-state">{assessment().status}</span>
            <strong>{assessment().status === "incompatible" ? "Some metrics cannot be compared" : "Review capture differences"}</strong>
          </div>
          <ul>
            <For each={assessment().issues}>{(issue) => <li><strong>{issue.title}</strong> — {issue.detail}</li>}</For>
            <For each={quality().baseline}>{(note) => <li><strong>Baseline quality</strong> — {note}</li>}</For>
            <For each={quality().candidate}>{(note) => <li><strong>Candidate quality</strong> — {note}</li>}</For>
          </ul>
        </section>
      </Show>

      <Show when={comparison().baseline && comparison().candidate}>
        <div class="compare-metric-grid">
          <ComparisonMetric
            label="Median"
            baseline={comparison().baseline!.p50Ms}
            candidate={comparison().candidate!.p50Ms}
            format={(value) => `${formatNumber(value, 2)} ms`}
          />
          <ComparisonMetric
            label="p90"
            baseline={comparison().baseline!.p90Ms}
            candidate={comparison().candidate!.p90Ms}
            format={(value) => `${formatNumber(value, 2)} ms`}
          />
          <ComparisonMetric
            label="p99"
            baseline={comparison().baseline!.p99Ms}
            candidate={comparison().candidate!.p99Ms}
            format={(value) => `${formatNumber(value, 2)} ms`}
          />
          <ComparisonMetric
            label="Budget miss"
            baseline={comparison().baseline!.budgetMissRate * 100}
            candidate={comparison().candidate!.budgetMissRate * 100}
            format={(value) => `${formatNumber(value, 1)}%`}
          />
        </div>

        <div class="compare-distribution-grid">
          <LineSeriesChart
            title="Frame-time quantile profile"
            subtitle={`Independent ${props.frameType === 0 ? "Game" : "Rendering"} frame populations · retained summaries`}
            data={profileRows()}
            xKey="quantile"
            series={[
              { key: "baseline", name: "Baseline" },
              { key: "candidate", name: "Candidate" },
            ]}
            height={320}
          />
          <section class="panel compare-observations">
            <header>
              <p class="eyebrow">Denominators</p>
              <h2>What was observed</h2>
            </header>
            <div class="compare-observation-counts">
              <div><span>A</span><strong>{comparison().baseline!.count.toLocaleString()}</strong><small>frames</small></div>
              <div><span>B</span><strong>{comparison().candidate!.count.toLocaleString()}</strong><small>frames</small></div>
            </div>
            <p>Totals are normalized by completed Game frames. The two runs are compared statistically; frame ordinals are not paired.</p>
            <div class="compare-budget-rule">
              <span>Budget</span>
              <strong>{formatNumber(props.budgetMs, 2)} ms</strong>
            </div>
          </section>
        </div>
      </Show>

      <section class="panel compare-provider-panel">
        <header>
          <p class="eyebrow">Observed evidence</p>
          <h2>Provider coverage</h2>
          <p class="muted">A missing provider is unavailable, never zero.</p>
        </header>
        <div class="compare-provider-grid" role="table" aria-label="Provider coverage">
          <div class="compare-provider-head" role="row"><span>Provider</span><span>Baseline</span><span>Candidate</span></div>
          <For each={providerCoverage()}>{(provider) => (
            <div role="row">
              <strong>{provider.label}</strong>
              <span classList={{ present: provider.baseline }}>{provider.baseline ? "observed" : "missing"}</span>
              <span classList={{ present: provider.candidate }}>{provider.candidate ? "observed" : "missing"}</span>
            </div>
          )}</For>
        </div>
      </section>
    </div>
  );
}
