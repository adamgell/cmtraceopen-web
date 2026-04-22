import { useCallback, useEffect, useRef, useState } from "react";
import { useMsal } from "@azure/msal-react";
import { entraConfig } from "../lib/auth-config";

const RUNBOOK_URL =
  "https://github.com/adamgell/cmtraceopen-web/blob/main/docs/provisioning/02-entra-app-registration.md";

const ANON_BANNER_COPY =
  "Anonymous mode — set VITE_ENTRA_* in `.env.local` to enable Entra sign-in.";

/**
 * Header-mounted auth control. Three render branches:
 *
 *   1. Anonymous mode (no env vars) — yellow banner + runbook link.
 *      No MSAL hooks are called in this branch (MsalProvider is absent).
 *   2. Configured + signed out — "Sign in with Entra" button.
 *   3. Configured + signed in   — username + tenant + sign-out, in a
 *      collapsible popover.
 *
 * Styling matches the rest of the viewer: inline CSS, no UI framework.
 */
export function AuthSettings() {
  if (entraConfig.status === "anonymous") {
    return <AnonymousBanner missing={entraConfig.missing} />;
  }
  return <ConfiguredAuth />;
}

export { ANON_BANNER_COPY };

// ---------------------------------------------------------------------------

function AnonymousBanner({ missing }: { missing: string[] }) {
  return (
    <div
      style={{
        padding: "4px 10px",
        background: "#fef3c7",
        border: "1px solid #fcd34d",
        borderRadius: 4,
        fontSize: 12,
        color: "#78350f",
        display: "flex",
        alignItems: "center",
        gap: 8,
      }}
      title={`Missing: ${missing.join(", ")}`}
    >
      <span>{ANON_BANNER_COPY}</span>
      <a
        href={RUNBOOK_URL}
        target="_blank"
        rel="noreferrer noopener"
        style={{ color: "#78350f", textDecoration: "underline" }}
      >
        runbook
      </a>
    </div>
  );
}

function ConfiguredAuth() {
  // Safe: MsalProvider is mounted whenever entraConfig.status === "configured",
  // and AuthSettings only reaches this branch in that case.
  const { instance, accounts } = useMsal();
  const account = accounts[0] ?? null;

  // Apply the active account so acquireTokenSilent can find it without a
  // hint param. MSAL doesn't auto-select on initialize() in v3+.
  useEffect(() => {
    if (account && instance.getActiveAccount() == null) {
      instance.setActiveAccount(account);
    }
  }, [account, instance]);

  const [open, setOpen] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const popoverRef = useRef<HTMLDivElement | null>(null);

  // Close popover on outside click — minimal, no portal.
  useEffect(() => {
    if (!open) return;
    const onDoc = (e: MouseEvent) => {
      if (
        popoverRef.current &&
        !popoverRef.current.contains(e.target as Node)
      ) {
        setOpen(false);
      }
    };
    document.addEventListener("mousedown", onDoc);
    return () => document.removeEventListener("mousedown", onDoc);
  }, [open]);

  const handleSignIn = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      // Redirect flow is more robust than popup across browsers — no
      // window.opener / bridge-timeout failure modes. The response is
      // picked up by handleRedirectPromise() during main.tsx bootstrap.
      await instance.loginRedirect({
        scopes: [entraConfig.status === "configured" ? entraConfig.apiScope : ""],
      });
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
      setBusy(false);
    }
  }, [instance]);

  const handleSignOut = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      // logoutRedirect navigates to Entra and back; matches the sign-in
      // flow above and avoids browser popup restrictions entirely.
      await instance.logoutRedirect({ account: account ?? undefined });
    } catch (err: unknown) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
      setOpen(false);
    }
  }, [instance, account]);

  if (!account) {
    return (
      <button
        type="button"
        onClick={handleSignIn}
        disabled={busy}
        style={btnStyle}
      >
        {busy ? "Signing in…" : "Sign in with Entra"}
      </button>
    );
  }

  const username = account.username || account.name || "(unknown user)";

  return (
    <div ref={popoverRef} style={{ position: "relative" }}>
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        style={btnStyle}
        aria-haspopup="menu"
        aria-expanded={open}
      >
        {username}
      </button>
      {open && (
        <div
          role="menu"
          style={{
            position: "absolute",
            right: 0,
            top: "calc(100% + 4px)",
            minWidth: 280,
            background: "white",
            border: "1px solid #ddd",
            borderRadius: 4,
            boxShadow: "0 4px 12px rgba(0,0,0,0.08)",
            padding: 10,
            fontSize: 12,
            color: "#333",
            zIndex: 10,
          }}
        >
          <div style={{ marginBottom: 6 }}>
            <div style={{ color: "#888" }}>Signed in as</div>
            <div style={{ fontWeight: 500 }}>{username}</div>
          </div>
          <div style={{ marginBottom: 8 }}>
            <div style={{ color: "#888" }}>Tenant</div>
            <div style={{ fontFamily: "ui-monospace, Menlo, Consolas, monospace" }}>
              {entraConfig.status === "configured" ? entraConfig.tenantId : ""}
            </div>
          </div>
          {error && (
            <div
              style={{
                marginBottom: 8,
                padding: 6,
                background: "#fef2f2",
                color: "#991b1b",
                border: "1px solid #fecaca",
                borderRadius: 3,
                whiteSpace: "pre-wrap",
              }}
            >
              {error}
            </div>
          )}
          <button
            type="button"
            onClick={handleSignOut}
            disabled={busy}
            style={{ ...btnStyle, width: "100%" }}
          >
            {busy ? "Signing out…" : "Sign out"}
          </button>
        </div>
      )}
    </div>
  );
}

const btnStyle: React.CSSProperties = {
  padding: "4px 10px",
  fontSize: 12,
  border: "1px solid #ccc",
  background: "white",
  borderRadius: 4,
  cursor: "pointer",
  color: "#222",
};
