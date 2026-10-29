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
  /** Plan4-U2 S13: ralph.yml runtime webhook hint ("none" when absent). */
  runtimeWebhookHint: z.string().default("none"),
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

export const actionDeclSchema = z.object({
  id: z.string(),
  available: z.boolean(),
  reason: z.string().optional(),
  revision: z.string(),
  input_schema: z
    .object({ topics: z.array(z.string()).optional() })
    .nullish(),
});
export type ActionDecl = z.infer<typeof actionDeclSchema>;

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
  actions: z.array(actionDeclSchema).default([]),
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
    /// monitor-descriptor/v1 passthrough — stages/artifacts drive
    /// the detail strip and artifact slots.
    presetDescriptor: z.record(z.string(), z.unknown()).nullable().default(null),
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
  collector: z.object({
    status: z.enum(["ok", "error"]),
    error: z.string().nullable().optional(),
    consecutive_failures: z.number(),
    last_collected_at: z.string().nullable(),
  }),
  degraded_runs: z.array(z.string()),
});
export type ProjectHealth = z.infer<typeof projectHealthSchema>;

// ── Plan3-U2/U3 health + stream contract ──

export const runHealthSchema = z.object({
  run_id: z.string(),
  global_run_id: z.string(),
  layers: z.object({
    collector: z.object({
      status: z.enum(["ok", "error", "stale"]),
      error: z.string().optional(),
      consecutive_failures: z.number(),
      last_collected_at: z.string().nullable(),
    }),
    runtime: z.object({
      status: z.enum([
        "running",
        "silent",
        "exited",
        "dead_unverified",
        "pid_unverified",
        "unknown",
      ]),
      pid: z.number().nullable(),
      pid_alive: z.boolean().nullable(),
      pid_start_verified: z.boolean().nullable(),
      last_event_at: z.string().nullable(),
      last_activity_at: z.string().nullable(),
    }),
    backend: z.object({
      status: z.enum([
        "ok",
        "timeout",
        "exit_error",
        "transport_unknown",
        "unknown",
      ]),
      hint: z.string().optional(),
      pending: z.boolean().optional(),
      last_end_at: z.string().nullable(),
    }),
  }),
  terminal_converged: z.boolean(),
  last_valid_at: z.string().nullable(),
});
export type RunHealth = z.infer<typeof runHealthSchema>;

export const snapshotSchema = z.object({
  watermark: z.number(),
  generated_at: z.string(),
  projects: z.array(
    z.object({
      id: z.string(),
      canonical_path: z.string(),
      display_name: z.string(),
      created_at: z.string(),
      health: projectHealthSchema,
    }),
  ),
  runs: z.array(runSchema),
});
export type MonitorSnapshot = z.infer<typeof snapshotSchema>;

/** One frame of the monitor event stream (SSE `data` payload). */
export const streamEventSchema = z.object({
  kind: z.string(),
  seq: z.number(),
  project_id: z.string().nullable(),
  run_id: z.string().nullable(),
  payload: z.record(z.string(), z.unknown()).nullable(),
});
export type MonitorStreamEvent = z.infer<typeof streamEventSchema>;

// ── Plan2-U2 events/progress/graph contract ──

export const runEventSchema = z.object({
  seq: z.number(),
  ts: z.string().nullable(),
  iteration: z.number().nullable(),
  hat: z.string().nullable(),
  topic: z.string().nullable(),
  payload: z.string().nullable(),
  truncated: z.boolean(),
});
export type MonitorEvent = z.infer<typeof runEventSchema>;

export const eventsResponseSchema = z.object({
  events: z.array(runEventSchema),
  next_cursor: z.number(),
  has_more: z.boolean(),
  watermark: z.string().nullable(),
  evidence_timestamps: z.object({
    last_event_at: z.string().nullable(),
    last_observed_at: z.string().nullable(),
    terminal_at: z.string().nullable(),
    collected_at: z.string().nullable(),
  }),
});
export type EventsResponse = z.infer<typeof eventsResponseSchema>;

