/**
 * Monitor theme store — zustand persist under `ralph-monitor-theme`
 * (mirrors the `ralph-ui` persist pattern). The same key is read
 * synchronously by the inline script in index.html to prevent FOUC.
 *
 * Resolution order: stored choice → OS preference → dark.
 */

import { useEffect } from "react";
import { create } from "zustand";
import { persist } from "zustand/middleware";

export type Theme = "dark" | "light";

interface ThemeState {
  theme: Theme;
  setTheme: (t: Theme) => void;
  toggleTheme: () => void;
}

function systemTheme(): Theme {
  return typeof matchMedia !== "undefined" &&
    matchMedia("(prefers-color-scheme: light)").matches
    ? "light"
    : "dark";
}

export function applyTheme(theme: Theme): void {
  document.documentElement.dataset.theme = theme;
}

export const useThemeStore = create<ThemeState>()(
  persist(
    (set) => ({
      theme: systemTheme(),
      setTheme: (theme) => set({ theme }),
      toggleTheme: () =>
        set((s) => ({ theme: s.theme === "dark" ? "light" : "dark" })),
    }),
    { name: "ralph-monitor-theme" },
  ),
);

/** Keep `<html data-theme>` in sync with the store. Call once at App root. */
export function useThemeEffect(): void {
  const theme = useThemeStore((s) => s.theme);
  useEffect(() => applyTheme(theme), [theme]);
}
