/**
 * DeleteRunButton.test.tsx — confirm dialog, DELETE call,
 * success callback, cancel and error paths.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createElement, type ReactNode } from "react";
import { DeleteRunButton } from "./DeleteRunButton";
import { Toaster } from "@/components/ui/toast";
import type { MonitorRun } from "../api";

const fetchMock = vi.fn();

const RUN = {
  globalRunId: "m:p1:loop-1",
  projectId: "p1",
  runId: "loop-1",
  kind: "worktree",
  state: "exited_failed",
  stateEvidence: {},
  cwd: "/w/a",
  repoRoot: "/w/a",
  worktreePath: null,
  promptPreview: null,
  promptSha256: null,
  preset: null,
  planPath: null,
  startedAt: "2026-10-28T10:00:00Z",
  terminal: null,
  lastEventAt: null,
  lastObservedAt: "2026-10-28T12:00:00Z",
  manifestPath: null,
  stale: false,
} as unknown as MonitorRun;

function wrapper() {
  const client = new QueryClient({
    defaultOptions: { queries: { retry: false } },
  });
  return ({ children }: { children: ReactNode }) =>
    createElement(
      QueryClientProvider,
      { client },
      createElement("div", null, children, createElement(Toaster)),
    );
}

beforeEach(() => {
  vi.stubGlobal("fetch", fetchMock);
  fetchMock.mockReset();
  fetchMock.mockImplementation(
    () => new Promise<Response>(() => {}), // pending by default
  );
});

afterEach(() => vi.unstubAllGlobals());

describe("DeleteRunButton", () => {
  it("opens the confirm dialog and issues DELETE on confirm", async () => {
    const user = userEvent.setup();
    const onDeleted = vi.fn();
    fetchMock.mockImplementation((_url: string, init?: RequestInit) => {
      if (init?.method === "DELETE") {
        return Promise.resolve(new Response(null, { status: 204 }));
      }
      return Promise.resolve(
        new Response(JSON.stringify({}), {
          headers: { "content-type": "application/json" },
        }),
      );
    });
    render(
      <DeleteRunButton run={RUN} variant="text" onDeleted={onDeleted} />,
      { wrapper: wrapper() },
    );

    await user.click(screen.getByTestId("delete-run"));
    expect(await screen.findByText("删除这个 run?")).toBeInTheDocument();
    expect(screen.getByText("loop-1")).toBeInTheDocument();

    await user.click(screen.getByTestId("confirm-delete-run"));
    await waitFor(() => {
      const del = fetchMock.mock.calls.find(([, i]) => i?.method === "DELETE");
      expect(del?.[0]).toContain("/runs/m%3Ap1%3Aloop-1");
    });
    await waitFor(() => expect(onDeleted).toHaveBeenCalled());
    expect(
      await screen.findByText((t) => t.includes("已删除")),
    ).toBeInTheDocument();
  });

  it("icon variant stops propagation and opens the same dialog", async () => {
    const user = userEvent.setup();
    const rowClick = vi.fn();
    render(
      <div onClick={rowClick}>
        <DeleteRunButton run={RUN} variant="icon" />
      </div>,
      { wrapper: wrapper() },
    );
    await user.click(screen.getByTestId("delete-run-loop-1"));
    expect(await screen.findByText("删除这个 run?")).toBeInTheDocument();
    expect(rowClick).not.toHaveBeenCalled();
  });

  it("cancel closes the dialog without calling DELETE", async () => {
    const user = userEvent.setup();
    render(<DeleteRunButton run={RUN} variant="text" />, {
      wrapper: wrapper() },
    );
    await user.click(screen.getByTestId("delete-run"));
    await user.click(await screen.findByText("取消"));
    await waitFor(() =>
      expect(screen.queryByText("删除这个 run?")).not.toBeInTheDocument(),
    );
    expect(
      fetchMock.mock.calls.some(([, i]) => i?.method === "DELETE"),
    ).toBe(false);
  });

  it("server error → error toast, no onDeleted", async () => {
    const user = userEvent.setup();
    const onDeleted = vi.fn();
    fetchMock.mockImplementation((_url: string, init?: RequestInit) => {
      if (init?.method === "DELETE") {
        return Promise.resolve(
          new Response(
            JSON.stringify({ error: { code: "run_not_found", message: "gone" } }),
            { status: 404, headers: { "content-type": "application/json" } },
          ),
        );
      }
      return Promise.resolve(
        new Response(JSON.stringify({}), {
          headers: { "content-type": "application/json" },
        }),
      );
    });
    render(
      <DeleteRunButton run={RUN} variant="text" onDeleted={onDeleted} />,
      { wrapper: wrapper() },
    );
    await user.click(screen.getByTestId("delete-run"));
    await user.click(await screen.findByTestId("confirm-delete-run"));
    expect(
      await screen.findByText((t) => t.includes("删除失败")),
    ).toBeInTheDocument();
    expect(onDeleted).not.toHaveBeenCalled();
  });
});
