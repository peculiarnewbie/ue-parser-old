import { defineConfig } from "@playwright/test";

const benchmarkPort = 4174;
const benchmarkBaseUrl = `http://127.0.0.1:${benchmarkPort}`;

export default defineConfig({
  testDir: "./e2e",
  outputDir: "./test-results",
  fullyParallel: false,
  workers: 1,
  timeout: 180_000,
  expect: { timeout: 120_000 },
  reporter: [["line"]],
  use: {
    baseURL: benchmarkBaseUrl,
    headless: true,
    viewport: { width: 1600, height: 1000 },
    trace: "retain-on-failure",
  },
  webServer: {
    command: `npx vite preview --host 127.0.0.1 --port ${benchmarkPort} --strictPort`,
    url: `${benchmarkBaseUrl}/utrace`,
    reuseExistingServer: false,
    timeout: 120_000,
  },
});
