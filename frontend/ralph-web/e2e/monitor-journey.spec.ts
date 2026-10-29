import { expect, test, type APIRequestContext } from "@playwright/test";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import {
  killMonitorChild,
  readFixtures,
  type JourneyFixtures,
} from "./helpers/monitor-server";

const artifactsDir = path.join(
  path.dirname(fileURLToPath(import.meta.url)),
  "artifacts/monitor-journey",
);

let fixtures: JourneyFixtures;

/** Register a fixture project through the real API (CSRF double-submit). */
async function registerProject(
  request: APIRequestContext,
  dir: string,
): Promise<void> {
  const health = await request.get("/api/monitor/v1/health");
  expect(health.ok()).toBeTruthy();
  // The request context jar auto-attaches monitor_sid/monitor_csrf —
  // only the x-monitor-csrf header must echo the csrf cookie value.
  const state = await request.storageState();
  const csrf = state.cookies.find((c) => c.name === "monitor_csrf")?.value;
  expect(csrf, "monitor_csrf session cookie").toBeTruthy();
  const res = await request.post("/api/monitor/v1/projects", {
    headers: { "x-monitor-csrf": csrf ?? "" },
    data: { path: dir },
  });
  expect([201, 409]).toContain(res.status());
}

test.describe("monitor journey (Plan5-U5)", () => {
  test.beforeAll(() => {
    fixtures = readFixtures();
    fs.mkdirSync(artifactsDir, { recursive: true });
  });

  test("register → launch → health → stop → notify → kill → reconnect", async ({
    page,
    request,
  }) => {
    test.setTimeout(90_000);
    const [alpha, beta] = fixtures.projects;

    // ── register both projects via the real API ──
    for (const p of fixtures.projects) await registerProject(request, p.dir);

    // ── overview distinguishes the two projects' historical runs ──
    await page.goto("/monitor");
    await expect(page.getByTestId("connection-badge")).toHaveAttribute(
      "data-status",
      "connected",
      { timeout: 15_000 },
    );
    await expect(page.getByText(alpha.runId.slice(0, 20))).toBeVisible({
      timeout: 15_000,
    });
    await expect(page.getByText(beta.runId.slice(0, 20))).toBeVisible();
    await page.screenshot({ path: path.join(artifactsDir, "overview.png") });

    // ── launch a new run on project alpha through the 4-step dialog ──
    await page.getByTestId("launch-button").click();
    await expect(page.getByTestId("launch-dialog")).toBeVisible();
    await page
      .getByTestId("launch-projects")
      .getByRole("button")
      .first()
      .click();
    await page.getByTestId("launch-next").click();
    await page
      .getByTestId("launch-presets")
      .getByRole("button")
      .first()
      .click();
    await page.getByTestId("launch-next").click();
    await page.getByTestId("launch-prompt").fill("e2e journey launch");
    await page.getByTestId("launch-next").click();
    await page.getByTestId("launch-submit").click();

    // Confirmed launch auto-navigates to the new run's detail page.
    await expect(page).toHaveURL(/\/monitor\/runs\//, { timeout: 20_000 });
    await expect(page.getByTestId("run-detail")).toBeVisible();

    // ── four-layer health strip ──
    for (const layer of ["collector", "runtime", "backend"]) {
      await expect(page.getByText(layer, { exact: false }).first()).toBeVisible();
    }
    await page.screenshot({ path: path.join(artifactsDir, "run-detail.png") });

    // ── stop action → confirm → confirmed receipt ──
    await page.getByTestId("action-stop").click();
    await expect(page.getByTestId("confirm-dialog")).toBeVisible();
    await page.getByTestId("confirm-submit").click();
    await expect(page.getByTestId("confirm-result")).toBeVisible();
    await page
      .getByTestId("confirm-result")
      .getByRole("button", { name: "关闭" })
      .click();

    // ── operations tab shows the op reaching confirmed ──
    await page.getByTestId("tab-operations").click();
    await expect(
      page.getByTestId("operations-table").getByText("已确认").first(),
    ).toBeVisible({ timeout: 15_000 });

    // ── notification produced → inbox → deep link back to the run ──
    await expect(page.getByTestId("unread-badge")).toBeVisible({
      timeout: 15_000,
    });
    await page.getByTestId("inbox-link").click();
    await expect(page.getByTestId("notif-list")).toBeVisible();
    await page.locator("[data-testid^='notif-']").first().click();
    await expect(page).toHaveURL(/\/monitor\/runs\/.+source=notification/);

    // ── kill the monitor child → reconnect after supervisor respawn ──
    killMonitorChild();
    // Deterministic down-window proof: health refuses connections while
    // the child is dead and the supervisor is still respawning.
    await expect
      .poll(
        async () =>
          request
            .get("/api/monitor/v1/health")
            .then((r) => r.status())
            .catch(() => 0),
        { timeout: 15_000, intervals: [100, 200, 400, 800] },
      )
      .not.toBe(200);
    await expect(page.getByTestId("connection-badge")).not.toHaveAttribute(
      "data-status",
      "connected",
      { timeout: 15_000 },
    );
    await expect(page.getByTestId("connection-badge")).toHaveAttribute(
      "data-status",
      "connected",
      { timeout: 30_000 },
    );
    // State stays consistent after resync.
    await expect(page.getByTestId("run-detail")).toBeVisible();
    await page.screenshot({ path: path.join(artifactsDir, "reconnected.png") });
  });

  test("sweep: viewports / themes / keyboard / reduced-motion", async ({
    page,
    request,
  }) => {
    test.setTimeout(60_000);
    for (const p of fixtures.projects) await registerProject(request, p.dir);
    await page.goto("/monitor");
    await expect(page.getByTestId("runs-table")).toBeVisible({
      timeout: 15_000,
    });

    // Mobile viewport.
    await page.setViewportSize({ width: 375, height: 812 });
    await expect(page.getByTestId("runs-table")).toBeVisible();
    await page.screenshot({
      path: path.join(artifactsDir, "viewport-375.png"),
    });
    await page.setViewportSize({ width: 1280, height: 800 });

    // Dark / light theme screenshots.
    const themeBtn = page.getByRole("button", {
      name: /dark|light|主题|theme/i,
    });
    await page.screenshot({
      path: path.join(artifactsDir, "theme-current.png"),
    });
    if (await themeBtn.count()) {
      await themeBtn.first().click();
      await page.screenshot({
        path: path.join(artifactsDir, "theme-toggled.png"),
      });
      await themeBtn.first().click();
    }

    // Keyboard-only pass: Tab lands on interactive elements in order.
    await page.keyboard.press("Tab");
    const focusedTag = await page.evaluate(
      () => document.activeElement?.tagName,
    );
    expect(["A", "BUTTON", "INPUT", "TR"]).toContain(focusedTag);
    await page.screenshot({ path: path.join(artifactsDir, "keyboard.png") });

    // Reduced motion.
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.screenshot({
      path: path.join(artifactsDir, "reduced-motion.png"),
    });
  });
});
