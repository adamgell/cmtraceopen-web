// Severity palette contract.
//
// Copied (minimal subset) from the desktop repo's src/lib/constants.ts so
// the theme files under src/lib/themes/ don't need to pull the full
// desktop constants module. The palette structure must stay 1:1 with the
// desktop copy -- palette entries are shared between the two codebases
// today; a future @cmtrace/themes package would be the right home.

export interface LogSeverityPalette {
  error: {
    background: string;
    text: string;
  };
  warning: {
    background: string;
    text: string;
  };
  info: {
    background: string;
    text: string;
  };
  highlightDefault: string;
}
