/**
 * Plan5-U3 ConfirmDialog: 展示将执行的动作摘要 + revision 截断
 * （expected_revision 隐性携带），per-kind 参数输入，提交后显示
 * 终态（confirmed/failed/receipt 摘要）。409 revision_conflict →
 * 提示刷新后重试。idempotency_key 每次打开生成一个。
 */

import { useEffect, useRef, useState } from "react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { toast } from "@/components/ui/toast";
import {
  MonitorApiError,
  type ActionDecl,
  type MonitorOperation,
  type MonitorRun,
} from "../api";
import { useCreateOperation } from "../hooks";
import { newIdempotencyKey } from "@/lib/utils";

const KIND_LABEL: Record<string, string> = {
  stop: "停止",
  retry: "重试",
  merge: "合并",
};

export function ConfirmDialog({
  run,
  action,
  onClose,
}: {
  run: MonitorRun;
  action: ActionDecl;
  onClose: () => void;
}) {
  const [force, setForce] = useState(false);
  const [steering, setSteering] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [conflict, setConflict] = useState(false);
  const [result, setResult] = useState<MonitorOperation | null>(null);
  const idemKey = useRef("");
  const create = useCreateOperation();

  useEffect(() => {
    idemKey.current = newIdempotencyKey();
  }, [action.id]);

  const submit = () => {
    setError(null);
    setConflict(false);
    const params: Record<string, unknown> = {};
    if (action.id === "stop" || action.id === "merge") {
      if (force) params.force = true;
    } else if (action.id === "retry") {
      if (steering.trim()) params.steering = steering.trim();
    }
    create.mutate(
      {
        kind: action.id as "stop" | "retry" | "merge",
        project_id: run.projectId,
        run_gid: run.globalRunId,
        idempotency_key: idemKey.current,
        expected_revision: action.revision,
        params,
      },
      {
        onSuccess: (op) => {
          setResult(op);
          if (op.state === "confirmed") {
            toast(`${KIND_LABEL[action.id] ?? action.id} 已确认`, { variant: "success" });
          } else if (op.state === "failed") {
            toast(`${action.id} 失败：${op.error ?? "未知错误"}`, { variant: "error" });
          }
        },
        onError: (e) => {
          if (e instanceof MonitorApiError) {
            if (e.code === "revision_conflict") setConflict(true);
            setError(e.message);
          } else {
            setError("提交失败，请重试");
          }
        },
      },
    );
  };

  return (
    <Dialog open onOpenChange={(o) => !o && onClose()}>
      <DialogContent data-testid="confirm-dialog">
        <DialogHeader>
          <DialogTitle>
            {KIND_LABEL[action.id] ?? action.id}：{run.runId}
          </DialogTitle>
          <DialogDescription>
            动作将以 revision{" "}
            <code className="font-mono" data-testid="confirm-revision">
              {action.revision.slice(0, 12)}…
            </code>{" "}
            提交；若运行状态已变化将被拒绝。
          </DialogDescription>
        </DialogHeader>

        {result ? (
          <div className="space-y-2" data-testid="confirm-result">
            <p className="text-sm">
              状态：<span className="font-mono">{result.state}</span>
            </p>
            {result.receipt && (
              <pre className="max-h-40 overflow-auto rounded bg-muted p-2 text-xs">
                {JSON.stringify(result.receipt, null, 2)}
              </pre>
            )}
            {result.error && (
              <p className="text-sm text-red-600" role="alert">
                {result.error}
              </p>
            )}
            <div className="flex justify-end">
              <button
                type="button"
                onClick={onClose}
                className="rounded border border-border px-3 py-1.5 text-sm"
              >
                关闭
              </button>
            </div>
          </div>
        ) : (
          <div className="space-y-3">
            {(action.id === "stop" || action.id === "merge") && (
              <label className="flex items-center gap-2 text-sm">
                <input
                  type="checkbox"
                  checked={force}
                  onChange={(e) => setForce(e.target.checked)}
                  data-testid="confirm-force"
                />
                force
              </label>
            )}
            {action.id === "retry" && (
              <label className="block space-y-1 text-sm">
                <span>steering（可选）</span>
                <Input
                  value={steering}
                  onChange={(e) => setSteering(e.target.value)}
                  data-testid="confirm-steering"
                />
              </label>
            )}
            {error && (
              <p className="text-sm text-red-600" role="alert" data-testid="confirm-error">
                {conflict
                  ? "运行证据已变化（revision 冲突）——请关闭后重新打开。"
                  : error}
              </p>
            )}

            <div className="flex justify-end gap-2">
              <button
                type="button"
                onClick={onClose}
                className="rounded border border-border px-3 py-1.5 text-sm"
              >
                取消
              </button>
              <button
                type="button"
                onClick={submit}
                disabled={create.isPending}
                data-testid="confirm-submit"
                className="rounded bg-primary px-3 py-1.5 text-sm text-primary-foreground disabled:opacity-50"
              >
                {create.isPending ? "提交中…" : "确认执行"}
              </button>
            </div>
          </div>
        )}
      </DialogContent>
    </Dialog>
  );
}
