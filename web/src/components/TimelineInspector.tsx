import { For, Show, createMemo } from "solid-js";
import type {
  TimelineIntervalInspection,
  TimelineLaneInterval,
} from "../lib/timeline-inspection";

type TimelineInspectorProps = {
  inspection: TimelineIntervalInspection;
  inspections: ReadonlyMap<string, TimelineIntervalInspection>;
  windowDuration: number;
  color: string;
  truncated: boolean;
  returnedCount: number;
  totalCount: number;
  formatOffset: (cycle: number) => string;
  formatDuration: (cycles: number) => string;
  onSelect: (id: string) => void;
  onFocusSpan: (interval: TimelineLaneInterval) => void;
  onClear: () => void;
};

export function TimelineInspector(props: TimelineInspectorProps) {
  const interval = () => props.inspection.interval;
  const parent = () =>
    props.inspection.parentId == null
      ? undefined
      : props.inspections.get(props.inspection.parentId);
  const children = createMemo(() =>
    props.inspection.childIds
      .map((id) => props.inspections.get(id))
      .filter(
        (child): child is TimelineIntervalInspection => child != null,
      )
      .sort(
        (left, right) =>
          durationOf(right.interval) - durationOf(left.interval),
      ),
  );
  const duration = () => durationOf(interval());
  const childDuration = () =>
    Math.max(0, duration() - props.inspection.selfDuration);
  const previousOccurrence = () =>
    props.inspection.occurrenceIds[props.inspection.occurrenceIndex - 1];
  const nextOccurrence = () =>
    props.inspection.occurrenceIds[props.inspection.occurrenceIndex + 1];
  const windowShare = () =>
    props.windowDuration > 0
      ? (duration() / props.windowDuration) * 100
      : 0;

  return (
    <aside
      class="timer-inspector"
      data-timeline-inspector
      aria-live="polite"
      style={`--span-color: ${props.color}`}
    >
      <header class="timer-inspector-head">
        <div class="timer-inspector-title">
          <span class="timer-inspector-swatch" aria-hidden="true" />
          <div>
            <p class="eyebrow">Selected span</p>
            <h3>{interval().label}</h3>
            <div class="timer-inspector-context">
              <span>{interval().lane}</span>
              <span>depth {props.inspection.depth + 1}</span>
              <span>
                occurrence {props.inspection.occurrenceIndex + 1} of{" "}
                {props.inspection.occurrenceIds.length}
              </span>
            </div>
          </div>
        </div>
        <div class="timer-inspector-actions">
          <button
            type="button"
            class="btn ghost compact"
            disabled={previousOccurrence() == null}
            onClick={() => {
              const id = previousOccurrence();
              if (id != null) props.onSelect(id);
            }}
          >
            ← Previous
          </button>
          <button
            type="button"
            class="btn ghost compact"
            disabled={nextOccurrence() == null}
            onClick={() => {
              const id = nextOccurrence();
              if (id != null) props.onSelect(id);
            }}
          >
            Next →
          </button>
          <button
            type="button"
            class="btn primary compact"
            onClick={() => props.onFocusSpan(interval())}
          >
            Focus span
          </button>
          <button
            type="button"
            class="timer-inspector-close"
            aria-label="Close span inspector"
            onClick={props.onClear}
          >
            ×
          </button>
        </div>
      </header>

      <div class="timer-inspector-metrics">
        <InspectorMetric
          label="Inclusive time"
          value={interval().durationLabel}
          hint={formatShare(windowShare()) + " of query window"}
          featured
        />
        <InspectorMetric
          label={props.truncated ? "Est. self" : "Self time"}
          value={props.formatDuration(props.inspection.selfDuration)}
          hint="excluding returned direct children"
        />
        <InspectorMetric
          label="Child time"
          value={props.formatDuration(childDuration())}
          hint={`${props.inspection.childIds.length} direct ${props.inspection.childIds.length === 1 ? "child" : "children"}`}
        />
        <InspectorMetric
          label="Range"
          value={`${props.formatOffset(interval().start)} → ${props.formatOffset(interval().end)}`}
          hint="relative to query start"
        />
      </div>

      <div class="timer-inspector-body">
        <section class="timer-inspector-identity" aria-labelledby="span-identity-heading">
          <h4 id="span-identity-heading">Identity</h4>
          <dl>
            <InspectorField label="Thread" value={threadIdentity(interval())} />
            <InspectorField label="Timer spec" value={optionalId(interval().specId)} />
            <InspectorField label="Metadata" value={optionalId(interval().metadataId)} />
            <InspectorField label="Start cycle" value={formatExact(interval().start)} />
            <InspectorField label="End cycle" value={formatExact(interval().end)} />
            <Show
              when={
                interval().sourceLabel != null &&
                interval().sourceLabel !== interval().label
              }
            >
              <InspectorField label="Source" value={interval().sourceLabel!} />
            </Show>
          </dl>
        </section>

        <section class="timer-inspector-relations" aria-labelledby="span-relations-heading">
          <h4 id="span-relations-heading">Call context</h4>
          <div class="timer-relation-group">
            <span class="timer-relation-label">Parent</span>
            <Show
              when={parent()}
              fallback={<span class="timer-relation-empty">Root in returned window</span>}
            >
              {(item) => (
                <RelationButton
                  inspection={item()}
                  formatDuration={props.formatDuration}
                  onSelect={props.onSelect}
                />
              )}
            </Show>
          </div>
          <div class="timer-relation-group">
            <span class="timer-relation-label">
              Direct children
              <Show when={children().length > 6}> · top 6 by duration</Show>
            </span>
            <Show
              when={children().length > 0}
              fallback={<span class="timer-relation-empty">Leaf span in returned window</span>}
            >
              <div class="timer-relation-list">
                <For each={children().slice(0, 6)}>
                  {(child) => (
                    <RelationButton
                      inspection={child}
                      formatDuration={props.formatDuration}
                      onSelect={props.onSelect}
                    />
                  )}
                </For>
              </div>
            </Show>
          </div>
        </section>
      </div>

      <Show when={props.truncated}>
        <p class="timer-inspector-caveat">
          Showing {props.returnedCount.toLocaleString()} of{" "}
          {props.totalCount.toLocaleString()} matching spans. Nesting, occurrence
          count, and estimated self time describe only the returned subset.
        </p>
      </Show>
    </aside>
  );
}

