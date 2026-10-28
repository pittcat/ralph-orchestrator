/**
 * Pagination — prev/next pager bound to the server's `total` and the
 * fixed page size.
 */

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
        disabled={page <= 1}
        onClick={() => onChange(page - 1)}
        className="rounded-md border border-border px-2 py-1 disabled:opacity-40"
      >
        Prev
      </button>
      <span className="text-muted-foreground">
        {page} / {pages}
      </span>
      <button
        type="button"
        disabled={page >= pages}
        onClick={() => onChange(page + 1)}
        className="rounded-md border border-border px-2 py-1 disabled:opacity-40"
      >
        Next
      </button>
    </nav>
  );
}
