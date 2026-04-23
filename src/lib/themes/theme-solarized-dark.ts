import { createDarkTheme } from "@fluentui/react-components";
import { cyanBrand } from "./brand-ramps";
import { sharedOverrides } from "./shared-overrides";
import { themeSeverityPalettes } from "./palettes";
import type { CMTraceTheme } from "./types";

const baseDarkTheme = createDarkTheme(cyanBrand);

const fluentTheme = {
  ...baseDarkTheme,
  ...sharedOverrides,
  // Solarized Dark base colors
  colorNeutralBackground1: "#002B36",
  colorNeutralBackground1Hover: "#073642",
  colorNeutralBackground1Pressed: "#073642",
  colorNeutralBackground1Selected: "#073642",
  colorNeutralBackground2: "#073642",
  colorNeutralBackground3: "#073642",
  colorNeutralBackground4: "#0A3E4A",
  colorNeutralBackground5: "#0D4A57",
  colorNeutralBackground6: "#0D4A57",
  colorNeutralForeground1: "#839496",
  colorNeutralForeground2: "#93A1A1",
  colorNeutralForeground3: "#657B83",
  colorNeutralForeground4: "#586E75",
  colorNeutralCardBackground: "#073642",
  colorNeutralCardBackgroundHover: "#0A3E4A",
  colorSubtleBackground: "transparent",
  colorSubtleBackgroundHover: "#073642",
  colorSubtleBackgroundPressed: "#0A3E4A",
  colorSubtleBackgroundSelected: "#073642",
};

export const solarizedDarkTheme: CMTraceTheme = {
  id: "solarized-dark",
  label: "Solarized Dark",
  colorScheme: "dark",
  fluentTheme,
  severityPalette: themeSeverityPalettes["solarized-dark"],
  swatchColor: "#2AA198",
};
