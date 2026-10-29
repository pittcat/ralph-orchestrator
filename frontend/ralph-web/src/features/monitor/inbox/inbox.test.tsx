/**
 * Plan4-U3 inbox tests: filter state machine, read/mute/read-all
 * mutations, pending section, unread badge, and the browser
 * notification permission branches.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { createElement, type ReactNode } from "react";
import { InboxPage } from "./InboxPage";
import { InboxLink } from "../MonitorLayout";

const fetchMock = vi.fn();

const NOTIFS = [
  {
    id: "n1",
    kind: "run.completed",
    project_id: "p1",
    run_gid: "r1",
    pending_interaction_id: null,
    title: "run finished",
    body: null,
    detail: null,
    state: "unread",
    created_at: "2026-10-29T01:00:00Z",
    resolved_at: null,
    link: "/monitor/runs/r1",
  },
  {
    id: "n2",
    kind: "run.failed",
    project_id: "p1",
    run_gid: "r2",
    pending_interaction_id: null,
    title: "run failed",
    body: null,
    detail: null,
    state: "muted",
    created_at: "2026-10-29T02:00:00Z",
    resolved_at: null,
    link: null,
  },
];

const PENDING = [
  {
    id: "pi1",
    project_id: "p1",
    session_id: "sess-1",
    run_gid: "r1",
    prompt: "approve?",
    status: "pending",
    created_at: "2026-10-29T02:30:00Z",
    link: "/monitor/runs/r1",
  },
];

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

beforeEach(() => {
  vi.stubGlobal("fetch", fetchMock);
  fetchMock.mockReset();
  localStorage.clear();
  fetchMock.mockImplementation((url: string, init?: RequestInit) => {
    let body: unknown = {};
    if (url.includes("/notifications/read-all")) body = { updated: 3 };
    else if (url.includes("/notifications/") && init?.method === "POST")
      body = { notification: NOTIFS[0] };
    else if (url.includes("/pending-interactions"))
      body = { pending: PENDING };
    else if (url.includes("/notifications/channels/webhook"))
      body = { url: null, secret: null, enabled: false };
    else if (url.includes("/notifications"))
      body = {
        notifications: NOTIFS,
        total: 2,
        limit: 50,
        offset: 0,
      };
    else if (url.includes("/projects")) body = { projects: [] };
    return Promise.resolve(
      new Response(JSON.stringify(body), {
        headers: { "content-type": "application/json" },
      }),
    );
  });
});

afterEach(() => vi.unstubAllGlobals());

describe("InboxPage", () => {
  it("renders notifications + pending section", async () => {
    render(createElement(wrapper(), null, createElement(InboxPage)));
    expect(await screen.findByText("run finished")).toBeInTheDocument();
    expect(screen.getByTestId("pending-sess-1")).toBeInTheDocument();
    expect(screen.getByText("完成")).toBeInTheDocument();
    expect(screen.getByText("失败")).toBeInTheDocument();
  });

  it("marks a row read and sends read-all", async () => {
    const user = userEvent.setup();
    render(createElement(wrapper(), null, createElement(InboxPage)));
    await screen.findByText("run finished");
    await user.click(screen.getByTestId("read-n1"));
    await waitFor(() =>
      expect(
        fetchMock.mock.calls.some(
          ([u, i]) =>
            String(u).includes("/notifications/n1/read") &&
            (i as RequestInit)?.method === "POST",
        ),
      ).toBe(true),
    );
    await user.click(screen.getByTestId("read-all"));
    await waitFor(() =>
      expect(
        fetchMock.mock.calls.some(([u, i]) =>
          String(u).includes("/notifications/read-all") &&
          (i as RequestInit)?.method === "POST",
        ),
      ).toBe(true),
    );
  });

  it("mutes a row", async () => {
    const user = userEvent.setup();
    render(createElement(wrapper(), null, createElement(InboxPage)));
    await screen.findByText("run finished");
    await user.click(screen.getByTestId("mute-n1"));
    await waitFor(() =>
      expect(
        fetchMock.mock.calls.some(([u, i]) =>
          String(u).includes("/notifications/n1/mute") &&
          (i as RequestInit)?.method === "POST",
        ),
      ).toBe(true),
    );
  });

  it("filter tabs change the state query param", async () => {
    const user = userEvent.setup();
    render(createElement(wrapper(), null, createElement(InboxPage)));
    await screen.findByText("run finished");
    fetchMock.mockClear();
    await user.click(screen.getByTestId("filter-muted"));
    await waitFor(() =>
      expect(
        fetchMock.mock.calls.some(([u]) =>
          String(u).includes("state=muted"),
        ),
      ).toBe(true),
    );
  });
});

describe("InboxLink badge", () => {
  it("shows unread count from the notifications total", async () => {
    render(createElement(wrapper(), null, createElement(InboxLink)));
    expect(await screen.findByTestId("unread-badge")).toHaveTextContent("2");
  });
});
