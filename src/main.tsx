import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { MsalProvider } from "@azure/msal-react";
import App from "./App";
import { CommandBridge } from "./components/shell/CommandBridge";
import { entraConfig } from "./lib/auth-config";
import { ThemeProvider } from "./lib/theme-context";
import { WorkspaceProvider } from "./lib/workspace-context";

const rootEl = document.getElementById("root");
if (!rootEl) {
  throw new Error("#root element not found");
}

// `?v=next` opts into the command-bridge shell (Task 3 of 19). Default stays
// on the legacy <ViewerShell /> until Task 18 cutover. Read once at module
// scope — no router dependency, and we don't need per-render re-reads.
const useNextShell =
  new URLSearchParams(window.location.search).get("v") === "next";

// MSAL needs `initialize()` to resolve before any token APIs are called.
// In anonymous mode there's no instance, so we render immediately.
async function bootstrap() {
  if (entraConfig.status === "configured") {
    await entraConfig.msalInstance.initialize();
    // Process any pending auth response (popup or redirect). In popup
    // flow this is what signals the opener and closes the popup window.
    // MsalProvider does this on mount too, but doing it before the first
    // React render avoids a visible app flicker inside the popup.
    try {
      await entraConfig.msalInstance.handleRedirectPromise();
    } catch (err) {
      console.error("handleRedirectPromise failed", err);
    }
  }
  const root = createRoot(rootEl!);
  // ThemeProvider wraps everything so Fluent UI's tokens + classic-cmtrace
  // severity palette are available in both MSAL-configured and anonymous
  // modes.
  if (entraConfig.status === "configured") {
    root.render(
      <StrictMode>
        <ThemeProvider>
          <WorkspaceProvider>
            <MsalProvider instance={entraConfig.msalInstance}>
              {useNextShell ? <CommandBridge /> : <App />}
            </MsalProvider>
          </WorkspaceProvider>
        </ThemeProvider>
      </StrictMode>,
    );
  } else {
    // No MsalProvider in anonymous mode — useMsal would throw without one,
    // so the settings panel branches on entraConfig.status before calling it.
    root.render(
      <StrictMode>
        <ThemeProvider>
          <WorkspaceProvider>
            {useNextShell ? <CommandBridge /> : <App />}
          </WorkspaceProvider>
        </ThemeProvider>
      </StrictMode>,
    );
  }
}

void bootstrap();
