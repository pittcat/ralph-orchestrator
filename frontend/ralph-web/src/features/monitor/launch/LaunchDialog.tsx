/**
 * Plan5-U3 LaunchDialog: 项目 → preset → 参数 → 确认 → 提交。
 * idempotency_key 随 dialog 打开生成一次（重试提交复用同键；
 * 重新打开生成新键）。提交后进入 inline 等待：confirmed → 跳转
 * run 详情；failed/confirm_timeout → 内联错误 + recheck。
 */

import { useEffect, useRef, useState } from "react";
import { useNavigate } from "react-router-dom";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Textarea } from "@/components/ui/textarea";
import {
  MonitorApiError,
  type MonitorProject,
  type PresetEntry,
} from "../api";
import { useCreateOperation, useMonitorProjects, useOperation, usePresets } from "../hooks";
import { newIdempotencyKey } from "@/lib/utils";

const PROMPT_MAX = 4000;

type Step = 0 | 1 | 2 | 3;
const STEP_LABEL = ["项目", "Preset", "参数", "确认"] as const;

export function LaunchDialog({
  open,
  onOpenChange,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const navigate = useNavigate();
  const projects = useMonitorProjects();
  const [step, setStep] = useState<Step>(0);
  const [project, setProject] = useState<MonitorProject | null>(null);
  const [preset, setPreset] = useState<PresetEntry | null>(null);
  const [prompt, setPrompt] = useState("");
  const [plan, setPlan] = useState("");
  const [maxIterations, setMaxIterations] = useState("");
  const [backend, setBackend] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [opId, setOpId] = useState<string | null>(null);
  const idemKey = useRef<string>("");

  // Per-open intent key + reset (S8/D6).
  useEffect(() => {
    if (open) {
      idemKey.current = newIdempotencyKey();
      setStep(0);
      setProject(null);
      setPreset(null);
      setPrompt("");
      setPlan("");
      setMaxIterations("");
      setBackend("");
      setError(null);
      setOpId(null);
    }
  }, [open]);

  const presets = usePresets(project?.id);
  const create = useCreateOperation();
  const pending = useOperation(opId ?? undefined);

  // Confirmed launch → navigate to the run detail.
  useEffect(() => {
    const op = pending.data;
    if (!op) return;
    if (op.state === "confirmed" && op.runGid) {
      onOpenChange(false);
      navigate(`/monitor/runs/${encodeURIComponent(op.runGid)}`);
    } else if (op.state === "failed" || op.state === "confirm_timeout") {
      setError(op.error ?? `operation ${op.state}`);
    }
  }, [pending.data, navigate, onOpenChange]);

  const stepValid =
    (step === 0 && project !== null) ||
    (step === 1 && preset !== null) ||
    (step === 2 &&
      (prompt.trim() !== "" || plan.trim() !== "") &&
      prompt.length <= PROMPT_MAX &&
      (maxIterations === "" || /^\d+$/.test(maxIterations))) ||
    step === 3;

  const submit = () => {
    if (!project || !preset) return;
    setError(null);
    const params: Record<string, unknown> = { preset: preset.name };
    if (prompt.trim()) params.prompt = prompt;
    if (plan.trim()) params.plan = plan.trim();
    if (maxIterations) params.max_iterations = Number(maxIterations);
    if (backend.trim()) params.backend = backend.trim();
    create.mutate(
      {
        kind: "launch",
        project_id: project.id,
        idempotency_key: idemKey.current,
        params,
      },
      {
        onSuccess: (op) => setOpId(op.id),
        onError: (e) =>
          setError(
            e instanceof MonitorApiError ? e.message : "提交失败，请重试",
          ),
      },
    );
  };

  const waiting = opId !== null && !error;

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent data-testid="launch-dialog">
        <DialogHeader>
          <DialogTitle>启动运行</DialogTitle>
          <DialogDescription>
            {waiting
              ? "运行已分发，等待 runtime 确认…"
              : `第 ${step + 1} 步 / 4：${STEP_LABEL[step]}`}
          </DialogDescription>
        </DialogHeader>

        {waiting ? (
          <div data-testid="launch-waiting" className="py-6 text-center text-sm text-muted-foreground">
            {pending.data?.state ?? "dispatching"} — 正在确认 manifest…
          </div>
        ) : (
          <div className="space-y-3">
            {step === 0 && (
              <ul className="max-h-64 space-y-1 overflow-auto" data-testid="launch-projects">
                {(projects.data ?? []).map((p) => (
                  <li key={p.id}>
                    <button
                      type="button"
                      onClick={() => {
                        setProject(p);
                        setError(null);
                      }}
                      aria-pressed={project?.id === p.id}
                      className={`w-full rounded border px-3 py-2 text-left text-sm ${
                        project?.id === p.id
                          ? "border-primary bg-accent"
                          : "border-border hover:bg-accent/50"
                      }`}
                    >
                      <span className="font-medium">{p.displayName}</span>
                      <span className="ml-2 font-mono text-xs text-muted-foreground">
                        {p.canonicalPath}
                      </span>
                    </button>
                  </li>
                ))}
                {projects.data?.length === 0 && (
                  <li className="text-sm text-muted-foreground">
                    尚未注册项目。
                  </li>
                )}
              </ul>
            )}

            {step === 1 && (
              <ul className="max-h-64 space-y-1 overflow-auto" data-testid="launch-presets">
                {(presets.data?.presets ?? []).map((p) => (
                  <li key={p.id}>
                    <button
                      type="button"
                      onClick={() => {
                        setPreset(p);
                        setError(null);
                      }}
                      aria-pressed={preset?.id === p.id}
                      className={`w-full rounded border px-3 py-2 text-left text-sm ${
                        preset?.id === p.id
                          ? "border-primary bg-accent"
                          : "border-border hover:bg-accent/50"
                      }`}
                    >
                      <span className="font-medium">{p.name}</span>
                      <span className="ml-2 text-xs text-muted-foreground">
                        {p.source}
                      </span>
                    </button>
                  </li>
                ))}
                {presets.data && presets.data.presets.length === 0 && (
                  <li className="text-sm text-muted-foreground">
                    该项目没有可用 preset。
                  </li>
                )}
                {presets.data?.builtin_error && (
                  <li className="text-xs text-amber-600">
                    builtin 列表不可用：{presets.data.builtin_error}
                  </li>
                )}
              </ul>
            )}

            {step === 2 && (
              <div className="space-y-3" data-testid="launch-params">
                <label className="block space-y-1 text-sm">
                  <span>Prompt</span>
                  <Textarea
                    value={prompt}
                    onChange={(e) => setPrompt(e.target.value)}
                    rows={5}
                    maxLength={PROMPT_MAX}
                    data-testid="launch-prompt"
                  />
                  <span className="text-xs text-muted-foreground">
                    {prompt.length}/{PROMPT_MAX}
                  </span>
                </label>
                <label className="block space-y-1 text-sm">
                  <span>Plan 路径（项目内，可选）</span>
                  <Input
                    value={plan}
                    onChange={(e) => setPlan(e.target.value)}
                    placeholder="docs/plans/….md"
                    data-testid="launch-plan"
                  />
                </label>
                <div className="flex gap-3">
                  <label className="block flex-1 space-y-1 text-sm">
                    <span>max_iterations</span>
                    <Input
                      value={maxIterations}
                      onChange={(e) => setMaxIterations(e.target.value)}
                      inputMode="numeric"
                      data-testid="launch-max-iterations"
                    />
                  </label>
                  <label className="block flex-1 space-y-1 text-sm">
                    <span>backend（可选）</span>
                    <Input
                      value={backend}
                      onChange={(e) => setBackend(e.target.value)}
                      data-testid="launch-backend"
                    />
                  </label>
                </div>
              </div>
            )}

            {step === 3 && (
              <dl
                className="space-y-2 rounded border border-border p-3 text-sm"
                data-testid="launch-review"
              >
                <div className="flex gap-2">
                  <dt className="w-24 text-muted-foreground">项目</dt>
                  <dd>{project?.displayName}</dd>
                </div>
                <div className="flex gap-2">
                  <dt className="w-24 text-muted-foreground">preset</dt>
                  <dd className="font-mono">{preset?.name}</dd>
                </div>
                <div className="flex gap-2">
                  <dt className="w-24 text-muted-foreground">prompt</dt>
                  <dd className="truncate">{prompt.slice(0, 120) || "—"}</dd>
                </div>
                <div className="flex gap-2">
                  <dt className="w-24 text-muted-foreground">plan</dt>
                  <dd className="font-mono">{plan || "—"}</dd>
                </div>
              </dl>
            )}

            {error && (
              <p className="text-sm text-red-600" role="alert" data-testid="launch-error">
                {error}
              </p>
            )}

            <div className="flex justify-between pt-1">
              <button
                type="button"
                onClick={() => setStep((s) => Math.max(0, s - 1) as Step)}
                disabled={step === 0}
                className="rounded border border-border px-3 py-1.5 text-sm disabled:opacity-40"
              >
                上一步
              </button>
              {step < 3 ? (
                <button
                  type="button"
                  onClick={() => {
                    if (!stepValid) {
                      setError(
                        step === 2
                          ? "需要 prompt 或 plan 路径"
                          : "请先完成当前步骤",
                      );
                      return;
                    }
                    setError(null);
                    setStep((s) => Math.min(3, s + 1) as Step);
                  }}
                  data-testid="launch-next"
                  className="rounded bg-primary px-3 py-1.5 text-sm text-primary-foreground"
                >
                  下一步
                </button>
              ) : (
                <button
                  type="button"
                  onClick={submit}
                  disabled={create.isPending}
                  data-testid="launch-submit"
                  className="rounded bg-primary px-3 py-1.5 text-sm text-primary-foreground disabled:opacity-50"
                >
                  {create.isPending ? "提交中…" : "启动"}
                </button>
              )}
            </div>
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}
