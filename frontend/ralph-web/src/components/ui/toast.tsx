/**
 * Toast — self-implemented toaster region + queue (plan U4 §6:
 * no external dependency).
 *
 * Usage: `toast("message", { variant })` from anywhere; mount
 * `<Toaster />` once near the app root.
 */

import { X } from "lucide-react";
import { create } from "zustand";
import { cn } from "@/lib/utils";

export interface ToastItem {
  id: string;
  message: string;
  variant: "default" | "success" | "error";
}

interface ToastStore {
  toasts: ToastItem[];
  push: (t: ToastItem) => void;
  dismiss: (id: string) => void;
}

const useToastStore = create<ToastStore>((set) => ({
  toasts: [],
  push: (t) => set((s) => ({ toasts: [...s.toasts, t] })),
  dismiss: (id) => set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })),
}));

let counter = 0;

/** Queue a toast; auto-dismisses after `durationMs` (default 4s). */
export function toast(
  message: string,
  opts: { variant?: ToastItem["variant"]; durationMs?: number } = {},
): string {
  const id = `toast-${++counter}`;
  useToastStore.getState().push({
    id,
    message,
    variant: opts.variant ?? "default",
  });
  const ms = opts.durationMs ?? 4000;
  if (ms > 0) {
    setTimeout(() => useToastStore.getState().dismiss(id), ms);
  }
  return id;
}

export function dismissToast(id: string): void {
  useToastStore.getState().dismiss(id);
}

const VARIANT_STYLES: Record<ToastItem["variant"], string> = {
  default: "border-border bg-popover text-popover-foreground",
  success: "border-status-success/40 bg-popover text-popover-foreground",
  error: "border-status-failed/40 bg-popover text-popover-foreground",
};

function ToastCard({ item }: { item: ToastItem }) {
  return (
    <div
      role="status"
      className={cn(
        "pointer-events-auto flex w-80 items-start gap-3 rounded-md border p-4 shadow-lg",
        VARIANT_STYLES[item.variant],
      )}
    >
      <p className="flex-1 text-sm">{item.message}</p>
      <button
        type="button"
        aria-label="Dismiss"
        onClick={() => dismissToast(item.id)}
        className="rounded-sm text-muted-foreground transition-colors hover:text-foreground focus:outline-none focus:ring-2 focus:ring-ring"
      >
        <X className="h-4 w-4" />
      </button>
    </div>
  );
}

export function Toaster() {
  const toasts = useToastStore((s) => s.toasts);
  return (
    <div
      aria-live="polite"
      className="pointer-events-none fixed bottom-4 right-4 z-[100] flex flex-col gap-2"
    >
      {toasts.map((t) => (
        <ToastCard key={t.id} item={t} />
      ))}
    </div>
  );
}
