import { expect, test, type Locator } from "@playwright/test";
import { existsSync } from "node:fs";
import { resolve } from "node:path";

const repositoryRoot = resolve(import.meta.dirname, "..", "..");
const requestedTrace = process.env.UTRACE_E2E_TRACE
  ? resolve(process.env.UTRACE_E2E_TRACE)
  : resolve(
      repositoryRoot,
      "..",
      "tools",
      ".local",
      "traces",
      "funguys-build-30229-dev-20260807.utrace",
    );
const tracePath = existsSync(requestedTrace) ? requestedTrace : null;

test.describe("UTrace table virtualization", () => {
  test.skip(
    tracePath == null,
    "requires a real .utrace fixture; set UTRACE_E2E_TRACE",
  );

  test("renders large analysis tables on demand", async ({ page }) => {
    test.setTimeout(120_000);
    await page.goto("/utrace");
    await page.locator('input[type="file"]').setInputFiles(tracePath!);
    await expect(page.locator("[data-utrace-dashboard-ready]")).toBeVisible();

    await page.getByRole("button", { name: /^CPU/ }).click();
    const scopePanel = panelWithHeading(
      page.locator("main"),
      "Capture-wide scope rollup",
    );
    const scopeBody = scopePanel.locator("tbody[data-virtualized-total-rows]");
    const totalScopes = await virtualizedRowCount(scopeBody);
    expect(totalScopes).toBeGreaterThan(10_000);
    const initialMountedScopes = await mountedRowCount(scopeBody);
    expect(initialMountedScopes).toBeGreaterThan(0);
    expect(initialMountedScopes).toBeLessThan(100);

    const scopeScroller = scopePanel.locator(".datatable-wrap");
    await scopeScroller.evaluate((element) => {
      element.scrollTop = element.scrollHeight;
    });
    await expect
      .poll(() => highestMountedIndex(scopeBody), { timeout: 10_000 })
      .toBeGreaterThan(totalScopes - 100);
    expect(await mountedRowCount(scopeBody)).toBeLessThan(100);

    await scopePanel.getByPlaceholder("scope name…").fill("WaitForTasks");
    await expect.poll(() => virtualizedRowCount(scopeBody)).toBeGreaterThan(0);
    expect(await virtualizedRowCount(scopeBody)).toBeLessThan(10);
    await expect(scopeScroller).toHaveJSProperty("scrollTop", 0);
    await expect(scopeBody).toContainText("WaitForTasks");

    await page.getByRole("button", { name: /^Metrics/ }).click();
    const counterPanel = panelWithHeading(
      page.locator("main"),
      "Counter catalog",
    );
    const counterBody = counterPanel.locator("tbody[data-virtualized-total-rows]");
    expect(await virtualizedRowCount(counterBody)).toBeGreaterThan(200);
    expect(await mountedRowCount(counterBody)).toBeGreaterThan(0);
    expect(await mountedRowCount(counterBody)).toBeLessThan(100);

    await page.getByRole("button", { name: /^GPU/ }).click();
    const gpuFramePanel = panelWithHeading(
      page.locator("main"),
      "Hottest queue-local frames",
    );
    const gpuFrameBody = gpuFramePanel.locator("tbody[data-virtualized-total-rows]");
    expect(await virtualizedRowCount(gpuFrameBody)).toBe(200);
    expect(await mountedRowCount(gpuFrameBody)).toBeGreaterThan(0);
    expect(await mountedRowCount(gpuFrameBody)).toBeLessThan(100);

    await page.getByRole("button", { name: /^Timers/ }).click();
    const rangeStart = page.getByLabel("Range start");
    const lastFrameIndex = Number(await rangeStart.getAttribute("max"));
    expect(lastFrameIndex).toBeGreaterThan(1);
    await rangeStart.evaluate((element, value) => {
      const input = element as HTMLInputElement;
      input.value = String(value);
      input.dispatchEvent(new Event("input", { bubbles: true }));
      input.dispatchEvent(new Event("change", { bubbles: true }));
    }, Math.floor(lastFrameIndex / 2));

    const sharedRange = page.locator("[data-analysis-range]");
    await expect(sharedRange).toBeVisible();
    await expect(sharedRange).toContainText("Shared frame range");

    await page.getByRole("button", { name: /^CPU/ }).click();
    await expect(sharedRange).toBeVisible();
    await expect(
      page.getByRole("heading", { name: "Scopes in selected frames" }),
    ).toBeVisible();

    await sharedRange.getByRole("button", { name: "Clear range" }).click();
    await expect(sharedRange).toBeHidden();
    await expect(
      page.getByRole("heading", { name: "Capture-wide scope rollup" }),
    ).toBeVisible();
  });
});

function panelWithHeading(root: Locator, heading: string): Locator {
  return root.locator("section.panel").filter({ hasText: heading }).first();
}

async function virtualizedRowCount(body: Locator): Promise<number> {
  return Number(await body.getAttribute("data-virtualized-total-rows"));
}

async function mountedRowCount(body: Locator): Promise<number> {
  return body.locator("tr[data-virtualized-row]").count();
}

async function highestMountedIndex(body: Locator): Promise<number> {
  return body.locator("tr[data-virtualized-row]").evaluateAll((rows) =>
    Math.max(
      ...rows.map((row) => Number(row.getAttribute("data-virtualized-index"))),
    ),
  );
}
