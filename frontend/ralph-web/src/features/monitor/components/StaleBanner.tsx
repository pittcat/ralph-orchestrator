/**
 * StaleBanner — top banner when a project's last collection errored
 * (project health=error) so rows marked stale get a page-level
 * explanation.
 */

import { AlertTriangle } from "lucide-react";

export function StaleBanner({
  projectNames,
}: {
  projectNames: string[];
}) {
  if (projectNames.length === 0) return null;
  return (
    <div
      role="alert"
      data-testid="stale-banner"
      className="flex items-center gap-2 rounded-md border border-status-stale/50 bg-status-stale/10 px-3 py-2 text-sm text-status-stale"
    >
      <AlertTriangle className="h-4 w-4 shrink-0" />
      <span>
        Collection failed for {projectNames.join(", ")} — shown runs may
        be stale.
      </span>
    </div>
  );
}
