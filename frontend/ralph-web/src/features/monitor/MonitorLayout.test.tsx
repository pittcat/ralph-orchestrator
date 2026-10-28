/**
 * MonitorLayout.test.tsx — shell renders, empty state, theme toggle.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { MemoryRouter } from "react-router-dom";
import { createElement, type ReactNode } from "react";
import { MonitorLayout } from "./MonitorLayout";
import { MonitorOverview } from "./MonitorOverview";
import { useThemeStore } from "./theme";

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

beforeEach(() => {
  vi.stubGlobal("fetch", fetchMock);
  fetchMock.mockReset();
  localStorage.clear();
  fetchMock.mockImplementation((url: string) => {
    const body = url.includes("/projects")
      ? { projects: [] }
      : { runs: [], total: 0, watermark: null };
    return Promise.resolve(
      new Response(JSON.stringify(body), {
        headers: { "content-type": "application/json" },
      }),
    );
  });
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("MonitorLayout + Overview", () => {
  it("renders the shell with a Monitor heading and empty-state guidance", async () => {
    render(
      <MonitorLayout>
        <MonitorOverview />
      </MonitorLayout>,
      { wrapper: wrapper() },
    );
    expect(screen.getByRole("heading", { name: "Monitor" })).toBeInTheDocument();
    expect(await screen.findByTestId("overview-empty")).toBeInTheDocument();
    expect(screen.getByText(/Register a project directory/)).toBeInTheDocument();
  });

  it("theme toggle flips data-theme and persists", async () => {
    const user = userEvent.setup();
    useThemeStore.setState({ theme: "dark" });
    render(
      <MonitorLayout>
        <div />
      </MonitorLayout>,
      { wrapper: wrapper() },
    );
    await user.click(screen.getByLabelText("Switch to light theme"));
    expect(useThemeStore.getState().theme).toBe("light");
    const raw = JSON.parse(localStorage.getItem("ralph-monitor-theme")!);
    expect(raw.state.theme).toBe("light");
  });

  it("renders run count summary when runs exist", async () => {
    fetchMock.mockImplementation((url: string) => {
      const body = url.includes("/projects")
        ? { projects: [{ id: "p1", canonicalPath: "/w", displayName: "w", createdAt: "t" }] }
        : {
            runs: [
              {
                globalRunId: "m:p1:r1",
                projectId: "p1",
                runId: "r1",
                kind: "primary",
                state: "running",
                stateEvidence: {},
                cwd: null,
                repoRoot: null,
                worktreePath: null,
                promptPreview: null,
                promptSha256: null,
                preset: null,
                planPath: null,
                startedAt: null,
                terminal: null,
                lastEventAt: null,
                lastObservedAt: "t",
                manifestPath: null,
                stale: false,
              },
            ],
            total: 1,
            watermark: "t",
          };
      return Promise.resolve(
        new Response(JSON.stringify(body), {
          headers: { "content-type": "application/json" },
        }),
      );
    });
    render(
      <MonitorLayout>
        <MonitorOverview />
      </MonitorLayout>,
      { wrapper: wrapper() },
    );
    await waitFor(() =>
      expect(screen.getByTestId("overview-summary")).toBeInTheDocument(),
    );
    expect(screen.getByText(/1 run observed/)).toBeInTheDocument();
  });
});
