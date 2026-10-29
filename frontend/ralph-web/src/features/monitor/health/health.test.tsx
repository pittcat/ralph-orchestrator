import { describe, expect, it } from "vitest";
import { render, screen } from "@testing-library/react";
import { HealthPanel } from "./HealthPanel";
import { LayerBadge } from "./LayerBadge";
import type { RunHealth } from "../api";

function health(over: Partial<RunHealth["layers"]> = {}): RunHealth {
  return {
    run_id: "r1",
    global_run_id: "p::r::r1",
    layers: {
      collector: {
        status: "ok",
        consecutive_failures: 0,
        last_collected_at: "2026-10-28T00:00:00Z",
      },
      runtime: {
        status: "running",
        pid: 42,
        pid_alive: true,
        pid_start_verified: true,
        last_event_at: null,
        last_activity_at: null,
      },
      backend: { status: "ok", last_end_at: null },
      ...over,
    },
    terminal_converged: false,
    last_valid_at: "2026-10-28T00:00:01Z",
  };
}

describe("LayerBadge", () => {
  it("renders layer label + status", () => {
    render(<LayerBadge layer="runtime" status="running" tone="ok" />);
    expect(screen.getByTestId("layer-runtime")).toHaveTextContent(
      "runtimerunning",
    );
  });
});

describe("HealthPanel", () => {
  it("renders all three layers", () => {
    render(<HealthPanel health={health()} />);
    expect(screen.getByTestId("layer-collector")).toBeInTheDocument();
    expect(screen.getByTestId("layer-runtime")).toBeInTheDocument();
    expect(screen.getByTestId("layer-backend")).toBeInTheDocument();
  });

  it("unknown backend renders muted, not alarming", () => {
    render(
      <HealthPanel
        health={health({ backend: { status: "unknown", last_end_at: null } })}
      />,
    );
    expect(screen.getByTestId("layer-backend")).toHaveTextContent("unknown");
  });

  it("converged run suppresses silent scare and shows converged chip", () => {
    render(
      <HealthPanel
        health={{
          ...health({
            runtime: {
              status: "silent",
              pid: null,
              pid_alive: false,
              pid_start_verified: null,
              last_event_at: null,
              last_activity_at: null,
            },
          }),
          terminal_converged: true,
        }}
      />,
    );
    expect(screen.getByTestId("layer-runtime")).toHaveTextContent("exited");
    expect(screen.getByTestId("converged-chip")).toBeInTheDocument();
  });

  it("pending backend shows pending status", () => {
    render(
      <HealthPanel
        health={health({
          backend: { status: "unknown", pending: true, last_end_at: null },
        })}
      />,
    );
    expect(screen.getByTestId("layer-backend")).toHaveTextContent("pending");
  });
});
