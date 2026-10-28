/**
 * U4 placeholder run detail — the real minimal detail page lands in
 * U5; this only proves the route + id plumbing against the live API.
 */

import { Link, useParams } from "react-router-dom";
import { ArrowLeft } from "lucide-react";
import { useMonitorRunDetail } from "./hooks";

export function RunDetailPage() {
  const { globalId } = useParams<{ globalId: string }>();
  const decoded = globalId ? decodeURIComponent(globalId) : undefined;
  const detail = useMonitorRunDetail(decoded);

  return (
    <div className="space-y-4">
      <Link
        to="/monitor"
        className="inline-flex items-center gap-1 text-sm text-muted-foreground hover:text-foreground"
      >
        <ArrowLeft className="h-4 w-4" /> Back to overview
      </Link>
      <h2 className="font-mono text-lg" data-testid="run-detail-id">
        {decoded?.split(":").slice(2).join(":") ?? "…"}
      </h2>
      {detail.data && (
        <p className="text-sm text-muted-foreground">
          state: {detail.data.run.state}
        </p>
      )}
    </div>
  );
}
