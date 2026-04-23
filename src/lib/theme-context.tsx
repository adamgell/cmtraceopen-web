// Theme context for the web viewer.
//
// Mirrors the desktop app's theme system -- the theme files under
// src/lib/themes/ are copied as-is from ~/repo/cmtraceopen and re-export
// the same `CMTraceTheme` / `ThemeId` types. This context owns the
// "current theme" state (persisted in localStorage) and wraps the app
// in Fluent UI's `FluentProvider`.
//
// Upgrade path: when the theme files move into a shared @cmtrace/themes
// npm package, the import below is the only thing that needs to change.

import { createContext, useContext, useEffect, useMemo, useState } from "react";
import type { ReactNode } from "react";
import { FluentProvider } from "@fluentui/react-components";
import {
  DEFAULT_THEME_ID,
  getAllThemes,
  getThemeById,
  type CMTraceTheme,
  type ThemeId,
} from "./themes";

const STORAGE_KEY = "cmtraceopen-web.theme";

function readStoredTheme(): ThemeId {
  if (typeof window === "undefined") return DEFAULT_THEME_ID;
  const v = window.localStorage.getItem(STORAGE_KEY);
  if (!v) return DEFAULT_THEME_ID;
  const match = getAllThemes().find((t) => t.id === v);
  return match ? match.id : DEFAULT_THEME_ID;
}

interface ThemeContextValue {
  theme: CMTraceTheme;
  themeId: ThemeId;
  setThemeId: (id: ThemeId) => void;
  allThemes: CMTraceTheme[];
}

const ThemeContext = createContext<ThemeContextValue | null>(null);

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [themeId, setThemeIdState] = useState<ThemeId>(readStoredTheme);
  const theme = useMemo(() => getThemeById(themeId), [themeId]);

  const setThemeId = (id: ThemeId) => {
    setThemeIdState(id);
    if (typeof window !== "undefined") {
      window.localStorage.setItem(STORAGE_KEY, id);
    }
  };

  // Keep body background in sync with the theme so the un-themed region
  // (outside the FluentProvider's DOM subtree) doesn't flash the wrong
  // color on a cold render.
  useEffect(() => {
    if (typeof document === "undefined") return;
    document.body.style.background =
      theme.fluentTheme.colorNeutralBackground1 ?? "";
    document.body.style.color =
      theme.fluentTheme.colorNeutralForeground1 ?? "";
    document.body.style.colorScheme = theme.colorScheme;
  }, [theme]);

  const value: ThemeContextValue = {
    theme,
    themeId,
    setThemeId,
    allThemes: getAllThemes(),
  };

  return (
    <ThemeContext.Provider value={value}>
      <FluentProvider theme={theme.fluentTheme}>{children}</FluentProvider>
    </ThemeContext.Provider>
  );
}

export function useTheme(): ThemeContextValue {
  const ctx = useContext(ThemeContext);
  if (!ctx) {
    throw new Error("useTheme must be used inside <ThemeProvider>");
  }
  return ctx;
}
