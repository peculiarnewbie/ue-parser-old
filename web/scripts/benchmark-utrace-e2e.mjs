import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdir, readFile, writeFile } from "node:fs/promises";
import { basename, resolve } from "node:path";
import { performance } from "node:perf_hooks";
import { parseArgs } from "node:util";

const webRoot = resolve(import.meta.dirname, "..");
const repositoryRoot = resolve(webRoot, "..");
const defaultTrace = resolve(
  repositoryRoot,
  "..",
  "tools",
  ".local",
  "traces",
  "funguys-build-30229-dev-20260807.utrace",
);
const defaultInsights =
  "C:\\Program Files\\Epic Games\\UE_5.7\\Engine\\Binaries\\Win64\\UnrealInsights.exe";

const { values } = parseArgs({
  options: {
    input: { type: "string", short: "i" },
    repeat: { type: "string", short: "r" },
    output: { type: "string", short: "o" },
    unreal: { type: "string" },
    "browser-only": { type: "boolean", default: false },
    "skip-build": { type: "boolean", default: false },
    help: { type: "boolean", short: "h", default: false },
  },
});

if (values.help) {
  console.log(`Usage: npm run benchmark:e2e -- [options]

Options:
  -i, --input <trace>      Real .utrace fixture
  -r, --repeat <1..10>    Samples for each frontend (default: 3)
  -o, --output <dir>      Durable result directory
      --unreal <exe>      UnrealInsights.exe path
      --browser-only      Do not run Unreal Insights
      --skip-build        Reuse the current production dist bundle
  -h, --help              Show this help

Environment equivalents: UTRACE_E2E_TRACE, UTRACE_E2E_REPEAT,
UTRACE_E2E_OUTPUT_DIR, and UNREAL_INSIGHTS_EXE.`);
  process.exit(0);
}

const tracePath = resolve(values.input ?? process.env.UTRACE_E2E_TRACE ?? defaultTrace);
const repeatCount = parseRepeatCount(values.repeat ?? process.env.UTRACE_E2E_REPEAT);
const outputDirectory = resolve(
  values.output ??
    process.env.UTRACE_E2E_OUTPUT_DIR ??
    resolve(webRoot, "benchmark-results", timestamp()),
);
const insightsPath = resolve(
  values.unreal ?? process.env.UNREAL_INSIGHTS_EXE ?? defaultInsights,
);

requireFile(tracePath, "trace");
if (!values["browser-only"]) requireFile(insightsPath, "Unreal Insights executable");
await mkdir(outputDirectory, { recursive: true });

console.log(`Browser benchmark: ${basename(tracePath)} (${repeatCount} sample(s))`);
const browserScript = values["skip-build"] ? "test:e2e:quick" : "test:e2e";
const npmCommand =
  process.platform === "win32"
    ? {
        command: process.env.ComSpec ?? "cmd.exe",
        arguments: ["/d", "/s", "/c", `npm run ${browserScript}`],
      }
    : { command: "npm", arguments: ["run", browserScript] };
await runCommand(npmCommand.command, npmCommand.arguments, {
  cwd: webRoot,
  env: {
    ...process.env,
    UTRACE_E2E_TRACE: tracePath,
    UTRACE_E2E_REPEAT: String(repeatCount),
    UTRACE_E2E_OUTPUT_DIR: outputDirectory,
  },
});

const browserReportPath = resolve(outputDirectory, "browser-performance.json");
const browser = requireBrowserReport(
  JSON.parse(await readFile(browserReportPath, "utf8")),
);
let unreal = null;

if (!values["browser-only"]) {
  console.log(`Unreal Insights benchmark: ${basename(insightsPath)} (${repeatCount} sample(s))`);
  const samples = [];
  for (let run = 1; run <= repeatCount; run += 1) {
    const logPath = resolve(outputDirectory, `unreal-insights-${run}.log`);
    const sample = await benchmarkUnrealInsights({
      executable: insightsPath,
      tracePath,
      logPath,
      timeoutMs: 180_000,
    });
    samples.push({ run, ...sample, log: logPath });
    console.log(
      `  ${run}: analysis ${formatMs(sample.analysis_ms)}, ` +
        `process → Timers data ${formatMs(sample.process_to_timers_ready_ms)}`,
    );
  }
  unreal = {
    executable: insightsPath,
    samples,
    medians_ms: {
      analysis: median(samples.map((sample) => sample.analysis_ms)),
      timers_aggregation: median(samples.map((sample) => sample.timers_aggregation_ms)),
      process_to_timers_ready: median(
        samples.map((sample) => sample.process_to_timers_ready_ms),
      ),
    },
  };
}