export const progressSchema = z.object({
  stage: z.string().nullable(),
  stage_label: z.string().nullable(),
  stage_history: z.array(
    z.object({ stage: z.string(), at_seq: z.number(), round: z.number() }),
  ),
  current_hat: z.string().nullable(),
  iteration: z.number().nullable(),
  elapsed_ms: z.number().nullable(),
  units: z.object({
    done: z.number(),
    total: z.number().nullable(),
    source: z.string(),
  }),
  denominator: z.string(),
});
export type RunProgress = z.infer<typeof progressSchema>;

export const graphSchema = z.object({
  nodes: z.array(
    z.object({ unit: z.string(), status: z.string(), last_seq: z.number() }),
  ),
  edges: z.array(z.object({ from: z.string(), to: z.string() })),
  source: z.string(),
  scheduler: z.record(z.string(), z.unknown()).nullable().optional(),
  illegal_transitions: z.number().default(0),
});
export type RunGraph = z.infer<typeof graphSchema>;

export interface EventsQuery {
  cursor?: number;
  limit?: number;
  hat?: string;
  topic?: string;
  q?: string;
}

// ── Plan2-U3 artifacts contract ──

export const artifactEntrySchema = z.object({
  id: z.string(),
  kind: z.string(),
  path: z.string().optional(),
  status: z.enum(["present", "not_generated", "outside_allowed"]),
  size: z.number().optional(),
  mtime: z.string().optional(),
});
export type ArtifactEntry = z.infer<typeof artifactEntrySchema>;

export const artifactsResponseSchema = z.object({
  artifacts: z.array(artifactEntrySchema),
});
export type ArtifactsResponse = z.infer<typeof artifactsResponseSchema>;

export const artifactContentSchema = z.object({
  id: z.string(),
  kind: z.string(),
  content: z.string(),
  truncated: z.boolean(),
});
export type ArtifactContent = z.infer<typeof artifactContentSchema>;

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

export function apiGetRunHealth(globalRunId: string): Promise<RunHealth> {
  return request(
    `/runs/${encodeURIComponent(globalRunId)}/health`,
    runHealthSchema,
  );
}

export function apiGetSnapshot(): Promise<MonitorSnapshot> {
  return request("/snapshot", snapshotSchema);
}

// ── Plan4 notifications contract ──

export const NOTIFICATION_STATES = [
  "unread",
  "read",
  "muted",
  "resolved",
] as const;
export type NotificationState = (typeof NOTIFICATION_STATES)[number];

export const notificationSchema = z.object({
  id: z.string(),
  kind: z.string(),
  project_id: z.string(),
  run_gid: z.string().nullable(),
  pending_interaction_id: z.string().nullable(),
  title: z.string(),
  body: z.string().nullable(),
  detail: z.record(z.string(), z.unknown()).nullable(),
  state: z.enum(NOTIFICATION_STATES),
  created_at: z.string(),
  resolved_at: z.string().nullable(),
  link: z.string().nullable(),
});
export type MonitorNotification = z.infer<typeof notificationSchema>;

export const notificationsResponseSchema = z.object({
  notifications: z.array(notificationSchema),
  total: z.number(),
  limit: z.number(),
  offset: z.number(),
});

export const pendingInteractionSchema = z.object({
  id: z.string(),
  project_id: z.string(),
  session_id: z.string(),
  run_gid: z.string().nullable(),
  prompt: z.string().nullable(),
  status: z.string(),
  created_at: z.string(),
  link: z.string().nullable(),
});
export type PendingInteraction = z.infer<typeof pendingInteractionSchema>;

export const channelConfigSchema = z.object({
  url: z.string().nullable(),
  secret: z.string().nullable(),
  enabled: z.boolean(),
  last_error: z.string().nullable().optional(),
});
export type ChannelConfig = z.infer<typeof channelConfigSchema>;

export const deliverySchema = z.object({
  id: z.string(),
  channel: z.string(),
  state: z.string(),
  attempts: z.number(),
  next_attempt_at: z.string().nullable(),
  delivery_id: z.string().nullable(),
  error: z.string().nullable(),
  created_at: z.string(),
  updated_at: z.string(),
});
export type Delivery = z.infer<typeof deliverySchema>;

