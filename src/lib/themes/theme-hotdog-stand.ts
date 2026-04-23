import { createLightTheme } from "@fluentui/react-components";
import { redBrand } from "./brand-ramps";
import { sharedOverrides } from "./shared-overrides";
import { themeSeverityPalettes } from "./palettes";
import type { CMTraceTheme } from "./types";

const baseLightTheme = createLightTheme(redBrand);

const fluentTheme = {
  ...baseLightTheme,
  ...sharedOverrides,
  // Intentionally garish -- the Windows 3.1 Hot Dog Stand tribute
  colorNeutralBackground1: "#FF0000",
  colorNeutralBackground1Hover: "#CC0000",
  colorNeutralBackground1Pressed: "#990000",
  colorNeutralBackground1Selected: "#CC0000",
  colorNeutralBackground2: "#FF0000",
  colorNeutralBackground2Hover: "#CC0000",
  colorNeutralBackground2Pressed: "#990000",
  colorNeutralBackground2Selected: "#CC0000",
  colorNeutralBackground3: "#CC0000",
  colorNeutralBackground4: "#DD0000",
  colorNeutralBackground5: "#BB0000",
  colorNeutralBackground6: "#AA0000",
  colorNeutralForeground1: "#FFFF00",
  colorNeutralForeground1Hover: "#FFFF00",
  colorNeutralForeground1Pressed: "#FFFF00",
  colorNeutralForeground1Selected: "#FFFF00",
  colorNeutralForeground2: "#FFFF00",
  colorNeutralForeground3: "#FFEE00",
  colorNeutralForeground4: "#FFDD00",
  colorNeutralForegroundOnBrand: "#FFFF00",
  colorNeutralCardBackground: "#FF0000",
  colorNeutralCardBackgroundHover: "#CC0000",
  colorSubtleBackground: "transparent",
  colorSubtleBackgroundHover: "#CC0000",
  colorSubtleBackgroundPressed: "#990000",
  colorSubtleBackgroundSelected: "#CC0000",
  colorBrandBackground: "#FFFF00",
  colorBrandBackgroundHover: "#EEEE00",
  colorBrandBackgroundPressed: "#CCCC00",
  colorBrandBackgroundSelected: "#DDDD00",
  colorBrandForeground1: "#FFFF00",
  colorBrandForeground2: "#EEEE00",
  colorCompoundBrandForeground1: "#FFFF00",
  colorCompoundBrandForeground1Hover: "#EEEE00",
  colorCompoundBrandForeground1Pressed: "#CCCC00",
  colorCompoundBrandBackground: "#FFFF00",
  colorCompoundBrandBackgroundHover: "#EEEE00",
  colorCompoundBrandBackgroundPressed: "#CCCC00",
  colorCompoundBrandStroke: "#FFFF00",
  colorCompoundBrandStrokeHover: "#EEEE00",
  colorCompoundBrandStrokePressed: "#CCCC00",
  colorBrandStroke1: "#FFFF00",
  colorNeutralStroke1: "#FFFF00",
  colorNeutralStrokeAccessible: "#FFFF00",
  colorStrokeFocus1: "#FF0000",
  colorStrokeFocus2: "#FFFF00",
};

export const hotdogStandTheme: CMTraceTheme = {
  id: "hotdog-stand",
  label: "Hot Dog Stand",
  colorScheme: "light",
  fluentTheme,
  severityPalette: themeSeverityPalettes["hotdog-stand"],
  swatchColor: "#FF0000",
};
