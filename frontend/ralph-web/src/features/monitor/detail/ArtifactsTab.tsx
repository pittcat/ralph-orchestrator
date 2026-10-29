/**
 * ArtifactsTab (Plan2-U5 S6/S7): slot index (present/not_generated/
 * outside_allowed) + content preview (Markdown rendered sanitized,
 * other kinds as pre text) + file location (path + mtime/size).
 */

import { useArtifacts, useArtifactContent } from "../hooks";
import { MarkdownViewer } from "../components/MarkdownViewer";
import { Skeleton } from "@/components/ui/skeleton";
import { cn } from "@/lib/utils";
import type { ArtifactEntry } from "../api";

const STATUS_LABEL: Record<ArtifactEntry["status"], string> = {
  present: "已生成",
  not_generated: "尚未生成",
  outside_allowed: "越界拒绝",
};

export function ArtifactsTab({
  globalRunId,
  selectedId,
  onSelect,
}: {
  globalRunId: string;
  selectedId: string | null;
  onSelect: (id: string | null) => void;
}) {
  const artifacts = useArtifacts(globalRunId, true);
  const content = useArtifactContent(globalRunId, selectedId);

  if (artifacts.isLoading) {
    return <Skeleton className="h-40 w-full" />;
  }
  if (artifacts.isError || !artifacts.data) {
    return (
      <p className="text-sm text-status-failed" role="alert">
        产物索引加载失败。
      </p>
    );
  }
  const list = artifacts.data;
  return (
    <div className="grid grid-cols-[16rem_1fr] gap-4" data-testid="artifacts-tab">
      <ul className="space-y-1" aria-label="Artifact slots">
        {list.map((a) => (
          <li key={a.id}>
            <button
              data-testid={`artifact-${a.id}`}
              disabled={a.status !== "present"}
              onClick={() => onSelect(a.id)}
              className={cn(
                "flex w-full items-center justify-between rounded-md px-2 py-1 text-left text-xs",
                selectedId === a.id
                  ? "bg-primary/10 text-primary"
                  : a.status === "present"
                    ? "hover:bg-muted/40"
                    : "text-muted-foreground/60",
              )}
            >
              <span className="truncate font-mono">{a.id}</span>
              <span
                className={cn(
                  "ml-2 shrink-0 text-[10px]",
                  a.status === "present"
                    ? "text-status-ok"
                    : a.status === "outside_allowed"
                      ? "text-status-failed"
                      : "text-muted-foreground",
                )}
                data-testid={`artifact-status-${a.id}`}
              >
                {STATUS_LABEL[a.status]}
              </span>
            </button>
          </li>
        ))}
        {list.length === 0 && (
          <li className="text-xs text-muted-foreground">无已声明产物槽位。</li>
        )}
      </ul>

      <div className="min-h-40">
        {!selectedId ? (
          <p className="text-sm text-muted-foreground">选择一个产物槽位查看内容。</p>
        ) : content.isLoading ? (
          <Skeleton className="h-40 w-full" />
        ) : content.isError ? (
          <p role="alert" className="text-sm text-status-failed">
            {content.error instanceof Error
              ? content.error.message
              : "加载失败"}
          </p>
        ) : content.data ? (
          <div className="space-y-2" data-testid="artifact-preview">
            <div className="flex gap-3 text-xs text-muted-foreground">
              <span className="font-mono">{content.data.id}</span>
              <span>{content.data.kind}</span>
              {list.find((a) => a.id === selectedId)?.path && (
                <span className="font-mono" data-testid="artifact-path">
                  {list.find((a) => a.id === selectedId)!.path}
                </span>
              )}
            </div>
            {content.data.kind === "markdown" ? (
              <MarkdownViewer content={content.data.content} />
            ) : (
              <pre className="max-h-96 overflow-auto rounded-md border border-border bg-muted/30 p-3 font-mono text-xs">
                {content.data.content}
              </pre>
            )}
          </div>
        ) : null}
      </div>
    </div>
  );
}
