import { createDarkTheme } from "@fluentui/react-components";
import { purpleBrand } from "./brand-ramps";
import { sharedOverrides } from "./shared-overrides";
import { themeSeverityPalettes } from "./palettes";
import type { CMTraceTheme } from "./types";

const baseDarkTheme = createDarkTheme(purpleBrand);

const fluentTheme = {
  ...baseDarkTheme,
  ...sharedOverrides,
  // Dracula palette
  colorNeutralBackground1: "#282A36",
  colorNeutralBackground1Hover: "#44475A",
  colorNeutralBackground1Pressed: "#44475A",
  colorNeutralBackground1Selected: "#44475A",
  colorNeutralBackground2: "#21222C",
  colorNeutralBackground3: "#44475A",
  colorNeutralBackground4: "#44475A",
  colorNeutralBackground5: "#44475A",
  colorNeutralBackground6: "#44475A",
  colorNeutralForeground1: "#F8F8F2",
  colorNeutralForeground2: "#F8F8F2",
  colorNeutralForeground3: "#6272A4",
  colorNeutralForeground4: "#6272A4",
  colorNeutralCardBackground: "#21222C",
  colorNeutralCardBackgroundHover: "#282A36",
  colorSubtleBackground: "transparent",
  colorSubtleBackgroundHover: "#44475A",
  colorSubtleBackgroundPressed: "#44475A",
  colorSubtleBackgroundSelected: "#44475A",
};

export const draculaTheme: CMTraceTheme = {
  id: "dracula",
  label: "Dracula",
  colorScheme: "dark",
  fluentTheme,
  severityPalette: themeSeverityPalettes.dracula,
  swatchColor: "#BD93F9",
};
