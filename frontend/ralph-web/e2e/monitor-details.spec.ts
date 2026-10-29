/**
 * Monitor detail journey e2e (Plan2-U5, route-intercept — same
 * pattern as monitor-overview.spec.ts): enter detail → stage strip +
 * tabs → timeline row select → unit filter via graph list → artifact
 * slot open → markdown preview.
 */

import { expect, test, type Page } from "@playwright/test";

const GID = "m:p1:loop-1";

const RUN = {
  globalRunId: GID,
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
  planPath: "docs/plans/p.md",
  startedAt: "2026-10-28T10:00:00Z",
  terminal: null,
  lastEventAt: "2026-10-28T10:30:00Z",
  lastObservedAt: "2026-10-28T12:00:00Z",
  manifestPath: null,
  stale: false,
  presetDescriptor: {
    schema_version: "monitor-descriptor/v1",
    stages: [
      { id: "plan", label: "Plan", topics: ["plan.done"] },
      { id: "execute", label: "Execute", topics: ["work.start", "work.done"] },
    ],
    artifacts: [
      { id: "summary", path_glob: ".ralph/agent/summary.md", kind: "markdown" },
      { id: "missing", path_glob: ".ralph/agent/nope.md", kind: "markdown" },
    ],
  },
};

const EVENTS = {
  events: [
    { seq: 1, ts: "2026-10-28T10:00:01Z", iteration: 1, hat: "bootstrap", topic: "forge.start", payload: "{}", truncated: false },
    { seq: 2, ts: "2026-10-28T10:00:02Z", iteration: 1, hat: "executor", topic: "work.start", payload: "{}", truncated: false },
    { seq: 3, ts: "2026-10-28T10:00:03Z", iteration: 1, hat: "forge", topic: "forge.unit.ready", payload: '{"unit":"U1"}', truncated: false },
    { seq: 4, ts: "2026-10-28T10:00:04Z", iteration: 1, hat: "forge", topic: "forge.unit.executed", payload: '{"unit":"U1"}', truncated: false },
  ],
  next_cursor: 4,
  has_more: false,
  watermark: "t",
  evidence_timestamps: {
    last_event_at: "2026-10-28T10:00:04Z",
    last_observed_at: "2026-10-28T12:00:00Z",
    terminal_at: null,
    collected_at: "2026-10-28T12:01:00Z",
  },
};

const PROGRESS = {
  stage: "execute",
  stage_label: "Execute",
  stage_history: [{ stage: "plan", at_seq: 1, round: 1 }, { stage: "execute", at_seq: 2, round: 1 }],
  current_hat: "forge",
  iteration: 1,
  elapsed_ms: 1800,
  units: { done: 1, total: 1, source: "forge-events" },
  denominator: "stages",
};

const GRAPH = {
  nodes: [{ unit: "U1", status: "executed", last_seq: 4 }],
  edges: [],
  source: "forge-events",
  scheduler: null,
  illegal_transitions: 0,
};

const ARTIFACTS = {
  artifacts: [
    { id: "summary", kind: "markdown", path: ".ralph/agent/summary.md", status: "present", size: 22, mtime: "2026-10-28T10:00:05Z" },
    { id: "missing", kind: "markdown", status: "not_generated" },
  ],
};

async function mockApi(page: Page) {
  await page.route("**/api/monitor/v1/**", (route) => {
    const url = route.request().url();
    const json = (body: unknown) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(body),
      });
    if (url.includes("/artifacts/") && url.endsWith("/content")) {
      return json({ id: "summary", kind: "markdown", content: "# Run summary\nAll **good**.", truncated: false });
    }
    if (url.includes("/artifacts")) return json(ARTIFACTS);
    if (url.includes("/progress")) return json(PROGRESS);
    if (url.includes("/graph")) return json(GRAPH);
    if (url.includes("/events")) {
      const u = new URL(url);
      const q = u.searchParams.get("q");
      const events = q
        ? EVENTS.events.filter((e) => (e.payload ?? "").includes(q) || (e.topic ?? "").includes(q))
        : EVENTS.events;
      return json({ ...EVENTS, events, next_cursor: events.at(-1)?.seq ?? 0 });
    }
    if (url.includes("/projects")) return json({ projects: [{ id: "p1", canonicalPath: "/w/alpha", displayName: "alpha", createdAt: "t" }] });
    if (url.includes("/runs/")) {
      return json({
        run: {
          ...RUN,
          evidenceTimestamps: {
            lastEventAt: RUN.lastEventAt,
            lastObservedAt: RUN.lastObservedAt,
            terminalAt: null,
            collectedAt: "2026-10-28T12:01:00Z",
          },
        },
      });
    }
    return route.fallback();
  });
}

test.describe("Monitor run details", () => {
  test("journey: tabs → timeline select → graph unit filter → artifact preview", async ({ page }) => {
    await mockApi(page);
    await page.goto(`/monitor/runs/${encodeURIComponent(GID)}`);
    // SSE reconnect retries keep the network busy — networkidle never
    // settles; semantic waits below cover readiness.
    await page.waitForLoadState("domcontentloaded");

    // Header + stage strip (S1).
    await expect(page.getByTestId("run-detail-id")).toHaveText("loop-1");
    await expect(page.getByTestId("detail-current-hat")).toHaveText("forge");
    await expect(page.getByTestId("stage-node-execute")).toHaveAttribute("data-state", "current");

    // Timeline tab → rows; select seq → URL carries seq (S4).
    await page.getByTestId("tab-timeline").click();
    await expect(page.getByTestId("event-row-4")).toBeVisible();
    await page.getByTestId("event-row-2").click();
    await expect(page).toHaveURL(/tab=timeline/);
    await expect(page).toHaveURL(/seq=2/);

    // Logs tab → search filters rows (S4).
    await page.getByTestId("tab-logs").click();
    await page.getByTestId("log-search").fill("forge.unit.ready");
    await expect(page.getByTestId("log-list")).toContainText("forge.unit.ready");
    await expect(page.getByTestId("log-list")).not.toContainText("work.start");

    // Graph tab → list view → click unit → timeline filtered (S5).
    await page.getByTestId("tab-graph").click();
    await page.getByTestId("graph-view-list").click();
    await page.getByRole("button", { name: "U1" }).click();
    await expect(page).toHaveURL(/tab=timeline/);
    await expect(page).toHaveURL(/unit=U1/);
    await expect(page.getByTestId("event-row-3")).toBeVisible();
    await expect(page.getByTestId("event-row-2")).toHaveCount(0);

    // Artifacts tab → missing slot shows 尚未生成; open summary (S6/S7).
    await page.getByTestId("tab-artifacts").click();
    await expect(page.getByTestId("artifact-status-missing")).toHaveText("尚未生成");
    await page.getByTestId("artifact-summary").click();
    await expect(page.getByTestId("markdown-viewer")).toContainText("Run summary");
    await expect(page).toHaveURL(/artifact=summary/);
  });
});
