import { createLightTheme } from "@fluentui/react-components";
import { steelBrand } from "./brand-ramps";
import { sharedOverrides } from "./shared-overrides";
import { themeSeverityPalettes } from "./palettes";
import type { CMTraceTheme } from "./types";

const baseLightTheme = createLightTheme(steelBrand);

const fluentTheme = {
  ...baseLightTheme,
  ...sharedOverrides,
  // Classic CMTrace uses a slightly warm white background
  colorNeutralBackground1: "#FFFFFF",
  colorNeutralBackground2: "#F8F8F8",
  colorNeutralBackground3: "#F0F0F0",
  colorNeutralForeground1: "#000000",
  colorNeutralForeground2: "#333333",
};

export const classicCmtraceTheme: CMTraceTheme = {
  id: "classic-cmtrace",
  label: "Classic CMTrace",
  colorScheme: "light",
  fluentTheme,
  severityPalette: themeSeverityPalettes["classic-cmtrace"],
  swatchColor: "#4A6785",
};
