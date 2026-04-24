// Sign-in chip for the top header.
//
// Three states:
//   - anonymous    — viewer is running without VITE_ENTRA_* env vars;
//                    renders a dimmed "anonymous" label and nothing else.
//   - signed-out   — Entra is configured but no MSAL account is cached.
//                    Renders a "Sign in" button that triggers a popup.
//   - signed-in    — MSAL has at least one account. Renders the account's
//                    display name + a small menu with "Sign out".
//
// `useMsal` throws when called outside <MsalProvider>, and main.tsx only
// mounts the provider in configured mode — so the MSAL-using body is split
// into <SignedInOrOut /> and only rendered when `entraConfig.status ===
// "configured"`.

import { useState, useRef, useEffect } from "react";
import { useMsal } from "@azure/msal-react";
import { InteractionRequiredAuthError, type AccountInfo } from "@azure/msal-browser";
import { entraConfig, getApiScope } from "../../lib/auth-config";
import { theme } from "../../lib/theme";

export function UserChip() {
  if (entraConfig.status === "anonymous") {
    return (
      <span
        title={`Anonymous mode — set ${entraConfig.missing.join(", ")} to enable sign-in.`}
        style={{
          padding: "0.15rem 0.5rem",
          fontFamily: theme.font.mono,
          fontSize: "0.6rem",
          color: theme.textFainter,
          border: `1px solid ${theme.border}`,
          borderRadius: 3,
          letterSpacing: "0.08em",
          textTransform: "uppercase",
        }}
      >
        anonymous
      </span>
    );
  }
  return <SignedInOrOut />;
}

function SignedInOrOut() {
  const { instance, accounts } = useMsal();
  const account: AccountInfo | undefined = accounts[0];

  if (!account) {
    return <SignInButton onClick={() => startSignIn(instance)} />;
  }
  return <SignedInMenu account={account} onSignOut={() => startSignOut(instance, account)} />;
}

function SignInButton({ onClick }: { onClick: () => void }) {
  return (
    <button
      type="button"
      onClick={onClick}
      style={{
        all: "unset",
        cursor: "pointer",
        padding: "0.25rem 0.75rem",
        border: `1px solid ${theme.accent}`,
        borderRadius: 3,
        background: theme.accentBg,
        color: theme.accent,
        fontFamily: theme.font.mono,
        fontSize: "0.65rem",
        letterSpacing: "0.06em",
      }}
    >
      Sign in
    </button>
  );
}

function SignedInMenu({
  account,
  onSignOut,
}: {
  account: AccountInfo;
  onSignOut: () => void;
}) {
  const [open, setOpen] = useState(false);
  const wrapperRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    if (!open) return;
    function onDocClick(e: MouseEvent) {
      if (!wrapperRef.current) return;
      if (!wrapperRef.current.contains(e.target as Node)) setOpen(false);
    }
    function onEsc(e: KeyboardEvent) {
      if (e.key === "Escape") setOpen(false);
    }
    document.addEventListener("mousedown", onDocClick);
    document.addEventListener("keydown", onEsc);
    return () => {
      document.removeEventListener("mousedown", onDocClick);
      document.removeEventListener("keydown", onEsc);
    };
  }, [open]);

  const display = account.name ?? account.username;
  const initial = display.trim().slice(0, 1).toUpperCase() || "?";

  return (
    <div ref={wrapperRef} style={{ position: "relative" }}>
      <button
        type="button"
        onClick={() => setOpen((v) => !v)}
        title={account.username}
        style={{
          all: "unset",
          cursor: "pointer",
          display: "inline-flex",
          alignItems: "center",
          gap: "0.4rem",
          padding: "0.2rem 0.55rem 0.2rem 0.3rem",
          border: `1px solid ${theme.border}`,
          borderRadius: 3,
          background: theme.surface,
          fontFamily: theme.font.mono,
          fontSize: "0.65rem",
          color: theme.text,
        }}
      >
        <span
          aria-hidden="true"
          style={{
            width: 18,
            height: 18,
            borderRadius: "50%",
            background: theme.accent,
            color: theme.bgDeep,
            display: "inline-flex",
            alignItems: "center",
            justifyContent: "center",
            fontWeight: 700,
            fontSize: "0.62rem",
          }}
        >
          {initial}
        </span>
        <span style={{ maxWidth: "140px", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
          {display}
        </span>
        <span style={{ color: theme.textFainter, fontSize: "0.55rem" }}>▾</span>
      </button>
      {open && (
        <div
          role="menu"
          style={{
            position: "absolute",
            top: "calc(100% + 4px)",
            right: 0,
            minWidth: "220px",
            background: theme.bg,
            border: `1px solid ${theme.border}`,
            borderRadius: 4,
            padding: "0.35rem 0",
            fontFamily: theme.font.mono,
            fontSize: "0.68rem",
            zIndex: 120,
            boxShadow: "0 6px 18px rgba(0,0,0,0.4)",
          }}
        >
          <div style={{ padding: "0.25rem 0.75rem 0.4rem", borderBottom: `1px solid ${theme.border}` }}>
            <div style={{ color: theme.text }}>{display}</div>
            <div style={{ color: theme.textDim, fontSize: "0.6rem", marginTop: "0.1rem" }}>{account.username}</div>
          </div>
          <button
            type="button"
            role="menuitem"
            onClick={() => {
              setOpen(false);
              onSignOut();
            }}
            style={{
              all: "unset",
              display: "block",
              width: "100%",
              padding: "0.35rem 0.75rem",
              color: theme.text,
              cursor: "pointer",
            }}
          >
            Sign out
          </button>
        </div>
      )}
    </div>
  );
}

async function startSignIn(instance: ReturnType<typeof useMsal>["instance"]) {
  const scope = getApiScope();
  try {
    await instance.loginPopup({
      scopes: scope ? [scope] : [],
      prompt: "select_account",
    });
  } catch (err) {
    // Popup blocked, user cancelled, or network glitch — swallow quietly.
    // The button stays visible so the operator can retry; surfacing the
    // raw MSAL error text in a banner is out of scope for this chip.
    if (err instanceof InteractionRequiredAuthError) return;
    console.warn("sign-in failed", err);
  }
}

async function startSignOut(
  instance: ReturnType<typeof useMsal>["instance"],
  account: AccountInfo,
) {
  try {
    await instance.logoutPopup({ account });
  } catch (err) {
    console.warn("sign-out failed", err);
  }
}
