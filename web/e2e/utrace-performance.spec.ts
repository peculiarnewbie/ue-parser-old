import { chromium, expect, test, type Page } from "@playwright/test";
import { existsSync, readFileSync, statSync } from "node:fs";
import { mkdir, writeFile } from "node:fs/promises";
import { resolve } from "node:path";
import { performance } from "node:perf_hooks";
import {
  UTRACE_SPAN,
  type ChromeTraceEvent,
  type UtraceBenchmarkSnapshot,
  type UtraceSpanName,
} from "../src/lib/perf-span-types";
import {
  WASM_FINISH_PHASES,
  finishPhaseSpanName,
  type WasmFinishPhase,
} from "../src/lib/wasm-finish-profile";

const repositoryRoot = resolve(import.meta.dirname, "..", "..");
const defaultTrace = resolve(
  repositoryRoot,
  "..",
  "tools",
  ".local",
  "traces",
  "funguys-build-30229-dev-20260807.utrace",
);
const requestedTrace = process.env.UTRACE_E2E_TRACE
  ? resolve(process.env.UTRACE_E2E_TRACE)
  : defaultTrace;
const tracePath = existsSync(requestedTrace) ? requestedTrace : null;
const repeatCount = parseRepeatCount(process.env.UTRACE_E2E_REPEAT);
const durableOutputDirectory = process.env.UTRACE_E2E_OUTPUT_DIR
  ? resolve(process.env.UTRACE_E2E_OUTPUT_DIR)
  : null;

async function leaveDuringFileRead(page: Page): Promise<void> {
  await page.waitForFunction(({ push, finish }) => {
    const benchmark = (window as Window & { __UTRACE_BENCHMARK__: { snapshot: () => UtraceBenchmarkSnapshot } }).__UTRACE_BENCHMARK__;
    const spans = benchmark.snapshot().spans;
    if (!spans.some((span) => span.name === push) || spans.some((span) => span.name === finish)) return false;
    document.querySelector<HTMLAnchorElement>(".nav a[href='/uasset']")!.click();
    return true;
  }, { push: UTRACE_SPAN.sessionPush, finish: UTRACE_SPAN.sessionFinish });
}

