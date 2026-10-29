/**
 * Plan2-U5 component tests: MarkdownViewer XSS (S6), GraphListView
 * equivalence + unit click (S5), ArtifactsTab slot status (S7),
 * GraphTab fallback/list toggle.
 */

import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent, waitFor } from "@testing-library/react";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { createElement, useState, type ReactNode } from "react";
import { MarkdownViewer } from "../components/MarkdownViewer";
import { GraphListView } from "../components/GraphListView";
import { GraphTab } from "./GraphTab";
import { ArtifactsTab } from "./ArtifactsTab";
import type { RunGraph } from "../api";

const GRAPH: RunGraph = {
  nodes: [
    { unit: "U1", status: "executed", last_seq: 3 },
    { unit: "U2", status: "ready", last_seq: 5 },
  ],
  edges: [{ from: "U1", to: "U2" }],
  source: "forge-events",
  scheduler: null,
  illegal_transitions: 0,
};

const fetchMock = vi.fn();
vi.stubGlobal("fetch", fetchMock);

function qc() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return ({ children }: { children: ReactNode }) =>
    createElement(QueryClientProvider, { client }, children);
}

function json(body: unknown, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

describe("MarkdownViewer", () => {
  it("renders headings/links and neutralizes dangerous URLs (S6)", () => {
    render(
      <MarkdownViewer content={"# Title\n[ok](https://a)\n[bad](javascript:alert(1))\n[x](data:text/html;base64,PGI+)"} />,
    );
    expect(screen.getByTestId("markdown-viewer")).toHaveTextContent("Title");
    const links = screen.getAllByRole("link");
    expect(links[0]).toHaveAttribute("href", "https://a");
    // dangerous schemes never make it into an href attribute
    for (const a of links) {
      const href = a.getAttribute("href") ?? "";
      expect(href).not.toMatch(/^javascript:/i);
      expect(href).not.toMatch(/^data:/i);
    }
  });

  it("does not render raw HTML (react-markdown default)", () => {
    render(<MarkdownViewer content={'<img src=x onerror=alert(1)>\n<script>alert(2)</script>'} />);
    expect(document.querySelector("img")).toBeNull();
    expect(document.querySelector("script")).toBeNull();
  });
});

describe("GraphListView", () => {
  it("renders the same nodes/edges as the canvas (S5)", () => {
    render(<GraphListView graph={GRAPH} onUnitClick={vi.fn()} />);
    expect(screen.getByTestId("graph-node-U1")).toHaveTextContent("executed");
    expect(screen.getByTestId("graph-node-U2")).toHaveTextContent("ready");
    expect(screen.getByTestId("graph-node-U1")).toHaveTextContent("U2");
  });

  it("empty graph → placeholder", () => {
    render(<GraphListView graph={{ ...GRAPH, nodes: [], edges: [] }} />);
    expect(screen.getByTestId("graph-empty")).toBeInTheDocument();
  });

  it("unit click forwards to filter callback", () => {
    const onUnit = vi.fn();
    render(<GraphListView graph={GRAPH} onUnitClick={onUnit} />);
    fireEvent.click(screen.getByRole("button", { name: "U1" }));
    expect(onUnit).toHaveBeenCalledWith("U1");
  });
});

describe("GraphTab", () => {
  it("lazy canvas + list toggle (S5)", async () => {
    fetchMock.mockResolvedValueOnce(json(GRAPH));
    render(<GraphTab globalRunId="m:p:r" onUnitSelect={vi.fn()} />, { wrapper: qc() });
    await waitFor(() => expect(screen.getByTestId("graph-view-list")).toBeInTheDocument());
    fireEvent.click(screen.getByTestId("graph-view-list"));
    expect(screen.getByTestId("graph-node-U1")).toBeInTheDocument();
  });

  it("empty graph → list fallback without view buttons", async () => {
    fetchMock.mockResolvedValueOnce(json({ ...GRAPH, nodes: [], edges: [], source: "none" }));
    render(<GraphTab globalRunId="m:p:r" />, { wrapper: qc() });
    await waitFor(() => expect(screen.getByTestId("graph-empty")).toBeInTheDocument());
  });
});

describe("ArtifactsTab", () => {
  const INDEX = {
    artifacts: [
      { id: "summary", kind: "markdown", path: ".ralph/agent/summary.md", status: "present", size: 10, mtime: "2026-10-28T12:00:00Z" },
      { id: "missing", kind: "markdown", status: "not_generated" },
      { id: "leak", kind: "markdown", status: "outside_allowed" },
    ],
  };

  it("slot status labels incl. 尚未生成 (S7); preview on select", async () => {
    fetchMock
      .mockResolvedValueOnce(json(INDEX))
      .mockResolvedValueOnce(json({ id: "summary", kind: "markdown", content: "# Hello", truncated: false }));
    const onSelect = vi.fn();
    const Wrap = () => {
      const [sel, setSel] = useState<string | null>(null);
      return (
        <ArtifactsTab
          globalRunId="m:p:r"
          selectedId={sel}
          onSelect={(id: string | null) => {
            setSel(id);
            onSelect(id);
          }}
        />
      );
    };
    render(<Wrap />, { wrapper: qc() });
    await waitFor(() => expect(screen.getByTestId("artifact-status-summary")).toHaveTextContent("已生成"));
    expect(screen.getByTestId("artifact-status-missing")).toHaveTextContent("尚未生成");
    expect(screen.getByTestId("artifact-status-leak")).toHaveTextContent("越界拒绝");
    // missing slot disabled
    expect(screen.getByTestId("artifact-missing")).toBeDisabled();
    fireEvent.click(screen.getByTestId("artifact-summary"));
    expect(onSelect).toHaveBeenCalledWith("summary");
    await waitFor(() => expect(screen.getByTestId("artifact-preview")).toHaveTextContent("Hello"));
  });
});
