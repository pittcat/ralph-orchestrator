/**
 * api.test.ts — envelope unwrap, CSRF header injection, typed errors.
 */

import { describe, it, expect, vi, beforeEach, afterEach } from "vitest";
import {
  apiGetProjects,
  apiGetRuns,
  apiRegisterProject,
  MonitorApiError,
} from "./api";

function jsonResponse(status: number, body: unknown): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

const fetchMock = vi.fn();

beforeEach(() => {
  vi.stubGlobal("fetch", fetchMock);
  fetchMock.mockReset();
  document.cookie = "monitor_csrf=tok-123; path=/";
});

afterEach(() => {
  vi.unstubAllGlobals();
  document.cookie = "monitor_csrf=; max-age=0";
});

describe("monitor api client", () => {
  it("GET unwraps {projects} envelope", async () => {
    fetchMock.mockResolvedValueOnce(
      jsonResponse(200, {
        projects: [
          {
            id: "proj_1",
            canonicalPath: "/w/a",
            displayName: "a",
            createdAt: "t",
          },
        ],
      }),
    );
    const projects = await apiGetProjects();
    expect(projects).toHaveLength(1);
    expect(projects[0].canonicalPath).toBe("/w/a");
    const [url, init] = fetchMock.mock.calls[0];
    expect(url).toBe("/api/monitor/v1/projects");
    expect(new Headers(init.headers).has("x-monitor-csrf")).toBe(false);
  });

  it("POST injects x-monitor-csrf from the cookie + JSON content-type", async () => {
    fetchMock.mockResolvedValueOnce(
      jsonResponse(201, {
        project: {
          id: "proj_2",
          canonicalPath: "/w/b",
          displayName: "b",
          createdAt: "t",
        },
      }),
    );
    await apiRegisterProject("/w/b");
    const [, init] = fetchMock.mock.calls[0];
    const headers = new Headers(init.headers);
    expect(headers.get("x-monitor-csrf")).toBe("tok-123");
    expect(headers.get("content-type")).toBe("application/json");
  });

  it("error envelope unwraps into MonitorApiError with code/details", async () => {
    fetchMock.mockResolvedValueOnce(
      jsonResponse(409, {
        error: {
          code: "project_exists",
          message: "dup",
          details: { projectId: "proj_9" },
        },
      }),
    );
    await expect(apiRegisterProject("/w/c")).rejects.toMatchObject({
      name: "MonitorApiError",
      status: 409,
      code: "project_exists",
      details: { projectId: "proj_9" },
    });
  });

  it("404 on run detail → typed run_not_found", async () => {
    fetchMock.mockResolvedValueOnce(
      jsonResponse(404, { error: { code: "run_not_found", message: "no" } }),
    );
    const err = await apiGetRuns({}).then(
      () => null,
      (e: unknown) => e,
    );
    expect(err).toBeInstanceOf(MonitorApiError);
    expect((err as MonitorApiError).status).toBe(404);
    expect((err as MonitorApiError).code).toBe("run_not_found");
  });

  it("non-envelope error body → http_error fallback", async () => {
    fetchMock.mockResolvedValueOnce(
      jsonResponse(500, { unexpected: true }),
    );
    const err = await apiGetProjects().then(
      () => null,
      (e: unknown) => e,
    );
    expect((err as MonitorApiError).code).toBe("http_error");
    expect((err as MonitorApiError).status).toBe(500);
  });

  it("query params serialize filters", async () => {
    fetchMock.mockResolvedValueOnce(
      jsonResponse(200, { runs: [], total: 0, watermark: null }),
    );
    await apiGetRuns({
      project: "p1",
      state: "exited_failed",
      q: "kw",
      sort: "-startedAt",
      limit: 10,
      offset: 20,
    });
    const [url] = fetchMock.mock.calls[0];
    expect(url).toContain("project=p1");
    expect(url).toContain("state=exited_failed");
    expect(url).toContain("q=kw");
    expect(url).toContain("limit=10");
    expect(url).toContain("offset=20");
  });
});
