import { createDarkTheme } from "@fluentui/react-components";
import { tealBrand } from "./brand-ramps";
import { sharedOverrides } from "./shared-overrides";
import { themeSeverityPalettes } from "./palettes";
import type { CMTraceTheme } from "./types";

const baseDarkTheme = createDarkTheme(tealBrand);

const fluentTheme = {
  ...baseDarkTheme,
  ...sharedOverrides,
};

export const darkTheme: CMTraceTheme = {
  id: "dark",
  label: "Dark",
  colorScheme: "dark",
  fluentTheme,
  severityPalette: themeSeverityPalettes.dark,
  swatchColor: "#009688",
};