const browserApplication = browser.application_medians_ms;
const browserDiagnostics = browser.diagnostic_medians_ms;
const browserApplicationToFirstTimelineMs =
  browserApplication.load_to_dashboard_paint +
  browserApplication.timers_tab_paint +
  browserApplication.frame_to_timeline_paint;
const report = {
  schema_version: 3,
  status: "ok",
  trace: tracePath,
  repeat_count: repeatCount,
  measurement_policy: {
    primary_scope: "application",
    excluded_from_comparison: [
      "browser_process_launch",
      "page_navigation_and_empty_app_paint",
      "playwright_action_and_polling_intervals",
      "unreal_process_to_timers_ready",
    ],
  },
  milestones: {
    browser_process_launch_diagnostic:
      "Chromium launch request through process connection. Retained as a diagnostic and excluded from application comparisons.",
    browser_page_startup_diagnostic:
      "Connected Chromium through context/page creation, navigation, visible file input, and two animation frames. Retained as a diagnostic and excluded from application comparisons.",
    browser_load_to_dashboard_paint:
      "File selection through stream, WASM analysis, state commit, and two animation frames.",
    browser_timers_tab_paint:
      "Timers click handler through Solid mount and two animation frames. Includes chart and frame-table construction.",
    browser_timeline_load:
      "Worst-marker handler through retained query, state commit, and two animation frames for the SVG timeline.",
    browser_application_through_first_timeline_paint:
      "Sum of the in-page file-load, Timers-tab-paint, and timeline-load spans. Browser startup plus automation intervals between handlers are excluded.",
    unreal_analysis:
      "Unreal Insights' own 'Analysis has completed in' duration; excludes process startup.",
    unreal_process_to_timers_ready_diagnostic:
      "Process launch until the first Timers aggregated-stats completion log. Retained as a diagnostic, not used for ratios, and not a Slate paint event.",
  },
  browser,
  unreal,
  comparison:
    unreal == null
      ? null
      : {
          browser_finish_and_serialize_over_unreal_analysis:
            browserApplication.wasm_finish_and_serialize / unreal.medians_ms.analysis,
          browser_file_to_dashboard_over_unreal_analysis:
            browserApplication.load_to_dashboard_paint / unreal.medians_ms.analysis,
        },
};
const reportPath = resolve(outputDirectory, "comparison.json");
await writeFile(reportPath, `${JSON.stringify(report, null, 2)}\n`, "utf8");

console.log(`Browser file → dashboard median: ${formatMs(browserApplication.load_to_dashboard_paint)}`);
if (browserApplication.wasm_finish_phases) {
  console.log(
    `  finish dispatch ${formatMs(browserApplication.wasm_finish_phases.normal_event_dispatch)}, ` +
      `CPU aggregation ${formatMs(browserApplication.wasm_finish_phases.cpu_aggregation)}, ` +
      `provider finalize ${formatMs(browserApplication.wasm_finish_phases.provider_finalize)}`,
  );
}
console.log(
  `Browser Timers tab paint median: ${formatMs(browserApplication.timers_tab_paint)}`,
);
console.log(
  `Browser timeline query → paint median: ${formatMs(browserApplication.frame_to_timeline_paint)}`,
);
console.log(
  `Browser application through first timeline paint: ${formatMs(browserApplicationToFirstTimelineMs)}`,
);
console.log(
  `[diagnostic, excluded] Browser process launch: ${formatMs(browserDiagnostics.browser_process_launch)}`,
);
console.log(
  `[diagnostic, excluded] Page navigation + empty paint: ${formatMs(browserDiagnostics.page_navigation_and_empty_app_paint)}`,
);
if (unreal) {
  console.log(`Unreal analysis median: ${formatMs(unreal.medians_ms.analysis)}`);
  console.log(
    `[diagnostic, excluded] Unreal process → Timers data: ${formatMs(unreal.medians_ms.process_to_timers_ready)}`,
  );
}
console.log(`Report: ${reportPath}`);

