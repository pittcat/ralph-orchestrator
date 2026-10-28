/**
 * Monitor REST client — typed fetch wrapper over `/api/monitor/v1`.
 *
 * Matches the U2/U3 server contract: unsafe methods replay the
 * `monitor_csrf` cookie via the `x-monitor-csrf` header (double
 * submit), and failures unwrap the `{error:{code,message,details}}`
 * envelope into `MonitorApiError`. Responses are validated with zod
 * so contract drift surfaces as `schema_error`, not silent bad data.
 */

import { z } from "zod";

export const MONITOR_API_BASE = "/api/monitor/v1";

export class MonitorApiError extends Error {
  constructor(
    public status: number,
    public code: string,
    message: string,
    public details?: unknown,
  ) {
    super(message);
    this.name = "MonitorApiError";
  }
}

const errorEnvelope = z.object({
  error: z.object({
    code: z.string(),
    message: z.string(),
    details: z.unknown().optional(),
  }),
});

function csrfToken(): string | null {
  const match = document.cookie.match(/(?:^|;\s*)monitor_csrf=([^;]+)/);
  return match ? decodeURIComponent(match[1]) : null;
}

async function request<T>(
  path: string,
  schema: z.ZodType<T>,
  init: RequestInit = {},
): Promise<T> {
  const headers = new Headers(init.headers);
  const method = (init.method ?? "GET").toUpperCase();
  if (method !== "GET" && method !== "HEAD" && method !== "OPTIONS") {
    const token = csrfToken();
    if (token) headers.set("x-monitor-csrf", token);
    if (init.body && !headers.has("content-type")) {
      headers.set("content-type", "application/json");
    }
  }

  const res = await fetch(`${MONITOR_API_BASE}${path}`, {
    credentials: "same-origin",
    ...init,
    headers,
  });

  if (!res.ok) {
    const body = await res.json().catch(() => null);
    const parsed = errorEnvelope.safeParse(body);
    if (parsed.success) {
      const { code, message, details } = parsed.data.error;
      throw new MonitorApiError(res.status, code, message, details);
    }
    throw new MonitorApiError(
      res.status,
      "http_error",
      `request failed with status ${res.status}`,
    );
  }
  if (res.status === 204) {
    return schema.parse(undefined) as T;
  }
  const body = await res.json();
  return schema.parse(body);
}

// ── Schemas mirroring the U3 REST contract ──

export const projectSchema = z.object({
  id: z.string(),
  canonicalPath: z.string(),
  displayName: z.string(),
  createdAt: z.string(),
});
export type MonitorProject = z.infer<typeof projectSchema>;

export const MONITOR_RUN_STATES = [
  "running",
  "silent",
  "exited_success",
  "exited_failed",
  "exited_merged",
  "exited_unknown",
] as const;
export type MonitorRunState = (typeof MONITOR_RUN_STATES)[number];

export const runSchema = z.object({
  globalRunId: z.string(),
  projectId: z.string(),
  runId: z.string(),
  kind: z.string(),
  state: z.enum(MONITOR_RUN_STATES).catch("exited_unknown"),
  stateEvidence: z.record(z.string(), z.unknown()).default({}),
  cwd: z.string().nullable(),
  repoRoot: z.string().nullable(),
  worktreePath: z.string().nullable(),
  promptPreview: z.string().nullable(),
  promptSha256: z.string().nullable(),
  preset: z.string().nullable(),
  planPath: z.string().nullable(),
  startedAt: z.string().nullable(),
  terminal: z
    .object({
      reason: z.string(),
      at: z.string(),
      merge_commit: z.string().optional(),
    })
    .nullable(),
  lastEventAt: z.string().nullable(),
  lastObservedAt: z.string(),
  manifestPath: z.string().nullable(),
  stale: z.boolean(),
});
export type MonitorRun = z.infer<typeof runSchema>;

export const runsResponseSchema = z.object({
  runs: z.array(runSchema),
  total: z.number(),
  watermark: z.string().nullable(),
});
export type RunsResponse = z.infer<typeof runsResponseSchema>;

export const runDetailSchema = z.object({
  run: runSchema.extend({
    evidenceTimestamps: z.object({
      lastEventAt: z.string().nullable(),
      lastObservedAt: z.string().nullable(),
      terminalAt: z.string().nullable(),
      collectedAt: z.string().nullable(),
    }),
  }),
});
export type RunDetail = z.infer<typeof runDetailSchema>;

export const projectsResponseSchema = z.object({
  projects: z.array(projectSchema),
});

export const projectHealthSchema = z.object({
  collector: z.enum(["ok", "error"]),
  error: z.string().nullable().optional(),
  last_collected_at: z.string().nullable(),
});
export type ProjectHealth = z.infer<typeof projectHealthSchema>;

// ── Typed calls ──

export interface RunsQuery {
  project?: string;
  state?: string;
  q?: string;
  sort?: "startedAt" | "-startedAt";
  limit?: number;
  offset?: number;
}

function toSearchParams(query: RunsQuery): string {
  const params = new URLSearchParams();
  for (const [k, v] of Object.entries(query)) {
    if (v !== undefined && v !== "") params.set(k, String(v));
  }
  const s = params.toString();
  return s ? `?${s}` : "";
}

export function apiGetProjects(): Promise<MonitorProject[]> {
  return request("/projects", projectsResponseSchema).then((r) => r.projects);
}

export function apiGetRuns(query: RunsQuery = {}): Promise<RunsResponse> {
  return request(`/runs${toSearchParams(query)}`, runsResponseSchema);
}

export function apiGetRun(globalRunId: string): Promise<RunDetail> {
  return request(`/runs/${encodeURIComponent(globalRunId)}`, runDetailSchema);
}

export function apiGetProjectHealth(projectId: string): Promise<ProjectHealth> {
  return request(`/projects/${projectId}/health`, projectHealthSchema);
}

export function apiRegisterProject(path: string, displayName?: string): Promise<MonitorProject> {
  return request("/projects", z.object({ project: projectSchema }), {
    method: "POST",
    body: JSON.stringify({ path, ...(displayName ? { displayName } : {}) }),
  }).then((r) => r.project);
}
