/**
 * HealthPanel — the four-layer health strip for a run detail header
 * (Plan3-U4): collector / runtime / backend badges plus converged and
 * last-valid context. Unknown stays muted; terminal runs that
 * converged suppress the "silent" scare copy (plan D9/S12).
 */

import type { RunHealth } from "../api";
import { LayerBadge, type LayerTone } from "./LayerBadge";

const COLLECTOR_TONE: Record<string, LayerTone> = {
  ok: "ok",
  stale: "warn",
  error: "bad",
};
const RUNTIME_TONE: Record<string, LayerTone> = {
  running: "ok",
  exited: "ok",
  silent: "warn",
  dead_unverified: "bad",
  pid_unverified: "warn",
  unknown: "muted",
};
const BACKEND_TONE: Record<string, LayerTone> = {
  ok: "ok",
  timeout: "bad",
  exit_error: "bad",
  transport_unknown: "warn",
  unknown: "muted",
};

export function HealthPanel({ health }: { health: RunHealth }) {
  const { collector, runtime, backend } = health.layers;
  const converged = health.terminal_converged;

  // Terminal-converged runs: a silent runtime is expected, not an
  // alert — render it exited-equivalent (D9 suppression).
  const runtimeStatus =
    converged && runtime.status === "silent" ? "exited" : runtime.status;

  return (
    <div
      data-testid="health-panel"
      className="flex flex-wrap items-center gap-2"
    >
      <LayerBadge
        layer="collector"
        status={collector.status}
        tone={COLLECTOR_TONE[collector.status] ?? "muted"}
        title={
          collector.error ??
          `collector: ${collector.status}${
            collector.last_collected_at
              ? ` @ ${collector.last_collected_at}`
              : ""
          }`
        }
      />
      <LayerBadge
        layer="runtime"
        status={runtimeStatus}
        tone={RUNTIME_TONE[runtimeStatus] ?? "muted"}
        title={
          runtime.pid != null
            ? `runtime: ${runtimeStatus} (pid ${runtime.pid})`
            : `runtime: ${runtimeStatus}`
        }
      />
      <LayerBadge
        layer="backend"
        status={backend.pending ? "pending" : backend.status}
        tone={
          backend.pending ? "warn" : (BACKEND_TONE[backend.status] ?? "muted")
        }
        title={backend.hint ?? `backend: ${backend.status}`}
      />
      {converged ? (
        <span
          data-testid="converged-chip"
          className="rounded-full border border-border px-2 py-0.5 text-xs text-muted-foreground"
        >
          converged
        </span>
      ) : null}
      {health.last_valid_at ? (
        <span className="text-xs text-muted-foreground">
          last valid {health.last_valid_at}
        </span>
      ) : null}
    </div>
  );
}
