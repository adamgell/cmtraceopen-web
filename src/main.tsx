import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { MsalProvider } from "@azure/msal-react";
import App from "./App";
import { entraConfig } from "./lib/auth-config";

const rootEl = document.getElementById("root");
if (!rootEl) {
  throw new Error("#root element not found");
}

// MSAL needs `initialize()` to resolve before any token APIs are called.
// In anonymous mode there's no instance, so we render immediately.
async function bootstrap() {
  if (entraConfig.status === "configured") {
    await entraConfig.msalInstance.initialize();
  }
  const root = createRoot(rootEl!);
  if (entraConfig.status === "configured") {
    root.render(
      <StrictMode>
        <MsalProvider instance={entraConfig.msalInstance}>
          <App />
        </MsalProvider>
      </StrictMode>,
    );
  } else {
    // No MsalProvider in anonymous mode — useMsal would throw without one,
    // so the settings panel branches on entraConfig.status before calling it.
    root.render(
      <StrictMode>
        <App />
      </StrictMode>,
    );
  }
}

void bootstrap();
