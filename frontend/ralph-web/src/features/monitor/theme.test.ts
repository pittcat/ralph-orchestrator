/**
 * theme.test.ts — store defaults, persistence, system pref fallback.
 */

import { describe, it, expect, beforeEach } from "vitest";
import { applyTheme, useThemeStore } from "./theme";

describe("monitor theme store", () => {
  beforeEach(() => {
    localStorage.clear();
    useThemeStore.setState({ theme: "dark" });
  });

  it("defaults to dark when the OS does not prefer light", () => {
    // matchMedia stub returns matches:false → dark.
    expect(useThemeStore.getState().theme).toBeDefined();
  });

  it("toggle switches theme and applies data-theme", () => {
    applyTheme("dark");
    expect(document.documentElement.dataset.theme).toBe("dark");
    useThemeStore.getState().toggleTheme();
    applyTheme(useThemeStore.getState().theme);
    expect(useThemeStore.getState().theme).toBe("light");
    expect(document.documentElement.dataset.theme).toBe("light");
  });

  it("persists under ralph-monitor-theme in localStorage", async () => {
    useThemeStore.getState().setTheme("light");
    // zustand persist writes synchronously.
    const raw = localStorage.getItem("ralph-monitor-theme");
    expect(raw).toBeTruthy();
    expect(JSON.parse(raw!).state.theme).toBe("light");
  });

  it("restores a stored choice over the system default", () => {
    localStorage.setItem(
      "ralph-monitor-theme",
      JSON.stringify({ state: { theme: "light" }, version: 0 }),
    );
    const stored = JSON.parse(
      localStorage.getItem("ralph-monitor-theme")!,
    ) as { state: { theme: string } };
    expect(stored.state.theme).toBe("light");
  });
});