async function benchmarkUnrealInsights({ executable, tracePath: input, logPath, timeoutMs }) {
  await writeFile(logPath, "", "utf8");
  const startedAt = performance.now();
  const child = spawn(
    executable,
    [
      `-OpenTraceFile=${input}`,
      `-ABSLOG=${logPath}`,
      "-NoSplash",
      "-Unattended",
      "-NoTraceThread",
    ],
    { stdio: "ignore", windowsHide: false },
  );
  let exit = null;
  const exitPromise = new Promise((resolveExit, reject) => {
    child.once("error", reject);
    child.once("exit", (code, signal) => {
      exit = { code, signal };
      resolveExit(exit);
    });
  });

  try {
    while (performance.now() - startedAt < timeoutMs) {
      const log = await readWhenAvailable(logPath);
      const analysis = parseAnalysisDuration(log);
      const aggregation = parseTimersAggregation(log);
      if (analysis != null && aggregation != null) {
        return {
          analysis_ms: analysis,
          timers_aggregation_ms: aggregation,
          process_to_timers_ready_ms: roundMillis(performance.now() - startedAt),
        };
      }
      if (exit) {
        throw new Error(
          `Unreal Insights exited before Timers were ready (code ${exit.code}, signal ${exit.signal})`,
        );
      }
      await delay(100);
    }
    throw new Error(`Unreal Insights did not publish Timers data within ${timeoutMs} ms`);
  } finally {
    if (!exit) {
      child.kill();
      await Promise.race([exitPromise, delay(5_000)]);
    }
    if (!exit) child.kill("SIGKILL");
  }
}

function parseAnalysisDuration(log) {
  const match = /Analysis has completed in ([\d.]+)\s*(ms|s)\b/.exec(log);
  if (!match) return null;
  return roundMillis(Number(match[1]) * (match[2] === "s" ? 1_000 : 1));
}

function parseTimersAggregation(log) {
  const match = /\[Timers\] Aggregated stats computed in .*?- total: ([\d.]+)s/.exec(log);
  return match ? roundMillis(Number(match[1]) * 1_000) : null;
}

async function readWhenAvailable(path) {
  try {
    return await readFile(path, "utf8");
  } catch (error) {
    if (error?.code === "ENOENT") return "";
    throw error;
  }
}

async function runCommand(command, arguments_, options) {
  await new Promise((resolveCommand, reject) => {
    const child = spawn(command, arguments_, { ...options, stdio: "inherit" });
    child.once("error", reject);
    child.once("exit", (code, signal) => {
      if (code === 0) resolveCommand();
      else reject(new Error(`${command} exited with code ${code} (signal ${signal})`));
    });
  });
}

function requireFile(path, label) {
  if (!existsSync(path)) throw new Error(`${label} does not exist: ${path}`);
}

function requireBrowserReport(value) {
  if (value?.schema_version !== 3 || value?.status !== "ok") {
    throw new Error("browser performance report is not a successful schema v3 report");
  }
  const requiredApplicationMedians = [
    "load_to_dashboard_paint",
    "timers_tab_paint",
    "frame_to_timeline_paint",
  ];
  for (const name of requiredApplicationMedians) {
    if (!Number.isFinite(value.application_medians_ms?.[name])) {
      throw new Error(`browser performance report is missing application median: ${name}`);
    }
  }
  const requiredDiagnosticMedians = [
    "browser_process_launch",
    "page_navigation_and_empty_app_paint",
    "dashboard_to_timers_tab_paint",
    "timers_tab_to_timeline_paint",
  ];
  for (const name of requiredDiagnosticMedians) {
    if (!Number.isFinite(value.diagnostic_medians_ms?.[name])) {
      throw new Error(`browser performance report is missing diagnostic median: ${name}`);
    }
  }
  return value;
}

function parseRepeatCount(value) {
  const parsed = Number(value ?? "3");
  if (!Number.isSafeInteger(parsed) || parsed < 1 || parsed > 10) {
    throw new Error("repeat must be an integer from 1 through 10");
  }
  return parsed;
}

function median(values) {
  const ordered = [...values].sort((left, right) => left - right);
  const middle = Math.floor(ordered.length / 2);
  return ordered.length % 2 === 1
    ? ordered[middle]
    : (ordered[middle - 1] + ordered[middle]) / 2;
}

function roundMillis(value) {
  return Math.round(value * 1_000) / 1_000;
}

function formatMs(value) {
  return `${value.toFixed(2)} ms`;
}

function timestamp() {
  return new Date().toISOString().replaceAll(":", "-").replace(".", "-");
}

function delay(milliseconds) {
  return new Promise((resolveDelay) => setTimeout(resolveDelay, milliseconds));
}