export interface NotificationsQuery {
  state?: string;
  kind?: string;
  q?: string;
  project?: string;
  limit?: number;
  offset?: number;
}

export function apiGetNotifications(
  q: NotificationsQuery = {},
): Promise<z.infer<typeof notificationsResponseSchema>> {
  const params = new URLSearchParams();
  for (const [k, v] of Object.entries(q)) {
    if (v !== undefined && v !== "") params.set(k, String(v));
  }
  const s = params.toString();
  return request(
    `/notifications${s ? `?${s}` : ""}`,
    notificationsResponseSchema,
  );
}

export function apiGetPending(): Promise<PendingInteraction[]> {
  return request(
    "/pending-interactions",
    z.object({ pending: z.array(pendingInteractionSchema) }),
  ).then((r) => r.pending);
}

function post(path: string, schema: z.ZodType): Promise<unknown> {
  return request(path, schema, { method: "POST" });
}

export function apiReadNotification(id: string): Promise<unknown> {
  return post(
    `/notifications/${encodeURIComponent(id)}/read`,
    z.object({ notification: notificationSchema }),
  );
}

export function apiMuteNotification(id: string): Promise<unknown> {
  return post(
    `/notifications/${encodeURIComponent(id)}/mute`,
    z.object({ notification: notificationSchema }),
  );
}

export function apiReadAllNotifications(): Promise<{ updated: number }> {
  return post("/notifications/read-all", z.object({ updated: z.number() })) as Promise<{
    updated: number;
  }>;
}

export function apiGetChannelConfig(): Promise<ChannelConfig> {
  return request("/notifications/channels/webhook", channelConfigSchema);
}

export function apiPutChannelConfig(cfg: {
  url: string;
  secret?: string | null;
  enabled?: boolean;
}): Promise<ChannelConfig> {
  return request("/notifications/channels/webhook", channelConfigSchema, {
    method: "PUT",
    body: JSON.stringify(cfg),
  });
}

export function apiTestWebhook(url?: string): Promise<{
  delivery_id: string;
  outcome: string;
  error?: string;
  status?: number;
}> {
  return request(
    "/notifications/channels/webhook/test",
    z.object({
      delivery_id: z.string(),
      outcome: z.string(),
      error: z.string().optional(),
      status: z.number().optional(),
    }),
    { method: "POST", body: JSON.stringify(url ? { url } : {}) },
  );
}

export function apiGetDeliveries(notificationId: string): Promise<Delivery[]> {
  return request(
    `/notifications/${encodeURIComponent(notificationId)}/deliveries`,
    z.object({ deliveries: z.array(deliverySchema) }),
  ).then((r) => r.deliveries);
}

export function apiRetryDelivery(deliveryId: string): Promise<unknown> {
  return post(
    `/deliveries/${encodeURIComponent(deliveryId)}/retry`,
    z.object({ retried: z.string() }),
  );
}

export function apiGetEvents(
  globalRunId: string,
  query: EventsQuery = {},
): Promise<EventsResponse> {
  const params = new URLSearchParams();
  for (const [k, v] of Object.entries(query)) {
    if (v !== undefined && v !== "") params.set(k, String(v));
  }
  const s = params.toString();
  return request(
    `/runs/${encodeURIComponent(globalRunId)}/events${s ? `?${s}` : ""}`,
    eventsResponseSchema,
  );
}

export function apiGetProgress(globalRunId: string): Promise<RunProgress> {
  return request(
    `/runs/${encodeURIComponent(globalRunId)}/progress`,
    progressSchema,
  );
}

export function apiGetGraph(globalRunId: string): Promise<RunGraph> {
  return request(
    `/runs/${encodeURIComponent(globalRunId)}/graph`,
    graphSchema,
  );
}

