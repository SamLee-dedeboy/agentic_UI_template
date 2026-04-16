import { useEffect, useState } from "react";

/**
 * Minimal two-mode theme hook (dark / light).
 *
 * How it works:
 *  - Default is dark (matches the `@theme` block in styles.css).
 *  - Light mode is switched on by adding `theme-light` to the <html> element.
 *    The CSS in styles.css overrides the color tokens under that class.
 *  - The choice is persisted in localStorage under `ui-theme`, so reloading
 *    keeps the user's preference.
 *  - If the user hasn't made a choice, we honour `prefers-color-scheme`.
 *
 * Forks that want more palettes can add `.theme-<name>` blocks to
 * styles.css alongside the existing `.theme-light` and extend the Theme
 * type + this hook. The toggle UI is in `ThemeToggle.tsx`.
 */
export type Theme = "dark" | "light";

const STORAGE_KEY = "ui-theme";

function readInitialTheme(): Theme {
  if (typeof window === "undefined") return "dark";
  const stored = window.localStorage.getItem(STORAGE_KEY);
  if (stored === "dark" || stored === "light") return stored;
  const prefersLight =
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-color-scheme: light)").matches;
  return prefersLight ? "light" : "dark";
}

function applyTheme(theme: Theme) {
  const root = document.documentElement;
  root.classList.toggle("theme-light", theme === "light");
  // `color-scheme` tells the browser to render native form controls,
  // scrollbars, etc. in the matching palette.
  root.style.colorScheme = theme;
}

export function useTheme() {
  const [theme, setTheme] = useState<Theme>(() => readInitialTheme());

  useEffect(() => {
    applyTheme(theme);
    try {
      window.localStorage.setItem(STORAGE_KEY, theme);
    } catch {
      // Ignore quota / privacy-mode errors — in-memory state still works.
    }
  }, [theme]);

  return {
    theme,
    setTheme,
    toggle: () => setTheme((t) => (t === "dark" ? "light" : "dark")),
  };
}
