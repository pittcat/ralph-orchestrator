/**
 * RunDetailPage.test.tsx — fields, evidence timestamps (incl.
 * unknown state), back link preserving filter query.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter, Routes, Route } from "react-router-dom";
import { createElement, type ReactNode } from "react";
import { RunDetailPage } from "./RunDetailPage";

const fetchMock = vi.fn();

const DETAIL = {
  run: {
    globalRunId: "m:p1:loop-9",
    projectId: "p1",
    runId: "loop-9",
    kind: "worktree",
    state: "exited_unknown",
    stateEvidence: { manifest: "corrupt" },
    cwd: "/w/a",
    repoRoot: "/w/a",
    worktreePath: "/w/a/.wt",
    promptPreview: "fix the thing",
    promptSha256: null,
    preset: "ce",
    planPath: "/docs/plans/x.md",
    startedAt: "2026-10-28T10:00:00Z",
    terminal: null,
    lastEventAt: null,
    lastObservedAt: "2026-10-28T12:00:00Z",
    manifestPath: "/w/a/.ralph/runs/loop-9.manifest.json",
    stale: false,
    evidenceTimestamps: {
      lastEventAt: null,
      lastObservedAt: "2026-10-28T12:00:00Z",
      terminalAt: null,
      collectedAt: "2026-10-28T12:01:00Z",
    },
  },
};

function wrapper(url: string) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return ({ children }: { children: ReactNode }) =>
    createElement(
      QueryClientProvider,
      { client },
      createElement(
        MemoryRouter,
        { initialEntries: [url] },
        createElement(
          Routes,
          null,
          createElement(Route, {
            path: "/monitor/runs/:globalId",
            element: children,
          }),
        ),
      ),
    );
}

beforeEach(() => {
  vi.stubGlobal("fetch", fetchMock);
  fetchMock.mockReset();
  fetchMock.mockImplementation(() =>
    Promise.resolve(
      new Response(JSON.stringify(DETAIL), {
        headers: { "content-type": "application/json" },
      }),
    ),
  );
});

afterEach(() => vi.unstubAllGlobals());

describe("RunDetailPage", () => {
  it("renders run id, state badge, fields and evidence timestamps", async () => {
    render(<RunDetailPage />, {
      wrapper: wrapper("/monitor/runs/m%3Ap1%3Aloop-9"),
    });
    expect(await screen.findByTestId("run-detail-id")).toHaveTextContent("loop-9");
    expect(screen.getByTestId("run-state-badge")).toHaveAttribute(
      "data-state",
      "exited_unknown",
    );
    // unknown state still shows evidence timestamps (S5/S6).
    const ev = screen.getByTestId("evidence-timestamps");
    expect(ev).toHaveTextContent("last_event_at");
    expect(ev).toHaveTextContent("terminal.at");
    expect(ev).toHaveTextContent("collected_at");
    // field table.
    expect(screen.getByText("/w/a/.wt")).toBeInTheDocument();
    expect(screen.getByText("fix the thing")).toBeInTheDocument();
    expect(screen.getByTestId("state-evidence")).toHaveTextContent("corrupt");
  });

  it("back link preserves the overview filter query", async () => {
    render(<RunDetailPage />, {
      wrapper: wrapper("/monitor/runs/m%3Ap1%3Aloop-9?state=exited_unknown&q=x"),
    });
    await screen.findByTestId("run-detail-id");
    const back = screen.getByRole("link", { name: /back to overview/i });
    expect(back).toHaveAttribute(
      "href",
      "/monitor?state=exited_unknown&q=x",
    );
  });
});