function InspectorMetric(props: {
  label: string;
  value: string;
  hint: string;
  featured?: boolean;
}) {
  return (
    <div
      class="timer-inspector-metric"
      classList={{ featured: props.featured }}
    >
      <span>{props.label}</span>
      <strong>{props.value}</strong>
      <em>{props.hint}</em>
    </div>
  );
}

function InspectorField(props: { label: string; value: string }) {
  return (
    <div>
      <dt>{props.label}</dt>
      <dd>{props.value}</dd>
    </div>
  );
}

function RelationButton(props: {
  inspection: TimelineIntervalInspection;
  formatDuration: (cycles: number) => string;
  onSelect: (id: string) => void;
}) {
  return (
    <button
      type="button"
      class="timer-relation"
      onClick={() => props.onSelect(props.inspection.interval.id)}
    >
      <span>{props.inspection.interval.label}</span>
      <em>{props.formatDuration(durationOf(props.inspection.interval))}</em>
    </button>
  );
}

function durationOf(interval: TimelineLaneInterval): number {
  return Math.max(0, interval.end - interval.start);
}

function optionalId(value: number | undefined): string {
  return value == null ? "—" : String(value);
}

function threadIdentity(interval: TimelineLaneInterval): string {
  return interval.threadId == null
    ? interval.lane
    : `${interval.lane} · ${interval.threadId}`;
}

function formatExact(value: number): string {
  return new Intl.NumberFormat("en-US", { maximumFractionDigits: 0 }).format(
    value,
  );
}

function formatShare(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return "0%";
  if (value < 0.01) return "<0.01%";
  return `${value.toFixed(value < 1 ? 2 : 1)}%`;
}
