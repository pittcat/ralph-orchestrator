import { defineConfig, devices } from "@playwright/test";

const MONITOR_PORT = process.env.MONITOR_E2E_PORT ?? "3310";

export default defineConfig({
  testDir: "./e2e",
  fullyParallel: true,
  forbidOnly: !!process.env.CI,
  retries: process.env.CI ? 2 : 0,
  workers: process.env.CI ? 1 : undefined,
  reporter: "list",
  use: {
    baseURL: "http://localhost:5173",
    trace: "on-first-retry",
  },
  projects: [
    {
      name: "chromium",
      use: { ...devices["Desktop Chrome"] },
    },
  ],
  // Plan5-U5: real monitor (fake-ralph fixture on PATH) + vite dev
  // proxying /api/monitor → the monitor, same-origin like production.
  webServer: [
    {
      command: "npx tsx e2e/helpers/monitor-server.ts",
      url: `http://127.0.0.1:${MONITOR_PORT}/api/monitor/v1/health`,
      reuseExistingServer: false,
      timeout: 60_000,
      env: { MONITOR_E2E_PORT: MONITOR_PORT },
    },
    {
      command: "npm run dev -- --port 5173",
      url: "http://localhost:5173",
      reuseExistingServer: !process.env.CI,
      timeout: 60_000,
      env: {
        ...process.env,
        RALPH_MONITOR_TARGET: `http://127.0.0.1:${MONITOR_PORT}`,
      },
    },
  ],
});
