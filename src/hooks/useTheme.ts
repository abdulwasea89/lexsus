import { useSyncExternalStore } from "react";

export type Theme = "dark" | "light";

export const THEME_KEY = "lexsus.theme";

/** Persisted preference — white/light unless explicitly set to dark. */
export function loadTheme(): Theme {
  try {
    return localStorage.getItem(THEME_KEY) === "dark" ? "dark" : "light";
  } catch {
    return "light";
  }
}

/** Apply to <html>: `.dark` drives both our tokens and HeroUI's. */
export function applyTheme(theme: Theme) {
  const root = document.documentElement;
  root.classList.toggle("dark", theme === "dark");
  root.setAttribute("data-theme", theme);
}

let current: Theme = loadTheme();
const listeners = new Set<() => void>();

export function toggleTheme() {
  current = current === "dark" ? "light" : "dark";
  try {
    localStorage.setItem(THEME_KEY, current);
  } catch {
    // storage unavailable (private mode) — session-only toggle
  }
  applyTheme(current);
  listeners.forEach((l) => l());
}

/** Current theme, reactive to `toggleTheme` calls from anywhere. */
export function useTheme(): Theme {
  return useSyncExternalStore(
    (cb) => {
      listeners.add(cb);
      return () => listeners.delete(cb);
    },
    () => current,
    () => current,
  );
}
