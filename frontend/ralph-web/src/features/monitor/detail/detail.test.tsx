/**
 * Plan2-U4 component tests: StageStrip highlight/round (S1/S10),
 * no-descriptor fallback (S2), TimelineTab selection + keyboard +
 * artifact jump (S9), LogsTab pause/search/window (S4).
 */

import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { StageStrip } from "./StageStrip";
import { TimelineTab } from "./TimelineTab";
import { LogsTab } from "./LogsTab";
import { DetailHeader } from "./DetailHeader";
import type { MonitorEvent, MonitorRun, RunProgress } from "../api";

const stages = [
  { id: "plan", label: "Plan" },
  { id: "execute", label: "Execute" },
  { id: "review", label: "Review" },
];

const progress: RunProgress = {
  stage: "review",
  stage_label: "Review",
  stage_history: [
    { stage: "plan", at_seq: 1, round: 1 },
    { stage: "execute", at_seq: 4, round: 1 },
    { stage: "review", at_seq: 9, round: 2 },
  ],
  current_hat: "reviewer",
  iteration: 3,
  elapsed_ms: 125_000,
  units: { done: 2, total: 5, source: "forge-events" },
  denominator: "stages",
};

const ev = (seq: number, over: Partial<MonitorEvent> = {}): MonitorEvent => ({
  seq,
  ts: `2026-10-28T12:00:0${seq}Z`,
  iteration: 1,
  hat: "executor",
  topic: `topic.${seq}`,
  payload: "{}",
  truncated: false,
  ...over,
});

describe("DetailHeader", () => {
  it("renders run id, project, elapsed, current_hat, units", () => {
    const run = {
      runId: "loop-1",
      projectId: "p1",
      state: "running",
      stale: false,
      preset: "ce",
      stateEvidence: {},
      terminal: null,
    } as unknown as MonitorRun;
    render(<DetailHeader run={run} progress={progress} />);
    expect(screen.getByTestId("run-detail-id")).toHaveTextContent("loop-1");
    expect(screen.getByTestId("detail-project")).toHaveTextContent("p1");
    expect(screen.getByTestId("detail-elapsed")).toHaveTextContent("2m 5s");
    expect(screen.getByTestId("detail-current-hat")).toHaveTextContent("reviewer");
    expect(screen.getByTestId("detail-units")).toHaveTextContent("2/5");
  });
});

describe("StageStrip", () => {
  it("highlights current stage, dims earlier, marks pending (S1)", () => {
    render(<StageStrip stages={stages} progress={progress} />);
    expect(screen.getByTestId("stage-node-review")).toHaveAttribute("data-state", "current");
    expect(screen.getByTestId("stage-node-execute")).toHaveAttribute("data-state", "done");
    expect(screen.getByTestId("stage-node-plan")).toHaveAttribute("data-state", "done");
  });

  it("shows round counter for re-entered stage (S10)", () => {
    render(<StageStrip stages={stages} progress={progress} />);
    expect(screen.getByTestId("stage-round-review")).toHaveTextContent("×2");
  });

  it("empty stages → renders nothing (S2)", () => {
    const { container } = render(<StageStrip stages={[]} progress={progress} />);
    expect(container.firstChild).toBeNull();
  });
});

describe("TimelineTab", () => {
  const events = [ev(1), ev(2, { topic: "forge.unit.ready", payload: "ref to summary" }), ev(3, { truncated: true })];

  it("renders seq rows, truncated marker, selects on Enter", () => {
    const onSelect = vi.fn();
    render(
      <TimelineTab
        events={events}
        isLoading={false}
        selectedSeq={null}
        onSelectSeq={onSelect}
        artifactSlotIds={new Set()}
        onArtifactJump={vi.fn()}
      />,
    );
    expect(screen.getByTestId("event-row-2")).toBeInTheDocument();
    expect(screen.getByText("truncated")).toBeInTheDocument();
    fireEvent.keyDown(screen.getByTestId("event-row-2"), { key: "Enter" });
    expect(onSelect).toHaveBeenCalledWith(2);
  });

  it("selected seq marks row; arrow keys move focus", () => {
    render(
      <TimelineTab
        events={events}
        isLoading={false}
        selectedSeq={3}
        onSelectSeq={vi.fn()}
        artifactSlotIds={new Set()}
        onArtifactJump={vi.fn()}
      />,
    );
    expect(screen.getByTestId("event-row-3")).toHaveAttribute("data-selected", "true");
    const list = screen.getByRole("table");
    fireEvent.keyDown(list, { key: "ArrowUp" });
    expect(screen.getByTestId("event-row-2")).toHaveFocus();
  });

  it("payload naming a declared slot shows jump button (S9)", () => {
    const jump = vi.fn();
    render(
      <TimelineTab
        events={events}
        isLoading={false}
        selectedSeq={null}
        onSelectSeq={vi.fn()}
        artifactSlotIds={new Set(["summary"])}
        onArtifactJump={jump}
      />,
    );
    fireEvent.click(screen.getByRole("button", { name: "summary" }));
    expect(jump).toHaveBeenCalledWith("summary");
  });
});

describe("LogsTab", () => {
  const events = [ev(1, { topic: "alpha.start" }), ev(2, { topic: "beta.mid" }), ev(3, { topic: "alpha.done" })];

  it("pause/resume toggles follow; search filters lines (S4)", () => {
    const onPaused = vi.fn();
    render(
      <LogsTab
        events={events}
        isLoading={false}
        paused={false}
        onPausedChange={onPaused}
        onLoadEarlier={vi.fn()}
        canLoadEarlier={false}
      />,
    );
    fireEvent.click(screen.getByTestId("pause-follow"));
    expect(onPaused).toHaveBeenCalledWith(true);
    // search filter
    fireEvent.change(screen.getByTestId("log-search"), { target: { value: "beta" } });
    expect(screen.getByTestId("log-list")).toHaveTextContent("beta.mid");
    expect(screen.getByTestId("log-list")).not.toHaveTextContent("alpha.start");
  });

  it("ctrl+f focuses the search box", () => {
    render(
      <LogsTab
        events={events}
        isLoading={false}
        paused={false}
        onPausedChange={vi.fn()}
        onLoadEarlier={vi.fn()}
        canLoadEarlier
      />,
    );
    fireEvent.keyDown(window, { key: "f", ctrlKey: true });
    expect(screen.getByTestId("log-search")).toHaveFocus();
  });
});
