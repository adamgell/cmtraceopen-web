import { teamsHighContrastTheme } from "@fluentui/react-components";
import { sharedOverrides } from "./shared-overrides";
import { themeSeverityPalettes } from "./palettes";
import type { CMTraceTheme } from "./types";

const fluentTheme = {
  ...teamsHighContrastTheme,
  ...sharedOverrides,
};

export const highContrastTheme: CMTraceTheme = {
  id: "high-contrast",
  label: "High Contrast",
  colorScheme: "dark",
  fluentTheme,
  severityPalette: themeSeverityPalettes["high-contrast"],
  swatchColor: "#FFFFFF",
};
