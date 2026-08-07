export * from "./index.js";

import type {
  DashboardInput,
  ProgressiveDashboardInput,
  ProgressiveDashboardSession,
  UtraceDashboard,
  UtraceDashboardBundle,
  UtraceInput,
  UtraceInspect,
  UtraceInventory,
} from "./index.js";

/** Eagerly initialize the bundled WebAssembly module from its local package file. */
export function init(): Promise<void>;

export function inspect(input: UtraceInput): Promise<UtraceInspect>;
export function inventory(input: UtraceInput): Promise<UtraceInventory>;
export function dashboard(input: DashboardInput): Promise<UtraceDashboard>;
export function dashboardBundle(input: DashboardInput): Promise<UtraceDashboardBundle>;
export function createProgressiveDashboard(
  input: ProgressiveDashboardInput,
): Promise<ProgressiveDashboardSession>;
