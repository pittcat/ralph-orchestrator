/**
 * Monitor recovery e2e (Plan3-U4 S6/S7/S10 journey): the page opens on
 * a live SSE stream → the stream is dropped (route.abort) → the badge
 * degrades to reconnecting → restoring the stream converges back to
 * connected with the same data. Route-intercept per plan §19 —
 * `route.fulfill` yields a complete (then-closed) stream, so the
 * driver treats each reconnect as snapshot+tail; assertions use
 * expect.poll to ride out transient states.
 */

import { expect, test, type Page } from "@playwright/test";

const RUN = {
  globalRunId: "m:p1:loop-1",
  projectId: "p1",
  runId: "loop-1",
  kind: "worktree",
  state: "running",
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

const SNAPSHOT = {
  watermark: 3,
  generated_at: "2026-10-28T12:00:00Z",
  projects: [],
  runs: [RUN],
};

async function mockApi(page: Page): Promise<{ drop: () => void; heal: () => void }> {
  let dropped = false;
  await page.route("**/api/monitor/v1/**", async (route) => {
    const url = route.request().url();
    const json = (body: unknown) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(body),
      });
    if (url.includes("/stream") || url.includes("/snapshot")) {
      if (dropped) return route.abort();
      if (url.includes("/snapshot")) return json(SNAPSHOT);
      return route.fulfill({
        status: 200,
        contentType: "text/event-stream",
        body: ": hb\n\n",
      });
    }
    if (url.includes("/projects")) return json({ projects: [] });
    if (url.includes("/runs"))
      return json({ runs: [RUN], total: 1, watermark: "t" });
    return json({});
  });
  return {
    drop: () => {
      dropped = true;
    },
    heal: () => {
      dropped = false;
    },
  };
}

test.describe("Monitor stream recovery", () => {
  test("connects, degrades on drop, recovers on heal", async ({ page }) => {
    const ctl = await mockApi(page);
    await page.goto("/monitor");

    const badge = page.getByTestId("connection-badge");
    await expect(badge).toBeVisible();
    await expect
      .poll(() => badge.getAttribute("data-status"))
      .toBe("connected");

    ctl.drop();
    await expect
      .poll(() => badge.getAttribute("data-status"), { timeout: 15_000 })
      .toMatch(/reconnecting|offline/);

    ctl.heal();
    await expect
      .poll(() => badge.getAttribute("data-status"), { timeout: 15_000 })
      .toBe("connected");

    // Data survived the drop — the run row still renders.
    await expect(page.getByText("loop-1")).toBeVisible();
  });
});
