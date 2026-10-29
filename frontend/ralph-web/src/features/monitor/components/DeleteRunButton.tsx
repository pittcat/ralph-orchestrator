/**
 * DeleteRunButton: dismiss a run from the dashboard. The server
 * deletes the projection rows and marks the (project, run) pair
 * dismissed so stale on-disk evidence (current-loop-id / events
 * files) cannot resurrect it. The project's `.ralph/` data is never
 * touched — this is a view-layer deletion only.
 *
 * `variant="icon"` renders a compact trash icon for table rows (the
 * caller must stopPropagation so row-click navigation is unaffected);
 * `variant="text"` renders a labelled destructive button for the
 * detail header, and `onDeleted` lets that page navigate away.
 */

import { useState } from "react";
import { Trash2 } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { toast } from "@/components/ui/toast";
import type { MonitorRun } from "../api";
import { useDeleteRun } from "../hooks";

export function DeleteRunButton({
  run,
  variant = "text",
  onDeleted,
}: {
  run: MonitorRun;
  variant?: "icon" | "text";
  onDeleted?: () => void;
}) {
  const [open, setOpen] = useState(false);
  const del = useDeleteRun();

  const confirm = () => {
    del.mutate(run.globalRunId, {
      onSuccess: () => {
        setOpen(false);
        toast(`已删除 ${run.runId.slice(0, 24)}`, { variant: "success" });
        onDeleted?.();
      },
      onError: (err) => {
        toast(`删除失败：${err.message}`, { variant: "error" });
      },
    });
  };

  return (
    <>
      {variant === "icon" ? (
        <button
          type="button"
          aria-label={`删除 ${run.runId}`}
          title="从 monitor 中删除"
          data-testid={`delete-run-${run.runId}`}
          onClick={(e) => {
            e.stopPropagation();
            setOpen(true);
          }}
          onKeyDown={(e) => e.stopPropagation()}
          className="rounded p-1 text-muted-foreground hover:bg-destructive/10 hover:text-destructive"
        >
          <Trash2 className="h-3.5 w-3.5" />
        </button>
      ) : (
        <Button
          type="button"
          variant="destructive"
          size="sm"
          data-testid="delete-run"
          onClick={() => setOpen(true)}
        >
          <Trash2 className="h-3.5 w-3.5" />
          删除
        </Button>
      )}

      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>删除这个 run?</DialogTitle>
            <DialogDescription>
              仅从 monitor 视图中移除{" "}
              <span className="font-mono">{run.runId}</span>
              ：删除已采集的事件/活动记录，并标记为已忽略——项目
              <code>.ralph/</code> 里的真实数据不受影响；若磁盘上仍有
              loop 证据残留，collector 也不会再采集它。
            </DialogDescription>
          </DialogHeader>
          <div className="flex justify-end gap-2 pt-2">
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => setOpen(false)}
            >
              取消
            </Button>
            <Button
              type="button"
              variant="destructive"
              size="sm"
              disabled={del.isPending}
              data-testid="confirm-delete-run"
              onClick={confirm}
            >
              {del.isPending ? "删除中…" : "确认删除"}
            </Button>
          </div>
        </DialogContent>
      </Dialog>
    </>
  );
}
