export const UTRACE_SCHEMA_VERSION: 2;

export type JsonPrimitive = boolean | number | string | null;
export type JsonValue = JsonPrimitive | JsonObject | JsonValue[];
export type JsonObject = { readonly [key: string]: JsonValue };

export type UtraceInput = Readonly<{
  bytes: Uint8Array;
  filename?: string;
}>;

export type DashboardOptions = Readonly<{
  maxFrames?: number;
  timelineFrame?: number;
  timelineLimit?: number;
  gpuTimelineFrame?: number;
  gpuTimelineLimit?: number;
}>;

export type DashboardInput = UtraceInput & Readonly<{ options?: DashboardOptions }>;

export type ProgressiveDashboardInput = Readonly<{
  totalBytes: number;
  filename?: string;
  options?: DashboardOptions;
}>;

export type UtraceEnvelope<BodyKey extends string> = Readonly<{
  schema_version: typeof UTRACE_SCHEMA_VERSION;
  status: "ok";
  path: string;
}> &
  Readonly<Record<BodyKey, JsonObject>>;

export type UtraceInspect = UtraceEnvelope<"trace">;
export type UtraceInventory = UtraceEnvelope<"inventory">;
export type UtraceDashboard = UtraceEnvelope<"dashboard">;
export type UtraceDashboardBundle = Readonly<{
  schema_version: typeof UTRACE_SCHEMA_VERSION;
  status: "ok";
  path: string;
  dashboard: JsonObject;
  inventory: JsonObject;
}>;

export type DecodeProgress = Readonly<{
  bytes_consumed: number;
  total_bytes?: number;
  packets_observed: number;
  phase: "reading" | "analyzing" | "complete";
}> &
  JsonObject;

export type BootstrapProgressEvent = Readonly<{
  type: "bootstrap";
  protocol_version: number;
  sequence: number;
  progress: DecodeProgress;
  bootstrap: JsonObject;
}>;

export type SnapshotProgressEvent = Readonly<{
  type: "snapshot";
  protocol_version: number;
  sequence: number;
  progress: DecodeProgress;
  patch: JsonObject;
}>;

export type CompleteProgressEvent = Readonly<{
  type: "complete";
  protocol_version: number;
  sequence: number;
  progress: DecodeProgress;
  dashboard: UtraceDashboard;
  inventory: UtraceInventory;
}>;

export type UtraceProgressEvent =
  | BootstrapProgressEvent
  | SnapshotProgressEvent
  | CompleteProgressEvent;

export class UtraceParserError extends Error {}

/** Eagerly initialize the bundled browser WebAssembly module. */
export function init(): Promise<void>;

export function inspect(input: UtraceInput): Promise<UtraceInspect>;
export function inventory(input: UtraceInput): Promise<UtraceInventory>;
export function dashboard(input: DashboardInput): Promise<UtraceDashboard>;
export function dashboardBundle(input: DashboardInput): Promise<UtraceDashboardBundle>;

export function createProgressiveDashboard(
  input: ProgressiveDashboardInput,
): Promise<ProgressiveDashboardSession>;

export class ProgressiveDashboardSession {
  readonly closed: boolean;
  pushChunk(bytes: Uint8Array): UtraceProgressEvent[];
  analyzing(): SnapshotProgressEvent;
  finish(): CompleteProgressEvent;
  dispose(): void;
}
