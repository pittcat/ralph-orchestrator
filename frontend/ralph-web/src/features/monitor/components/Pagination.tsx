/**
 * Pagination — prev/next pager bound to the server's `total` and the
 * fixed page size.
 */

import { ChevronLeft, ChevronRight } from "lucide-react";

const BTN_CLS =
  "inline-flex h-8 w-8 items-center justify-center rounded-lg border border-border bg-card/50 " +
  "text-muted-foreground transition-colors hover:text-foreground hover:border-foreground/25 " +
  "disabled:opacity-40 disabled:pointer-events-none";

export function Pagination({
  page,
  total,
  pageSize,
  onChange,
}: {
  page: number;
  total: number;
  pageSize: number;
  onChange: (page: number) => void;
}) {
  const pages = Math.max(1, Math.ceil(total / pageSize));
  if (pages <= 1) return null;
  return (
    <nav
      aria-label="Runs pagination"
      className="flex items-center gap-2 text-sm"
      data-testid="pagination"
    >
      <button
        type="button"
        aria-label="Previous page"
        disabled={page <= 1}
        onClick={() => onChange(page - 1)}
        className={BTN_CLS}
      >
        <ChevronLeft className="h-4 w-4" />
      </button>
      <span className="text-xs text-muted-foreground tabular-nums">
        {page} / {pages}
      </span>
      <button
        type="button"
        aria-label="Next page"
        disabled={page >= pages}
        onClick={() => onChange(page + 1)}
        className={BTN_CLS}
      >
        <ChevronRight className="h-4 w-4" />
      </button>
    </nav>
  );
}
