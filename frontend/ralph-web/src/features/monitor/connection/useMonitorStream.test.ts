import { describe, expect, it, vi } from "vitest";
import { QueryClient } from "@tanstack/react-query";
import { splitSseFrames } from "./sseClient";
import {
  backoffDelay,
  invalidationsFor,
  startMonitorStream,
  type MonitorStreamState,
} from "./useMonitorStream";
import type { MonitorSnapshot } from "../api";

function snap(watermark: number): MonitorSnapshot {
  return {
    watermark,
    generated_at: "2026-10-28T00:00:00Z",
    projects: [],
    runs: [],
  };
}

const sleepNow = () => Promise.resolve();

describe("splitSseFrames", () => {
  it("parses multi-field frames and keeps a partial tail", () => {
    const [frames, rest] = splitSseFrames(
      "id: 7\nevent: run.updated\ndata: {\"a\":1}\n\ndata: partial",
    );
    expect(frames).toEqual([{ id: "7", event: "run.updated", data: '{"a":1}' }]);
    expect(rest).toBe("data: partial");
  });

  it("joins multi-line data and skips comments", () => {
    const [frames] = splitSseFrames(": hb\n\ndata: a\ndata: b\n\n");
    expect(frames).toEqual([{ data: "a\nb" }]);
  });
});

describe("invalidationsFor", () => {
  it("maps kinds to query-key prefixes", () => {
    expect(invalidationsFor("run.events")).toContainEqual(["monitor", "events"]);
    expect(invalidationsFor("run.deleted")).toEqual(
      ["runs", "run", "progress", "graph", "events", "artifacts"].map((k) => [
        "monitor",
        k,
      ]),
    );
    expect(invalidationsFor("project.health")).toContainEqual([
      "monitor",
      "health",
    ]);
    expect(invalidationsFor("unknown.kind")).toEqual([]);
  });
});

describe("backoffDelay", () => {
  it("grows exponentially and caps at max", () => {
    vi.spyOn(Math, "random").mockReturnValue(0.5); // zero jitter
    expect(backoffDelay(0, 500, 30_000, 0.25)).toBeCloseTo(500, 0);
    expect(backoffDelay(1, 500, 30_000, 0.25)).toBeCloseTo(1000, 0);
    expect(backoffDelay(20, 500, 30_000, 0.25)).toBeLessThanOrEqual(30_000);
    vi.restoreAllMocks();
  });
});

describe("startMonitorStream driver", () => {
  function driver(opts: {
    snaps?: MonitorSnapshot[];
    frames?: string[];
    failStream?: number;
  }) {
    const states: MonitorStreamState[] = [];
    let snapCalls = 0;
    const qc = new QueryClient();
    const invalid = vi.spyOn(qc, "invalidateQueries");
    const drv = startMonitorStream(qc, {
      fetchSnapshot: async () =>
        opts.snaps?.[Math.min(snapCalls++, (opts.snaps?.length ?? 1) - 1)] ??
        snap(5),
      openStream: async (_url, o) => {
        let fails = opts.failStream ?? 0;
        if (fails-- > 0) throw new Error("net down");
        for (const raw of opts.frames ?? []) {
          const [frames] = splitSseFrames(raw + "\n\n");
          for (const f of frames) o.onFrame(f);
        }
        // Clean close → driver reconnects; stop after first pass.
        drv.stop();
      },
      sleep: sleepNow,
      onChange: (s) => states.push(s),
      baseDelayMs: 0,
      maxDelayMs: 0,
      jitter: 0,
    });
    return { drv, states, invalid, qc, snapCallsRef: () => snapCalls };
  }

  it("seeds baseline, applies frames after watermark, dedupes seqs", async () => {
    const { states, invalid } = driver({
      frames: [
        "id: 4\nevent: run.updated\ndata: {}", // ≤ watermark → deduped
        "id: 6\nevent: run.events\ndata: {}",
      ],
    });
    await vi.waitFor(() => {
      expect(states.some((s) => s.status === "connected")).toBe(true);
    });
    const kinds = invalid.mock.calls
      .map((c) => (c[0]?.queryKey as string[] | undefined)?.[1])
      .join(",");
    expect(kinds).toContain("events"); // seq 6 dispatched
    expect(kinds).not.toContain("run"); // seq 4 deduped → no run.updated
  });

  it("resync: resync_required frame → re-snapshot without backoff", async () => {
    let streams = 0;
    const states: MonitorStreamState[] = [];
    const qc = new QueryClient();
    let snapCalls = 0;
    const drv = startMonitorStream(qc, {
      fetchSnapshot: async () => {
        snapCalls += 1;
        return snap(snapCalls === 1 ? 5 : 99);
      },
      openStream: async (_url, o) => {
        streams += 1;
        if (streams === 1) {
          o.onFrame({ event: "resync_required", data: "{}" });
          return;
        }
        drv.stop();
      },
      sleep: sleepNow,
      onChange: (s) => states.push(s),
      baseDelayMs: 0,
      maxDelayMs: 0,
      jitter: 0,
    });
    await vi.waitFor(() => expect(snapCalls).toBe(2));
    expect(states.some((s) => s.status === "connected")).toBe(true);
  });

  it("failed snapshots escalate to offline after retries", async () => {
    const states: MonitorStreamState[] = [];
    const qc = new QueryClient();
    const drv = startMonitorStream(qc, {
      fetchSnapshot: async () => {
        if (states.length > 6) drv.stop();
        throw new Error("down");
      },
      openStream: async () => {},
      sleep: sleepNow,
      onChange: (s) => states.push(s),
      baseDelayMs: 0,
      maxDelayMs: 0,
      jitter: 0,
    });
    await vi.waitFor(() => {
      expect(states[states.length - 1].status).toBe("offline");
    });
  });
});
