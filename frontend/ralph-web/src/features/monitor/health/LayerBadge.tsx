/**
 * LayerBadge — one health layer rendered as a labeled pill
 * (Plan3-U4). Unknown/pending states stay muted rather than red:
 * "no evidence" is not a failure (plan S2 invariants).
 */

export type LayerTone = "ok" | "warn" | "bad" | "muted";

const TONE_CLASS: Record<LayerTone, string> = {
  ok: "border-emerald-500/40 bg-emerald-500/10 text-emerald-700 dark:text-emerald-300",
  warn: "border-amber-500/40 bg-amber-500/10 text-amber-700 dark:text-amber-300",
  bad: "border-red-500/40 bg-red-500/10 text-red-700 dark:text-red-300",
  muted: "border-border bg-muted/40 text-muted-foreground",
};

export function LayerBadge({
  layer,
  status,
  tone,
  title,
}: {
  layer: string;
  status: string;
  tone: LayerTone;
  title?: string;
}) {
  return (
    <span
      data-testid={`layer-${layer}`}
      title={title ?? `${layer}: ${status}`}
      className={`inline-flex items-center gap-1.5 rounded-full border px-2 py-0.5 text-xs font-medium ${TONE_CLASS[tone]}`}
    >
      <span className="opacity-70">{layer}</span>
      <span>{status}</span>
    </span>
  );
}
