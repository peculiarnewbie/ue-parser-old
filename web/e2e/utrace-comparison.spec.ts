import { expect as baseExpect, test } from "@playwright/test";
import { readFileSync } from "node:fs";

const expect = baseExpect.configure({ timeout: 15_000 });
test.use({ actionTimeout: 15_000 });
const workTrace = Buffer.from(readFileSync(new URL("../../tests/fixtures/tiny/comparison-work.utrace.hex", import.meta.url), "utf8")
  .replace(/#.*$/gm, "").replace(/\s/g, ""), "hex");
const capture = (name: string) => ({ name, mimeType: "application/octet-stream", buffer: workTrace });

test("comparison route starts with independent baseline and candidate slots", async ({ page }) => {
  await page.goto("/utrace/compare");

  await expect(page.getByRole("heading", { name: /Compare the outcome/ })).toBeVisible();
  await expect(page.getByLabel("Baseline capture")).toBeVisible();
  await expect(page.getByLabel("Candidate capture")).toBeVisible();
  await expect(page.locator(".compare-capture input[type=file]")).toHaveCount(2);
  await expect(page.getByText("Start with distributions—not synchronized clocks.")).toBeVisible();
  await expect(page.locator(".nav a.active")).toHaveText("compare");
});

test("a failed baseline parse leaves the candidate slot available", async ({ page }) => {
  await page.goto("/utrace/compare");
  await page
    .locator(".compare-capture-baseline input[type=file]")
    .setInputFiles({
      name: "invalid.utrace",
      mimeType: "application/octet-stream",
      buffer: Buffer.from([1, 2, 3, 4]),
    });

  await expect(page.getByText("Could not parse invalid.utrace")).toBeVisible();
  await expect(page.getByRole("heading", { name: "Choose candidate .utrace" })).toBeVisible();
  await expect(page.locator(".compare-orbit")).toHaveCount(0);
});

for (const runtime of ["threaded", "single-thread"] as const) {
  test(`compares real ${runtime} WASM captures, edits ranges, and releases replaced sessions`, async ({ page }, testInfo) => {
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    if (runtime === "single-thread") {
      await page.route("**/utrace/compare", async (route) => {
        const response = await route.fetch();
        const headers = response.headers();
        delete headers["cross-origin-opener-policy"];
        delete headers["cross-origin-embedder-policy"];
        await route.fulfill({ response, headers });
      });
    }
    await page.goto("/utrace/compare");
    expect(await page.evaluate(() => crossOriginIsolated)).toBe(runtime === "threaded");
    const baseline = page.getByLabel("Baseline capture", { exact: true });
    const candidate = page.getByLabel("Candidate capture", { exact: true });
    await baseline.locator("input[type=file]").setInputFiles(capture("baseline.utrace"));
    await expect(baseline.locator(".compare-capture-state")).toBeVisible();
    await candidate.locator("input[type=file]").setInputFiles(capture("candidate.utrace"));
    await expect(page.getByRole("heading", { name: "No material tail shift" })).toBeVisible();
    await expect(page.locator(".compare-observation-counts strong")).toHaveText(["3", "3"]);
    await page.screenshot({ path: testInfo.outputPath("comparison-outcome.png"), fullPage: true });

    await page.getByRole("button", { name: /^CPU attribution/ }).click();
    await page.getByRole("button", { name: "all", exact: true }).click();
    await expect(page.locator("tbody")).toContainText("Work");
    await expect(page.locator("tbody tr").first().locator("td").nth(2)).toHaveText("100");
    await page.getByRole("button", { name: "Explicit ranges", exact: true }).click();
    const leftRange = page.locator(".compare-range-fields fieldset").first();
    await leftRange.getByLabel("baseline range start").fill("50");
    await leftRange.getByLabel("baseline range end").fill("250");
    await expect(leftRange.getByLabel("baseline range start")).toHaveValue("50");
    await expect(leftRange.locator("small")).toContainText("200 ms selected");
    await page.getByRole("button", { name: "Analyze paired ranges" }).click();
    await page.getByRole("button", { name: "all", exact: true }).click();
    await expect(page.locator("tbody tr").first().locator("td").nth(2)).toHaveText("1,000");
    await page.locator("tbody tr").first().click();
    await expect(page.getByRole("heading", { name: "A · Baseline" })).toBeVisible();
    await expect(page.getByRole("heading", { name: "B · Candidate" })).toBeVisible();

    await page.getByRole("button", { name: "Swap baseline and candidate" }).click();
    await expect(baseline.locator(".compare-filename")).toHaveText("candidate.utrace");
    await expect(page.getByRole("heading", { name: "Select a CPU attribution row." })).toBeVisible();
    await page.getByRole("button", { name: /^CPU attribution/ }).click();
    await expect(leftRange.getByLabel("baseline range start")).toHaveValue("0");
    await baseline.locator("input[type=file]").setInputFiles(capture("replacement.utrace"));
    await expect(baseline.locator(".compare-filename")).toHaveText("replacement.utrace");
    await expect(candidate.locator(".compare-filename")).toHaveText("baseline.utrace");
    await expect(page.getByRole("heading", { name: "No material tail shift" })).toBeVisible();

    // Navigate within the SPA so this exercises cleanup in the same Worker.
    await page.locator(".nav a[href='/uasset']").click();
    await page.locator(".nav a[href='/utrace/compare']").click();
    await baseline.locator("input[type=file]").setInputFiles(capture("reopened-a.utrace"));
    await expect(baseline.locator(".compare-capture-state")).toBeVisible();
    await candidate.locator("input[type=file]").setInputFiles(capture("reopened-b.utrace"));
    await expect(page.getByRole("heading", { name: "No material tail shift" })).toBeVisible();
    expect(errors).toEqual([]);
  });
}
