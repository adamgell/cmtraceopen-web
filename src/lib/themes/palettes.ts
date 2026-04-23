import type { LogSeverityPalette } from "../constants";
import type { ThemeId } from "./types";

export const themeSeverityPalettes: Record<ThemeId, LogSeverityPalette> = {
  light: {
    error: { background: "#FEE2E2", text: "#7F1D1D" },
    warning: { background: "#FEF3C7", text: "#78350F" },
    info: { background: "#FFFFFF", text: "#111827" },
    highlightDefault: "#FDE68A",
  },
  dark: {
    error: { background: "#7F1D1D", text: "#FCA5A5" },
    warning: { background: "#78350F", text: "#FDE68A" },
    info: { background: "#1E1E1E", text: "#D4D4D4" },
    highlightDefault: "#854D0E",
  },
  "high-contrast": {
    error: { background: "#000000", text: "#FF0000" },
    warning: { background: "#000000", text: "#FFFF00" },
    info: { background: "#000000", text: "#FFFFFF" },
    highlightDefault: "#00FF00",
  },
  "classic-cmtrace": {
    error: { background: "#FF0000", text: "#FFFF00" },
    warning: { background: "#FFFF00", text: "#000000" },
    info: { background: "#FFFFFF", text: "#000000" },
    highlightDefault: "#FFFF00",
  },
  "solarized-dark": {
    error: { background: "#073642", text: "#DC322F" },
    warning: { background: "#073642", text: "#B58900" },
    info: { background: "#002B36", text: "#839496" },
    highlightDefault: "#586E75",
  },
  nord: {
    error: { background: "#3B4252", text: "#BF616A" },
    warning: { background: "#3B4252", text: "#EBCB8B" },
    info: { background: "#2E3440", text: "#D8DEE9" },
    highlightDefault: "#4C566A",
  },
  dracula: {
    error: { background: "#44475A", text: "#FF5555" },
    warning: { background: "#44475A", text: "#F1FA8C" },
    info: { background: "#282A36", text: "#F8F8F2" },
    highlightDefault: "#6272A4",
  },
  "hotdog-stand": {
    error: { background: "#FF0000", text: "#FFFF00" },
    warning: { background: "#FFFF00", text: "#FF0000" },
    info: { background: "#FF0000", text: "#FFFF00" },
    highlightDefault: "#00FF00",
  },
};
