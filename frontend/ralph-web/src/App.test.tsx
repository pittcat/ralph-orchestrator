/**
 * App Routing Tests
 *
 * The dashboard is monitor-only: monitor routes render their pages,
 * every other path redirects to the monitor overview.
 */

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor } from "@testing-library/react";
import { MemoryRouter } from "react-router-dom";
import { App } from "./App";

// Mock monitor pages to isolate routing tests.
vi.mock("./features/monitor/OverviewPage", () => ({
  OverviewPage: () => <div data-testid="overview-page">Overview</div>,
}));
vi.mock("./features/monitor/RunDetailPage", () => ({
  RunDetailPage: () => <div data-testid="run-detail-page">Run Detail</div>,
}));
vi.mock("./features/monitor/inbox/InboxPage", () => ({
  InboxPage: () => <div data-testid="inbox-page">Inbox</div>,
}));
vi.mock("./features/monitor/ops/OperationsPage", () => ({
  OperationsPage: () => <div data-testid="operations-page">Operations</div>,
}));
vi.mock("./features/monitor/MonitorLayout", () => ({
  MonitorLayout: ({ children }: { children: React.ReactNode }) => (
    <div data-testid="monitor-layout">{children}</div>
  ),
}));
vi.mock("./features/monitor/connection/ConnectionProvider", () => ({
  ConnectionProvider: ({ children }: { children: React.ReactNode }) => (
    <>{children}</>
  ),
}));
vi.mock("./features/monitor/theme", () => ({
  useThemeEffect: () => {},
}));

function renderWithRoute(route: string) {
  return render(
    <MemoryRouter initialEntries={[route]}>
      <App />
    </MemoryRouter>,
  );
}

describe("App routing (monitor-only)", () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it("redirects / to the monitor overview", async () => {
    renderWithRoute("/");
    await waitFor(() => {
      expect(screen.getByTestId("overview-page")).toBeInTheDocument();
    });
  });

  it("renders the overview for /monitor", async () => {
    renderWithRoute("/monitor");
    await waitFor(() => {
      expect(screen.getByTestId("overview-page")).toBeInTheDocument();
    });
  });

  it("renders the inbox for /monitor/inbox", async () => {
    renderWithRoute("/monitor/inbox");
    await waitFor(() => {
      expect(screen.getByTestId("inbox-page")).toBeInTheDocument();
    });
  });

  it("renders operations for /monitor/operations", async () => {
    renderWithRoute("/monitor/operations");
    await waitFor(() => {
      expect(screen.getByTestId("operations-page")).toBeInTheDocument();
    });
  });

  it("renders run detail for /monitor/runs/:globalId", async () => {
    renderWithRoute("/monitor/runs/m1:proj_1:run_1");
    await waitFor(() => {
      expect(screen.getByTestId("run-detail-page")).toBeInTheDocument();
    });
  });

  it("redirects unknown routes to the monitor overview", async () => {
    renderWithRoute("/tasks");
    await waitFor(() => {
      expect(screen.getByTestId("overview-page")).toBeInTheDocument();
    });
    expect(screen.queryByTestId("run-detail-page")).not.toBeInTheDocument();
  });
});
