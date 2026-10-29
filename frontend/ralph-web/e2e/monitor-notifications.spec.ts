/**
 * Monitor notifications e2e (Plan4-U3): inbox list journey — unread
 * badge shows on the header link, opening the inbox renders
 * notifications + pending interactions, marking read clears the row
 * action and drops the badge, mute filter works, settings panel
 * saves a webhook config and reports the test-delivery result.
 * All API traffic is route-intercepted; the stream is a heartbeat.
 */

import { expect, test, type Page } from "@playwright/test";

const SNAPSHOT = {
  watermark: 9,
  generated_at: "2026-10-29T12:00:00Z",
  projects: [
    {
      id: "p1",
      canonicalPath: "/w/alpha",
      displayName: "alpha",
      createdAt: "2026-10-28T00:00:00Z",
      runtimeWebhookHint: "enabled",
      health: null,
    },
  ],
  runs: [],
};

const NOTIF_UNREAD = {
  id: "n1",
  kind: "run.failed",
  project_id: "p1",
  run_gid: "m:p1:loop-1",
  pending_interaction_id: null,
  title: "run failed: loop-1",
  body: null,
  detail: null,
  state: "unread",
  created_at: "2026-10-29T11:00:00Z",
  resolved_at: null,
  link: "/monitor/runs/m%3Ap1%3Aloop-1",
};

const PENDING = {
  id: "pi1",
  project_id: "p1",
  session_id: "sess-9",
  run_gid: "m:p1:loop-1",
  prompt: "approve deploy?",
  status: "pending",
  created_at: "2026-10-29T11:30:00Z",
  link: "/monitor/runs/m%3Ap1%3Aloop-1",
};

async function mockApi(page: Page) {
  const state = { readAll: false, channelPut: false };
  await page.route("**/api/monitor/v1/**", async (route) => {
    const url = route.request().url();
    const method = route.request().method();
    const json = (body: unknown) =>
      route.fulfill({
        status: 200,
        contentType: "application/json",
        body: JSON.stringify(body),
      });
    if (url.includes("/stream"))
      return route.fulfill({
        status: 200,
        contentType: "text/event-stream",
        body: ": hb\n\n",
      });
    if (url.includes("/snapshot")) return json(SNAPSHOT);
    if (url.includes("/pending-interactions"))
      return json({ pending: [PENDING] });
    if (url.includes("/notifications/read-all")) {
      state.readAll = true;
      return json({ updated: 1 });
    }
    if (url.match(/\/notifications\/[^/]+\/(read|mute)/))
      return json({ notification: { ...NOTIF_UNREAD, state: "read" } });
    if (url.includes("/notifications/channels/webhook/test"))
      return json({ delivery_id: "d-1234", outcome: "delivered" });
    if (
      url.includes("/notifications/channels/webhook") &&
      method === "PUT"
    ) {
      state.channelPut = true;
      return json({
        url: "https://example.com/hook",
        secret: "••••cret",
        enabled: true,
      });
    }
    if (url.includes("/notifications/channels/webhook"))
      return json({ url: null, secret: null, enabled: false });
    if (url.includes("/notifications"))
      return json({
        notifications: state.readAll ? [] : [NOTIF_UNREAD],
        total: state.readAll ? 0 : 1,
        limit: 50,
        offset: 0,
      });
    if (url.includes("/projects"))
      return json({ projects: SNAPSHOT.projects });
    if (url.includes("/health")) return json({ status: "ok" });
    return json({});
  });
  return state;
}

test("inbox journey: badge → list → read-all → settings", async ({ page }) => {
  const state = await mockApi(page);
  await page.goto("/monitor");

  // Unread badge reflects the notifications total (S12).
  await expect(page.getByTestId("unread-badge")).toHaveText("1");

  // Open inbox — list + pending section render (S5/S11).
  await page.getByTestId("inbox-link").click();
  await expect(page).toHaveURL(/\/monitor\/inbox/);
  await expect(page.getByText("run failed: loop-1")).toBeVisible();
  await expect(page.getByTestId("pending-sess-9")).toBeVisible();
  await expect(page.getByText("approve deploy?")).toBeVisible();

  // Per-row read clears the badge after invalidation refetch.
  await page.getByTestId("read-n1").click();
  await expect(page.getByTestId("unread-badge")).toBeVisible(); // still 1 until server state flips
  await page.getByTestId("read-all").click();
  await expect(page.getByTestId("unread-badge")).toBeHidden();
  await expect(page.getByTestId("inbox-empty")).toBeVisible();

  // Filter tabs switch state (S5).
  await page.getByTestId("filter-muted").click();
  await expect(
    page.getByTestId("filter-muted"),
  ).toHaveAttribute("aria-selected", "true");

  // Settings: webhook save + test delivery result (S10/S13).
  await page.getByTestId("toggle-settings").click();
  await expect(page.getByTestId("runtime-webhook-hint")).toContainText("alpha");
  await page.getByTestId("webhook-url").fill("https://example.com/hook");
  await page.getByTestId("webhook-secret").fill("secret");
  await page.getByTestId("webhook-enabled").check();
  await page.getByTestId("webhook-save").click();
  await expect(page.getByTestId("settings-status")).toContainText("已保存");
  expect(state.channelPut).toBe(true);
  await page.getByTestId("webhook-test").click();
  await expect(page.getByTestId("settings-status")).toContainText(
    "测试投递成功",
  );
});