test.describe("UTrace browser performance", () => {
  test.skip(
    tracePath == null,
    "requires a real .utrace fixture; set UTRACE_E2E_TRACE",
  );

  test("leaving the single viewer during reading does not recreate its terminated worker", async ({ page }) => {
    let parserWorkers = 0;
    const errors: string[] = [];
    page.on("worker", (worker) => { if (worker.url().includes("wasm-worker")) parserWorkers += 1; });
    page.on("pageerror", (error) => errors.push(error.message));
    await page.goto("/utrace");
    await page.locator('input[type="file"]').setInputFiles(tracePath!);
    await leaveDuringFileRead(page);
    await expect(page).toHaveURL(/\/uasset$/);
    await expect.poll(async () => (await browserSnapshot(page)).spans
      .filter((span) => span.name === UTRACE_SPAN.load && span.status === "error").length).toBe(1);
    expect(parserWorkers).toBe(1);
    expect(errors).toEqual([]);
  });

  test("leaving during worker file reading releases the session and permits two new captures", async ({ page }) => {
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    await page.goto("/utrace/compare");
    await page.getByLabel("Baseline capture", { exact: true }).locator("input[type=file]").setInputFiles(tracePath!);
    await leaveDuringFileRead(page);
    await expect(page).toHaveURL(/\/uasset$/);
    await expect.poll(async () => completedSpanCount(await browserSnapshot(page), UTRACE_SPAN.sessionCancel)).toBeGreaterThan(0);
    await page.locator(".nav a[href='/utrace/compare']").click();
    const bytes = Buffer.from(readFileSync(resolve(repositoryRoot, "tests/fixtures/tiny/comparison-work.utrace.hex"), "utf8")
      .replace(/#.*$/gm, "").replace(/\s/g, ""), "hex");
    for (const side of ["Baseline", "Candidate"]) {
      const slot = page.getByLabel(`${side} capture`, { exact: true });
      await slot.locator("input[type=file]").setInputFiles({ name: `${side}.utrace`, mimeType: "application/octet-stream", buffer: bytes });
      await expect(slot.locator(".compare-capture-state")).toBeVisible();
    }
    await expect(page.getByRole("heading", { name: "No material tail shift" })).toBeVisible();
    expect(errors).toEqual([]);
  });

  test("records cold launch, dashboard, and timeline paint milestones", async ({}, testInfo) => {
    test.setTimeout(Math.max(180_000, repeatCount * 90_000));
    const samples: BrowserPerformanceSample[] = [];
    let lastSnapshot: UtraceBenchmarkSnapshot | undefined;
    let lastChromeTrace: { traceEvents: ChromeTraceEvent[] } | undefined;
    const benchmarkUrl = new URL("/utrace", requireBaseUrl(testInfo.project.use.baseURL)).href;
    const screenshotPath = testInfo.outputPath("utrace-timeline.png");

    if (durableOutputDirectory) {
      await mkdir(durableOutputDirectory, { recursive: true });
    }

    for (let run = 1; run <= repeatCount; run += 1) {
      const launchRequestedAt = performance.now();
      const browser = await chromium.launch({ headless: true });
      const browserConnectedAt = performance.now();

      try {
        const context = await browser.newContext({
          viewport: { width: 1600, height: 1000 },
        });
        const page = await context.newPage();
        await page.goto(benchmarkUrl);
        const fileInput = page.locator('input[type="file"]');
        await expect(fileInput).toHaveCount(1);
        await expect(page.getByRole("button", { name: /Drop a \.utrace/ })).toBeVisible();
        await waitForTwoAnimationFrames(page);
        const appReadyAt = performance.now();
        const preparationSnapshot = await browserSnapshot(page);

        await fileInput.setInputFiles(tracePath!);
        await expect(page.locator("[data-utrace-dashboard-ready]")).toBeVisible();
        await expect
          .poll(async () => completedSpanCount(await browserSnapshot(page), UTRACE_SPAN.load))
          .toBe(1);
        const dashboardReadyAt = performance.now();

        const timersTabRequestedAt = performance.now();
        await page.getByRole("button", { name: /^Timers/ }).click();
        const worstMarker = page.getByRole("button", { name: /Worst marker/ });
        await expect(worstMarker).toBeEnabled();
        await expect
          .poll(async () =>
            completedSpanCount(await browserSnapshot(page), UTRACE_SPAN.timersTabPaint),
          )
          .toBe(1);
        const timersTabReadyAt = performance.now();
        const frameRows = page.locator("[data-utrace-frame-row]");
        await expect.poll(async () => frameRows.count()).toBeGreaterThan(0);
        await expect.poll(async () => frameRows.count()).toBeLessThan(100);
        await expect(
          page.getByText(/[\d,]+ frames · scroll to browse the full capture/),
        ).toBeVisible();

        const markerRequestedAt = performance.now();
        await worstMarker.click();
        await expect(page.locator("[data-utrace-timeline-ready]")).toBeVisible();
        const firstTimerBar = page.locator(".timer-bar").first();
        await expect(firstTimerBar).toBeVisible();
        await expect
          .poll(async () => completedSpanCount(await browserSnapshot(page), UTRACE_SPAN.timelineLoad))
          .toBe(1);
        const timelineReadyAt = performance.now();

        await expect(firstTimerBar).toHaveAttribute("role", "button");
        await expect(page.locator('.timer-bar[tabindex="0"]')).toHaveCount(1);
        await firstTimerBar.focus();
        await page.keyboard.press("Enter");
        const inspector = page.locator("[data-timeline-inspector]");
        await expect(inspector).toBeVisible();
        await expect(inspector).toBeInViewport();
        await expect(inspector.getByText("Identity", { exact: true })).toBeVisible();
        await expect(inspector.getByText("Call context", { exact: true })).toBeVisible();
        await expect(inspector.getByRole("button", { name: "Focus span" })).toBeVisible();
        await expect(inspector.getByText(/Showing 2,500 of [\d,]+ matching spans/)).toBeVisible();
        await expect(firstTimerBar).toHaveAttribute("aria-pressed", "true");
        const nextOccurrence = inspector.getByRole("button", { name: /Next/ });
        await expect(nextOccurrence).toBeEnabled();
        await nextOccurrence.click();
        await expect(inspector.getByText(/occurrence 2 of \d+/)).toBeVisible();
        await inspector.getByRole("button", { name: /Previous/ }).click();
        await expect(inspector.getByText(/occurrence 1 of \d+/)).toBeVisible();
        await inspector.getByRole("button", { name: "Focus span" }).click();
        await expect(page.getByRole("button", { name: "Fit window" })).toBeVisible();
        await expect(inspector).toBeInViewport();
        await page.getByRole("button", { name: "Fit window" }).click();
        await expect(page.getByRole("button", { name: "Fit window" })).toHaveCount(0);

        const snapshot = await browserSnapshot(page);
        // Keep preparation work visible even when file selection resets load telemetry.
        snapshot.spans = [...new Map(
          [...preparationSnapshot.spans, ...snapshot.spans].map((span) => [span.span_id, span]),
        ).values()];
        expect(snapshot.spans
          .filter((span) => span.name === UTRACE_SPAN.sessionPush)
          .reduce((bytes, span) => bytes + Number(span.attributes["utrace.chunk_bytes"]), 0))
          .toBe(statSync(tracePath!).size);
        lastSnapshot = snapshot;
        const runtime = await page.evaluate(() => {
          const chromiumPerformance = performance as Performance & {
            memory?: {
              usedJSHeapSize: number;
              totalJSHeapSize: number;
            };
          };
          return {
            cross_origin_isolated: crossOriginIsolated,
            hardware_concurrency: navigator.hardwareConcurrency,
            user_agent: navigator.userAgent,
            used_js_heap_bytes: chromiumPerformance.memory?.usedJSHeapSize,
            total_js_heap_bytes: chromiumPerformance.memory?.totalJSHeapSize,
            timeline_bar_count: document.querySelectorAll(".timer-bar").length,
            svg_count: document.querySelectorAll("svg").length,
          };
        });

        samples.push({
          run,
          browser_process_launch_ms: roundMillis(browserConnectedAt - launchRequestedAt),
          page_navigation_and_empty_app_paint_ms: roundMillis(
            appReadyAt - browserConnectedAt,
          ),
          browser_process_to_app_ready_ms: roundMillis(appReadyAt - launchRequestedAt),
          app_ready_to_dashboard_paint_ms: roundMillis(dashboardReadyAt - appReadyAt),
          dashboard_to_timers_tab_paint_ms: roundMillis(
            timersTabReadyAt - timersTabRequestedAt,
          ),
          timers_tab_to_timeline_paint_ms: roundMillis(
            timelineReadyAt - timersTabReadyAt,
          ),
          marker_request_to_timeline_paint_ms: roundMillis(
            timelineReadyAt - markerRequestedAt,
          ),
          dashboard_to_timeline_paint_ms: roundMillis(timelineReadyAt - dashboardReadyAt),
          browser_process_to_dashboard_paint_ms: roundMillis(
            dashboardReadyAt - launchRequestedAt,
          ),
          browser_process_to_timeline_paint_ms: roundMillis(
            timelineReadyAt - launchRequestedAt,
          ),
          load_to_dashboard_paint_ms: onlyDuration(snapshot, UTRACE_SPAN.load),
          dashboard_paint_ms: onlyDuration(snapshot, UTRACE_SPAN.dashboardPaint),
          timers_tab_paint_ms: onlyDuration(snapshot, UTRACE_SPAN.timersTabPaint),
          file_stream_ms: onlyDuration(snapshot, UTRACE_SPAN.fileStream),
          wasm_push_ms: summedDuration(snapshot, UTRACE_SPAN.sessionPush),
          wasm_finish_and_serialize_ms: onlyDuration(snapshot, UTRACE_SPAN.sessionFinish),
          wasm_finish_phases_ms: finishPhaseDurations(snapshot),
          frame_to_timeline_paint_ms: onlyDuration(snapshot, UTRACE_SPAN.timelineLoad),
          timeline_query_and_serialize_ms: onlyDuration(snapshot, UTRACE_SPAN.timelineQuery),
          timeline_paint_ms: onlyDuration(snapshot, UTRACE_SPAN.timelinePaint),
          ...runtime,
        });

        if (run === repeatCount) {
          await inspector.screenshot({
            path: testInfo.outputPath("utrace-timeline-inspector.png"),
          });
          const frameScroller = page.locator(".frame-browser-wrap");
          const frameCount = Number(
            await frameScroller.getAttribute("data-utrace-frame-count"),
          );
          expect(frameCount).toBeGreaterThan(0);
          await frameScroller.evaluate((element) => {
            element.scrollTop = element.scrollHeight;
          });
          await expect
            .poll(async () =>
              frameRows.evaluateAll((elements) =>
                Math.max(
                  ...elements.map((element) =>
                    Number(element.getAttribute("data-utrace-frame-virtual-index")),
                  ),
                ),
              ),
            )
            .toBeGreaterThan(frameCount - 100);
          await expect.poll(async () => frameRows.count()).toBeLessThan(100);
          await frameScroller.evaluate((element) => {
            element.scrollTop = 0;
          });
          await expect
            .poll(async () =>
              frameRows.evaluateAll((elements) =>
                Math.min(
                  ...elements.map((element) =>
                    Number(element.getAttribute("data-utrace-frame-virtual-index")),
                  ),
                ),
              ),
            )
            .toBe(0);
          lastChromeTrace = await page.evaluate(() =>
            (window as BenchmarkWindow).__UTRACE_BENCHMARK__.chromeTrace(),
          );
          const eventKeys = new Set(lastChromeTrace.traceEvents.map((event) => `${event.name}:${event.ts}`));
          for (const span of preparationSnapshot.spans) {
            const ts = Math.round(span.start_time_unix_ms * 1000);
            if (eventKeys.has(`${span.name}:${ts}`)) continue;
            lastChromeTrace.traceEvents.push({
              name: span.name, cat: span.domain, ph: "X", ts,
              dur: Math.round(span.duration_ms * 1000), pid: 1, tid: span.domain, args: span.attributes,
            });
          }
          await page.screenshot({ path: screenshotPath, fullPage: true });
          if (durableOutputDirectory) {
            await inspector.screenshot({
              path: resolve(
                durableOutputDirectory,
                "browser-timeline-inspector.png",
              ),
            });
            await page.screenshot({
              path: resolve(durableOutputDirectory, "browser-timeline.png"),
              fullPage: true,
            });
          }
        }
      } finally {
        await browser.close();
      }
    }

    if (!lastSnapshot) throw new Error("browser benchmark produced no samples");
    if (!lastChromeTrace) throw new Error("browser benchmark produced no Chrome trace");
    const result: BrowserPerformanceReport = {
      schema_version: 3,
      status: "ok",
      trace: tracePath!,
      repeat_count: repeatCount,
      measurement_policy: {
        primary_scope: "application",
        excluded_from_primary: [
          "browser_process_launch",
          "page_navigation_and_empty_app_paint",
          "playwright_action_and_polling_intervals",
        ],
      },
      samples,
      application_medians_ms: {
        load_to_dashboard_paint: median(samples.map((sample) => sample.load_to_dashboard_paint_ms)),
        dashboard_paint: median(samples.map((sample) => sample.dashboard_paint_ms)),
        timers_tab_paint: median(samples.map((sample) => sample.timers_tab_paint_ms)),
        file_stream: median(samples.map((sample) => sample.file_stream_ms)),
        wasm_push: median(samples.map((sample) => sample.wasm_push_ms)),
        wasm_finish_and_serialize: median(
          samples.map((sample) => sample.wasm_finish_and_serialize_ms),
        ),
        wasm_finish_phases: medianFinishPhaseDurations(samples),
        frame_to_timeline_paint: median(
          samples.map((sample) => sample.frame_to_timeline_paint_ms),
        ),
        timeline_query_and_serialize: median(
          samples.map((sample) => sample.timeline_query_and_serialize_ms),
        ),
        timeline_paint: median(samples.map((sample) => sample.timeline_paint_ms)),
      },
      diagnostic_medians_ms: {
        browser_process_launch: median(
          samples.map((sample) => sample.browser_process_launch_ms),
        ),
        page_navigation_and_empty_app_paint: median(
          samples.map((sample) => sample.page_navigation_and_empty_app_paint_ms),
        ),
        browser_process_to_app_ready: median(
          samples.map((sample) => sample.browser_process_to_app_ready_ms),
        ),
        app_ready_to_dashboard_paint: median(
          samples.map((sample) => sample.app_ready_to_dashboard_paint_ms),
        ),
        dashboard_to_timers_tab_paint: median(
          samples.map((sample) => sample.dashboard_to_timers_tab_paint_ms),
        ),
        timers_tab_to_timeline_paint: median(
          samples.map((sample) => sample.timers_tab_to_timeline_paint_ms),
        ),
        marker_request_to_timeline_paint: median(
          samples.map((sample) => sample.marker_request_to_timeline_paint_ms),
        ),
        dashboard_to_timeline_paint: median(
          samples.map((sample) => sample.dashboard_to_timeline_paint_ms),
        ),
        browser_process_to_dashboard_paint: median(
          samples.map((sample) => sample.browser_process_to_dashboard_paint_ms),
        ),
        browser_process_to_timeline_paint: median(
          samples.map((sample) => sample.browser_process_to_timeline_paint_ms),
        ),
      },
    };
    const reportPath = testInfo.outputPath("utrace-browser-performance.json");
    await writeFile(reportPath, `${JSON.stringify(result, null, 2)}\n`, "utf8");
    if (durableOutputDirectory) {
      await writeFile(
        resolve(durableOutputDirectory, "browser-performance.json"),
        `${JSON.stringify(result, null, 2)}\n`,
        "utf8",
      );
    }
    await testInfo.attach("browser-performance", {
      path: reportPath,
      contentType: "application/json",
    });

    const chromeTracePath = testInfo.outputPath("utrace-browser-trace.json");
    await writeFile(chromeTracePath, `${JSON.stringify(lastChromeTrace)}\n`, "utf8");
    if (durableOutputDirectory) {
      await writeFile(
        resolve(durableOutputDirectory, "browser-chrome-trace.json"),
        `${JSON.stringify(lastChromeTrace)}\n`,
        "utf8",
      );
    }
    await testInfo.attach("browser-chrome-trace", {
      path: chromeTracePath,
      contentType: "application/json",
    });
  });
});

type BrowserPerformanceSample = {
  run: number;
  browser_process_launch_ms: number;
  page_navigation_and_empty_app_paint_ms: number;
  browser_process_to_app_ready_ms: number;
  app_ready_to_dashboard_paint_ms: number;
  dashboard_to_timers_tab_paint_ms: number;
  timers_tab_to_timeline_paint_ms: number;
  marker_request_to_timeline_paint_ms: number;
  dashboard_to_timeline_paint_ms: number;
  browser_process_to_dashboard_paint_ms: number;
  browser_process_to_timeline_paint_ms: number;
  load_to_dashboard_paint_ms: number;
  dashboard_paint_ms: number;
  timers_tab_paint_ms: number;
  file_stream_ms: number;
  wasm_push_ms: number;
  wasm_finish_and_serialize_ms: number;
  wasm_finish_phases_ms: WasmFinishPhaseDurations;
  frame_to_timeline_paint_ms: number;
  timeline_query_and_serialize_ms: number;
  timeline_paint_ms: number;
  cross_origin_isolated: boolean;
  hardware_concurrency: number;
  user_agent: string;
  used_js_heap_bytes?: number;
  total_js_heap_bytes?: number;
  timeline_bar_count: number;
  svg_count: number;
};

type BrowserPerformanceReport = {
  schema_version: 3;
  status: "ok";
  trace: string;
  repeat_count: number;
  measurement_policy: {
    primary_scope: "application";
    excluded_from_primary: [
      "browser_process_launch",
      "page_navigation_and_empty_app_paint",
      "playwright_action_and_polling_intervals",
    ];
  };
  samples: BrowserPerformanceSample[];
  application_medians_ms: {
    load_to_dashboard_paint: number;
    dashboard_paint: number;
    timers_tab_paint: number;
    file_stream: number;
    wasm_push: number;
    wasm_finish_and_serialize: number;
    wasm_finish_phases: WasmFinishPhaseDurations;
    frame_to_timeline_paint: number;
    timeline_query_and_serialize: number;
    timeline_paint: number;
  };
  diagnostic_medians_ms: {
    browser_process_launch: number;
    page_navigation_and_empty_app_paint: number;
    browser_process_to_app_ready: number;
    app_ready_to_dashboard_paint: number;
    dashboard_to_timers_tab_paint: number;
    timers_tab_to_timeline_paint: number;
    marker_request_to_timeline_paint: number;
    dashboard_to_timeline_paint: number;
    browser_process_to_dashboard_paint: number;
    browser_process_to_timeline_paint: number;
  };
};

type BenchmarkWindow = Window & {
  __UTRACE_BENCHMARK__: {
    snapshot: () => UtraceBenchmarkSnapshot;
    chromeTrace: () => { traceEvents: ChromeTraceEvent[] };
  };
};

async function browserSnapshot(
  page: Page,
): Promise<UtraceBenchmarkSnapshot> {
  return page.evaluate(() =>
    (window as BenchmarkWindow).__UTRACE_BENCHMARK__.snapshot(),
  );
}

async function waitForTwoAnimationFrames(page: Page): Promise<void> {
  await page.evaluate(
    () =>
      new Promise<void>((resolvePaint) => {
        requestAnimationFrame(() => requestAnimationFrame(() => resolvePaint()));
      }),
  );
}

function completedSpanCount(snapshot: UtraceBenchmarkSnapshot, name: UtraceSpanName): number {
  return snapshot.spans.filter((span) => span.name === name && span.status === "ok").length;
}

function onlyDuration(snapshot: UtraceBenchmarkSnapshot, name: UtraceSpanName): number {
  const spans = snapshot.spans.filter((span) => span.name === name);
  if (spans.length !== 1) {
    throw new Error(`expected exactly one ${name} span, found ${spans.length}`);
  }
  return spans[0]!.duration_ms;
}

function summedDuration(snapshot: UtraceBenchmarkSnapshot, name: UtraceSpanName): number {
  const spans = snapshot.spans.filter((span) => span.name === name);
  if (spans.length === 0) throw new Error(`expected at least one ${name} span`);
  return spans.reduce((total, span) => total + span.duration_ms, 0);
}

type WasmFinishPhaseDurations = Record<WasmFinishPhase, number>;

function finishPhaseDurations(snapshot: UtraceBenchmarkSnapshot): WasmFinishPhaseDurations {
  return Object.fromEntries(
    WASM_FINISH_PHASES.map((phase) => [phase, onlyDuration(snapshot, finishPhaseSpanName(phase))]),
  ) as WasmFinishPhaseDurations;
}

function medianFinishPhaseDurations(
  samples: BrowserPerformanceSample[],
): WasmFinishPhaseDurations {
  return Object.fromEntries(
    WASM_FINISH_PHASES.map((phase) => [
      phase,
      median(samples.map((sample) => sample.wasm_finish_phases_ms[phase])),
    ]),
  ) as WasmFinishPhaseDurations;
}

function median(values: number[]): number {
  if (values.length === 0) throw new Error("cannot calculate an empty median");
  const ordered = [...values].sort((left, right) => left - right);
  const middle = Math.floor(ordered.length / 2);
  return ordered.length % 2 === 1
    ? ordered[middle]!
    : (ordered[middle - 1]! + ordered[middle]!) / 2;
}

function parseRepeatCount(value: string | undefined): number {
  if (value == null) return 3;
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed < 1 || parsed > 10) {
    throw new Error("UTRACE_E2E_REPEAT must be an integer from 1 through 10");
  }
  return parsed;
}

function requireBaseUrl(value: unknown): string {
  if (typeof value !== "string") {
    throw new Error("Playwright project must provide a baseURL");
  }
  return value;
}

function roundMillis(value: number): number {
  return Math.round(value * 1_000) / 1_000;
}
