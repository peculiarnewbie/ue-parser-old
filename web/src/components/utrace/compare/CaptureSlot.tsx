import { Match, Show, Switch } from "solid-js";
import { formatBytes } from "../../../lib/format";
import type {
  ComparisonCaptureState,
  ComparisonSide,
  LoadedComparisonCapture,
} from "../../../lib/utrace-comparison/model";
import { DropZone } from "../../DropZone";

function captureIdentity(capture: LoadedComparisonCapture): string {
  const session = capture.dashboard.dashboard.session;
  return session?.project_name || session?.app_name || "Unknown Unreal session";
}

export function ComparisonCaptureSlot(props: {
  side: ComparisonSide;
  state: ComparisonCaptureState;
  disabled: boolean;
  onFile: (file: File) => void | Promise<void>;
  onClear: () => void;
}) {
  const label = () => props.side === "baseline" ? "Baseline" : "Candidate";
  return (
    <section
      class={`compare-capture compare-capture-${props.side}`}
      classList={{ ready: props.state.status === "ready" }}
      aria-label={`${label()} capture`}
    >
      <header class="compare-capture-head">
        <div>
          <span class="compare-side-index">{props.side === "baseline" ? "A" : "B"}</span>
          <div>
            <p>{label()}</p>
            <strong>{props.side === "baseline" ? "Known-good run" : "Run under test"}</strong>
          </div>
        </div>
        <Show when={props.state.status !== "empty"}>
          <button
            class="compare-icon-button"
            type="button"
            disabled={props.state.status === "loading"}
            onClick={props.onClear}
            aria-label={`Remove ${label().toLowerCase()} capture`}
          >
            ×
          </button>
        </Show>
      </header>

      <Switch>
        <Match when={props.state.status === "empty"}>
          <DropZone
            accept=".utrace,application/octet-stream"
            label={`Choose ${label().toLowerCase()} .utrace`}
            hint="Parsed locally. The capture never leaves this browser."
            busy={props.disabled}
            onFile={props.onFile}
          />
        </Match>
        <Match when={props.state.status === "loading" ? props.state : undefined}>
          {(state) => (
            <div class="compare-load-state" aria-live="polite">
              <span class="compare-orbit" aria-hidden="true" />
              <div>
                <strong>{state().file.name}</strong>
                <p>{state().progress.phase === "reading" ? "Reading and decoding" : "Building retained indexes"}</p>
              </div>
              <div class="compare-progress-track">
                <span
                  style={{
                    width: `${Math.min(100, (state().progress.bytesConsumed / Math.max(1, state().progress.totalBytes)) * 100)}%`,
                  }}
                />
              </div>
              <small>
                {formatBytes(state().progress.bytesConsumed)} / {formatBytes(state().progress.totalBytes)}
              </small>
            </div>
          )}
        </Match>
        <Match when={props.state.status === "ready" ? props.state.capture : undefined}>
          {(capture) => {
            const body = () => capture().dashboard.dashboard;
            return (
              <div class="compare-capture-ready">
                <span class="compare-capture-state"><i /> retained session</span>
                <h2>{captureIdentity(capture())}</h2>
                <p class="compare-filename">{capture().file.name}</p>
                <dl>
                  <div>
                    <dt>File</dt>
                    <dd>{formatBytes(capture().file.size)}</dd>
                  </div>
                  <div>
                    <dt>Frames</dt>
                    <dd>{(body().frame_timing?.frames.length ?? 0).toLocaleString()}</dd>
                  </div>
                  <div>
                    <dt>Build</dt>
                    <dd>{body().session?.build_version || body().session?.branch || "—"}</dd>
                  </div>
                  <div>
                    <dt>Platform</dt>
                    <dd>{body().session?.platform || "—"}</dd>
                  </div>
                </dl>
                <DropZone
                  accept=".utrace,application/octet-stream"
                  label={`Replace ${label().toLowerCase()}`}
                  hint=""
                  compact
                  busy={props.disabled}
                  onFile={props.onFile}
                />
              </div>
            );
          }}
        </Match>
        <Match when={props.state.status === "error" ? props.state : undefined}>
          {(state) => (
            <div class="compare-capture-error">
              <strong>Could not parse {state().file.name}</strong>
              <p>{state().message}</p>
              <DropZone
                accept=".utrace,application/octet-stream"
                label="Try another capture"
                hint=""
                compact
                busy={props.disabled}
                onFile={props.onFile}
              />
            </div>
          )}
        </Match>
      </Switch>
    </section>
  );
}
