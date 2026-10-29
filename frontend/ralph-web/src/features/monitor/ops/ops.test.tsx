/**
 * Plan5-U3 tests: LaunchDialog stepper + idempotency-key lifecycle,
 * ActionsMenu disabled-reason rendering, ConfirmDialog revision +
 * conflict presentation, OperationsSection state matrix + recheck.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { createElement, type ReactNode } from "react";
import { LaunchDialog } from "../launch/LaunchDialog";
import { ActionsMenu } from "../actions/ActionsMenu";
import { ConfirmDialog } from "../actions/ConfirmDialog";
import { OperationsSection } from "./OperationsSection";
import type { MonitorRun } from "../api";

const fetchMock = vi.fn();

function wrapper() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return ({ children }: { children: ReactNode }) =>
    createElement(
      QueryClientProvider,
      { client },
      createElement(MemoryRouter, null, children),
    );
}

const PROJECTS = {
  projects: [
    {
      id: "p1",
      canonicalPath: "/repo/one",
      displayName: "one",
      createdAt: "2026-10-28T00:00:00Z",
      runtimeWebhookHint: "none",
    },
  ],
};

const PRESETS = {
  presets: [
    { id: "builtin:debug", name: "debug", source: "builtin" },
    { id: "directory:custom-hat", name: "custom-hat", source: "custom" },
  ],
  fetched_at: "2026-10-29T00:00:00Z",
  builtin_error: null,
};

const RUN: MonitorRun = {
  globalRunId: "m:p1:loop-1",
  projectId: "p1",
  runId: "loop-1",
  kind: "primary",
  state: "running",
  stateEvidence: {},
  cwd: null,
  repoRoot: null,
  worktreePath: null,
  promptPreview: null,
  promptSha256: null,
  preset: "debug",
  planPath: null,
  startedAt: "2026-10-29T00:00:00Z",
  terminal: null,
  lastEventAt: null,
  lastObservedAt: "2026-10-29T00:00:00Z",
  manifestPath: null,
  stale: false,
  actions: [
    { id: "stop", available: true, revision: "abc123def4567890" },
    { id: "retry", available: false, reason: "not in merge queue", revision: "r2" },
    {
      id: "emit",
      available: true,
      revision: "r4",
      input_schema: { topics: ["loop.cancel", "task.resume"] },
    },
  ],
};

function op(over: Partial<Record<string, unknown>> = {}) {
  return {
    id: "op-1",
    idempotencyKey: "k",
    kind: "stop",
    projectId: "p1",
    runGid: "m:p1:loop-1",
    clientRequestId: null,
    params: {},
    expectedRevision: null,
    state: "confirmed",
    receipt: { ok: true, action: "stop" },
    actor: null,
    origin: null,
    error: null,
    createdAt: "2026-10-29T01:00:00Z",
    dispatchedAt: "2026-10-29T01:00:01Z",
    confirmedAt: "2026-10-29T01:00:02Z",
    ...over,
  };
}

function jsonResponse(body: unknown, status = 200) {
  return Promise.resolve(
    new Response(JSON.stringify(body), {
      status,
      headers: { "content-type": "application/json" },
    }),
  );
}

beforeEach(() => {
  vi.stubGlobal("fetch", fetchMock);
  fetchMock.mockReset();
  fetchMock.mockImplementation((url: string) => {
    if (url.includes("/projects/") && url.includes("/presets"))
      return jsonResponse(PRESETS);
    if (url.includes("/projects")) return jsonResponse(PROJECTS);
    if (url.includes("/operations")) return jsonResponse({ operations: [] });
    return jsonResponse({});
  });
});

afterEach(() => vi.unstubAllGlobals());

describe("LaunchDialog", () => {
  it("stepper: next blocked until each step is satisfied", async () => {
    const user = userEvent.setup();
    render(
      createElement(
        wrapper(),
        null,
        createElement(LaunchDialog, { open: true, onOpenChange: () => {} }),
      ),
    );
    // Step 0: project required.
    await user.click(screen.getByTestId("launch-next"));
    expect(screen.getByTestId("launch-error")).toBeInTheDocument();
    await user.click(await screen.findByText("one"));
    await user.click(screen.getByTestId("launch-next"));

    // Step 1: preset required.
    await screen.findByTestId("launch-presets");
    await user.click(screen.getByText("debug"));
    await user.click(screen.getByTestId("launch-next"));

    // Step 2: prompt or plan required.
    await screen.findByTestId("launch-params");
    await user.click(screen.getByTestId("launch-next"));
    expect(screen.getByTestId("launch-error")).toBeInTheDocument();
    await user.type(screen.getByTestId("launch-prompt"), "do it");
    await user.click(screen.getByTestId("launch-next"));

    // Step 3: review shows the summary.
    const review = await screen.findByTestId("launch-review");
    expect(review).toHaveTextContent("debug");
  });

  it("submit posts launch with one idempotency key; retry reuses it", async () => {
    const user = userEvent.setup();
    const posted: { url: string; body: string }[] = [];
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url.includes("/operations") && init?.method === "POST") {
        posted.push({ url, body: String(init.body) });
        // First attempt fails → retry must replay the same key.
        return posted.length === 1
          ? jsonResponse(
              { error: { code: "spawn", message: "boom" } },
              500,
            )
          : jsonResponse({
              operation: op({ id: "op-9", kind: "launch", state: "dispatched" }),
            });
      }
      if (url.includes("/projects/") && url.includes("/presets"))
        return jsonResponse(PRESETS);
      if (url.includes("/projects")) return jsonResponse(PROJECTS);
      if (url.includes("/operations")) return jsonResponse({ operations: [] });
      return jsonResponse({});
    });

    render(
      createElement(
        wrapper(),
        null,
        createElement(LaunchDialog, { open: true, onOpenChange: () => {} }),
      ),
    );
    await user.click(await screen.findByText("one"));
    await user.click(screen.getByTestId("launch-next"));
    await user.click(await screen.findByText("debug"));
    await user.click(screen.getByTestId("launch-next"));
    await user.type(await screen.findByTestId("launch-prompt"), "go");
    await user.click(screen.getByTestId("launch-next"));
    await user.click(await screen.findByTestId("launch-submit"));

    // First submit fails → error shown, same key reused on retry.
    await screen.findByTestId("launch-error");
    await user.click(screen.getByTestId("launch-submit"));
    await waitFor(() => expect(posted.length).toBe(2));
    const k1 = JSON.parse(posted[0].body).idempotency_key;
    const k2 = JSON.parse(posted[1].body).idempotency_key;
    expect(k1).toBe(k2);
    expect(JSON.parse(posted[0].body)).toMatchObject({
      kind: "launch",
      project_id: "p1",
      params: { preset: "debug", prompt: "go" },
    });
  });
});

describe("ActionsMenu + ConfirmDialog", () => {
  it("renders capability buttons; disabled carries reason", async () => {
    render(
      createElement(wrapper(), null, createElement(ActionsMenu, { run: RUN })),
    );
    const stop = screen.getByTestId("action-stop");
    expect(stop).toBeEnabled();
    const retry = screen.getByTestId("action-retry");
    expect(retry).toBeDisabled();
    expect(retry).toHaveAttribute("title", "not in merge queue");
  });

  it("confirm dialog shows truncated revision and submits expected_revision", async () => {
    const user = userEvent.setup();
    const posted: string[] = [];
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url.includes("/operations") && init?.method === "POST") {
        posted.push(String(init.body));
        return jsonResponse({ operation: op() });
      }
      if (url.includes("/operations")) return jsonResponse({ operations: [] });
      return jsonResponse({});
    });
    render(
      createElement(
        wrapper(),
        null,
        createElement(ConfirmDialog, {
          run: RUN,
          action: RUN.actions[0],
          onClose: () => {},
        }),
      ),
    );
    expect(screen.getByTestId("confirm-revision")).toHaveTextContent(
      "abc123def456",
    );
    await user.click(screen.getByTestId("confirm-force"));
    await user.click(screen.getByTestId("confirm-submit"));
    await screen.findByTestId("confirm-result");
    const body = JSON.parse(posted[0]);
    expect(body).toMatchObject({
      kind: "stop",
      run_gid: "m:p1:loop-1",
      expected_revision: "abc123def4567890",
      params: { force: true },
    });
    expect(screen.getByTestId("confirm-result")).toHaveTextContent("confirmed");
  });

  it("409 revision_conflict → conflict hint, dialog stays open", async () => {
    const user = userEvent.setup();
    fetchMock.mockImplementation((url: string, init?: RequestInit) => {
      if (url.includes("/operations") && init?.method === "POST") {
        return jsonResponse(
          { error: { code: "revision_conflict", message: "stale" } },
          409,
        );
      }
      return jsonResponse({ operations: [] });
    });
    render(
      createElement(
        wrapper(),
        null,
        createElement(ConfirmDialog, {
          run: RUN,
          action: RUN.actions[0],
          onClose: () => {},
        }),
      ),
    );
    await user.click(screen.getByTestId("confirm-submit"));
    const err = await screen.findByTestId("confirm-error");
    expect(err).toHaveTextContent(/revision 冲突/);
  });

  it("emit dialog offers only whitelisted topics", async () => {
    render(
      createElement(
        wrapper(),
        null,
        createElement(ConfirmDialog, {
          run: RUN,
          action: RUN.actions[2],
          onClose: () => {},
        }),
      ),
    );
    const select = screen.getByTestId("confirm-topic") as HTMLSelectElement;
    const options = [...select.options].map((o) => o.value);
    expect(options).toEqual(["loop.cancel", "task.resume"]);
  });
});

describe("OperationsSection", () => {
  it("renders state matrix + recheck on confirm_timeout", async () => {
    const user = userEvent.setup();
    const rechecked: string[] = [];
    fetchMock.mockImplementation((url: string) => {
      if (url.includes("/operations/op-2/recheck")) {
        rechecked.push("op-2");
        return jsonResponse({ operation: op({ id: "op-2" }) });
      }
      if (url.includes("/operations")) {
        return jsonResponse({
          operations: [
            op({ id: "op-1", state: "confirmed" }),
            op({ id: "op-2", state: "confirm_timeout", receipt: null }),
            op({ id: "op-3", state: "failed", receipt: null, error: "exit 5" }),
          ],
        });
      }
      return jsonResponse({});
    });
    render(
      createElement(
        wrapper(),
        null,
        createElement(OperationsSection, { runGid: "m:p1:loop-1" }),
      ),
    );
    await screen.findByTestId("operations-table");
    expect(screen.getByTestId("op-state-op-1")).toHaveTextContent("已确认");
    expect(screen.getByTestId("op-state-op-2")).toHaveTextContent("确认超时");
    expect(screen.getByTestId("op-state-op-3")).toHaveTextContent("失败");
    await user.click(screen.getByTestId("op-recheck-op-2"));
    await waitFor(() => expect(rechecked).toEqual(["op-2"]));
  });
});
