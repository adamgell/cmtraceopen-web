import { createDarkTheme } from "@fluentui/react-components";
import { frostBrand } from "./brand-ramps";
import { sharedOverrides } from "./shared-overrides";
import { themeSeverityPalettes } from "./palettes";
import type { CMTraceTheme } from "./types";

const baseDarkTheme = createDarkTheme(frostBrand);

const fluentTheme = {
  ...baseDarkTheme,
  ...sharedOverrides,
  // Nord Polar Night + Snow Storm palette
  colorNeutralBackground1: "#2E3440",
  colorNeutralBackground1Hover: "#3B4252",
  colorNeutralBackground1Pressed: "#3B4252",
  colorNeutralBackground1Selected: "#3B4252",
  colorNeutralBackground2: "#3B4252",
  colorNeutralBackground3: "#434C5E",
  colorNeutralBackground4: "#4C566A",
  colorNeutralBackground5: "#4C566A",
  colorNeutralBackground6: "#4C566A",
  colorNeutralForeground1: "#D8DEE9",
  colorNeutralForeground2: "#E5E9F0",
  colorNeutralForeground3: "#ECEFF4",
  colorNeutralForeground4: "#D8DEE9",
  colorNeutralCardBackground: "#3B4252",
  colorNeutralCardBackgroundHover: "#434C5E",
  colorSubtleBackground: "transparent",
  colorSubtleBackgroundHover: "#3B4252",
  colorSubtleBackgroundPressed: "#434C5E",
  colorSubtleBackgroundSelected: "#3B4252",
};

export const nordTheme: CMTraceTheme = {
  id: "nord",
  label: "Nord",
  colorScheme: "dark",
  fluentTheme,
  severityPalette: themeSeverityPalettes.nord,
  swatchColor: "#88C0D0",
};
