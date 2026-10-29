/**
 * Monitor overview e2e (route-intercept, per plan U5 §6 decision —
 * same pattern as builder.spec.ts; the full-stack fake-ralph journey
 * lands in Plan5 U5).
 *
 * Covers: two-project fixture, state filter via URL, table
 * convergence, row → detail navigation with evidence timestamps,
 * browser back restoring filters.
 */

import { expect, test, type Page } from "@playwright/test";

const PROJECTS = [
  { id: "p1", canonicalPath: "/w/alpha", displayName: "alpha", createdAt: "t" },
  { id: "p2", canonicalPath: "/w/beta", displayName: "beta", createdAt: "t" },
];

const BASE_RUN = {
  projectId: "p1",
  runId: "loop-1",
  kind: "worktree",
  stateEvidence: {},
  cwd: "/w/alpha",
  repoRoot: "/w/alpha",
  worktreePath: null,
  promptPreview: "do the thing",
  promptSha256: null,
  preset: "ce-executor-pipeline",
  planPath: null,
  startedAt: "2026-10-28T10:00:00Z",
  terminal: null,
  lastEventAt: "2026-10-28T10:30:00Z",
  lastObservedAt: "2026-10-28T12:00:00Z",
  manifestPath: null,
  stale: false,
};

const RUNS = [
  { ...BASE_RUN, globalRunId: "m:p1:loop-1", state: "running" },
  {
    ...BASE_RUN,
    globalRunId: "m:p1:loop-2",
    runId: "loop-2",
    state: "exited_failed",
    projectId: "p1",
    terminal: { reason: "backend_exit", at: "2026-10-28T11:00:00Z" },
  },
  {
    ...BASE_RUN,
    globalRunId: "m:p2:primary-1",
    runId: "primary-1",
    state: "exited_success",
    projectId: "p2",
    preset: null,
  },
];

async function mockMonitorApi(page: Page): Promise<string[]> {
  const runUrls: string[] = [];
  await page.route("**/api/monitor/v1/**", async (route) => {
    const url = route.request().url();
    const json = (body: unknown) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(body),
      });
    if (url.includes("/stream")) {
      return route.fulfill({
        status: 200,
        contentType: "text/event-stream",
        body: ": hb\n\n",
      });
    }
    if (url.includes("/snapshot")) {
      return json({
        watermark: 0,
        generated_at: "2026-10-28T12:00:00Z",
        projects: [],
        runs: [],
      });
    }
    if (url.includes("/projects/") && url.endsWith("/health")) {
      return json({
        collector: {
          status: "ok",
          consecutive_failures: 0,
          last_collected_at: "t",
        },
        degraded_runs: [],
      });
    }
    if (url.includes("/projects")) return json({ projects: PROJECTS });
    if (/\/runs\/[^?]+/.test(url)) {
      const id = decodeURIComponent(url.split("/runs/")[1].split("?")[0]);
      const run = RUNS.find((r) => r.globalRunId === id) ?? RUNS[1];
      return json({
        run: {
          ...run,
          evidenceTimestamps: {
            lastEventAt: run.lastEventAt,
            lastObservedAt: run.lastObservedAt,
            terminalAt: run.terminal?.at ?? null,
            collectedAt: "2026-10-28T12:01:00Z",
          },
        },
      });
    }
    if (url.includes("/runs")) {
      runUrls.push(url);
      const stateFilter = new URL(url).searchParams.get("state");
      const runs = stateFilter
        ? RUNS.filter((r) => stateFilter.split(",").includes(r.state))
        : RUNS;
      return json({ runs, total: runs.length, watermark: "t" });
    }
    return route.fallback();
  });
  return runUrls;
}

test.describe("Monitor overview", () => {
  test("filters via URL, opens detail, back preserves filters", async ({ page }) => {
    const runUrls = await mockMonitorApi(page);
    await page.goto("/monitor");
    await page.waitForLoadState("networkidle");

    // Table renders all three runs.
    const table = page.getByRole("table");
    await expect(table).toBeVisible();
    await expect(page.getByText("loop-1")).toBeVisible();
    await expect(page.getByText("primary-1")).toBeVisible();

    // Filter to failed via the state dropdown → URL + converged table.
    await page.getByTestId("state-filter").locator("summary").click();
    await page.getByRole("checkbox", { name: "exited_failed" }).click();
    await expect(page).toHaveURL(/state=exited_failed/);
    await expect
      .poll(() => runUrls.some((u) => u.includes("state=exited_failed")))
      .toBe(true);
    await expect(page.getByText("loop-2")).toBeVisible();
    await expect(page.getByText("loop-1")).toHaveCount(0);

    // Row click → detail: state badge + evidence timestamps.
    await page.getByText("loop-2").click();
    await expect(page.getByTestId("run-detail-id")).toHaveText("loop-2");
    await expect(page.getByTestId("run-state-badge")).toHaveAttribute(
      "data-state",
      "exited_failed",
    );
    await expect(page.getByTestId("evidence-timestamps")).toContainText("terminal.at");

    // Back → filters still applied.
    await page.goBack();
    await expect(page).toHaveURL(/state=exited_failed/);
    await expect(page.getByText("loop-2")).toBeVisible();
    await expect(page.getByText("loop-1")).toHaveCount(0);
  });
});