export function apiGetArtifacts(globalRunId: string): Promise<ArtifactEntry[]> {
  return request(
    `/runs/${encodeURIComponent(globalRunId)}/artifacts`,
    artifactsResponseSchema,
  ).then((r) => r.artifacts);
}

export function apiGetArtifactContent(
  globalRunId: string,
  artifactId: string,
): Promise<ArtifactContent> {
  return request(
    `/runs/${encodeURIComponent(globalRunId)}/artifacts/${encodeURIComponent(artifactId)}/content`,
    artifactContentSchema,
  );
}

export function apiRegisterProject(path: string, displayName?: string): Promise<MonitorProject> {
  return request("/projects", z.object({ project: projectSchema }), {
    method: "POST",
    body: JSON.stringify({ path, ...(displayName ? { displayName } : {}) }),
  }).then((r) => r.project);
}

// ── Plan5-U2/U3 operations + presets contract ──

export const OP_STATES = [
  "recorded",
  "dispatching",
  "dispatched",
  "confirmed",
  "failed",
  "confirm_timeout",
] as const;
export type OperationState = (typeof OP_STATES)[number];

export const operationSchema = z.object({
  id: z.string(),
  idempotencyKey: z.string(),
  kind: z.enum(["launch", "stop", "retry", "merge", "emit"]),
  projectId: z.string(),
  runGid: z.string().nullable(),
  clientRequestId: z.string().nullable(),
  params: z.record(z.string(), z.unknown()),
  expectedRevision: z.string().nullable(),
  state: z.enum(OP_STATES),
  receipt: z.record(z.string(), z.unknown()).nullable(),
  actor: z.string().nullable(),
  origin: z.string().nullable(),
  error: z.string().nullable(),
  createdAt: z.string(),
  dispatchedAt: z.string().nullable(),
  confirmedAt: z.string().nullable(),
});
export type MonitorOperation = z.infer<typeof operationSchema>;

export const operationsResponseSchema = z.object({
  operations: z.array(operationSchema),
});

export interface CreateOperationInput {
  kind: "launch" | "stop" | "retry" | "merge" | "emit";
  project_id: string;
  run_gid?: string;
  params?: Record<string, unknown>;
  idempotency_key: string;
  expected_revision?: string;
}

export function apiCreateOperation(
  input: CreateOperationInput,
): Promise<MonitorOperation> {
  return request("/operations", z.object({ operation: operationSchema }), {
    method: "POST",
    body: JSON.stringify(input),
  }).then((r) => r.operation);
}

export function apiGetOperations(query: {
  project_id?: string;
  run_gid?: string;
  state?: string;
} = {}): Promise<MonitorOperation[]> {
  const params = new URLSearchParams();
  for (const [k, v] of Object.entries(query)) {
    if (v !== undefined && v !== "") params.set(k, v);
  }
  const s = params.toString();
  return request(
    `/operations${s ? `?${s}` : ""}`,
    operationsResponseSchema,
  ).then((r) => r.operations);
}

export function apiGetOperation(id: string): Promise<MonitorOperation> {
  return request(
    `/operations/${encodeURIComponent(id)}`,
    z.object({ operation: operationSchema }),
  ).then((r) => r.operation);
}

export function apiRecheckOperation(id: string): Promise<MonitorOperation> {
  return request(
    `/operations/${encodeURIComponent(id)}/recheck`,
    z.object({ operation: operationSchema }),
    { method: "POST" },
  ).then((r) => r.operation);
}

export const presetEntrySchema = z.object({
  id: z.string(),
  name: z.string(),
  source: z.enum(["builtin", "custom"]),
  description: z.string().optional(),
});
export type PresetEntry = z.infer<typeof presetEntrySchema>;

export const presetCatalogSchema = z.object({
  presets: z.array(presetEntrySchema),
  fetched_at: z.string(),
  builtin_error: z.string().nullable(),
});
export type PresetCatalog = z.infer<typeof presetCatalogSchema>;

export function apiGetPresets(projectId: string): Promise<PresetCatalog> {
  return request(
    `/projects/${encodeURIComponent(projectId)}/presets`,
    presetCatalogSchema,
  );
}
