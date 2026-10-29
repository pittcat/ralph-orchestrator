/**
 * Pending interactions section (Plan4-U3, S4): planning sessions
 * awaiting input — they auto-resolve off this list when the session
 * state flips (stream invalidates the query).
 */

import { useNavigate } from "react-router-dom";
import type { PendingInteraction } from "../api";

export function PendingSection({ items }: { items: PendingInteraction[] }) {
  const navigate = useNavigate();
  if (items.length === 0) return null;
  return (
    <section data-testid="pending-section" className="space-y-2">
      <h3 className="text-sm font-semibold">待处理交互</h3>
      <ul className="space-y-2">
        {items.map((p) => (
          <li
            key={p.id}
            data-testid={`pending-${p.session_id}`}
            tabIndex={0}
            onKeyDown={(e) => {
              if (e.key === "Enter" && p.link) navigate(p.link);
            }}
            className="rounded-md border border-amber-500/40 bg-amber-500/5 px-3 py-2"
          >
            <div className="flex items-center gap-2">
              <span className="text-sm font-medium">{p.session_id}</span>
              <span className="text-xs text-muted-foreground">
                {p.created_at}
              </span>
            </div>
            {p.prompt && (
              <p className="mt-1 truncate text-xs text-muted-foreground">
                {p.prompt}
              </p>
            )}
          </li>
        ))}
      </ul>
    </section>
  );
}
