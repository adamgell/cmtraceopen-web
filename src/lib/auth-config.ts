// auth-config
//
// Reads the three Entra-related Vite env vars and produces a normalized
// configuration object the rest of the viewer can consume without
// re-checking individual variables.
//
// The viewer supports two operating modes:
//
//   - "configured" — all three of VITE_ENTRA_TENANT_ID, VITE_ENTRA_CLIENT_ID,
//     and VITE_ENTRA_API_SCOPE are set. MSAL is initialized and operators
//     sign in via popup. Bearer tokens are attached to api-server requests.
//
//   - "anonymous" — at least one of the variables above is missing. MSAL
//     is not initialized; api requests go without an Authorization header.
//     Useful for local dev against api-server with `CMTRACE_AUTH_MODE=disabled`.
//
// The contract for these variables is documented in
// `docs/provisioning/02-entra-app-registration.md` (PR #23).

import {
  PublicClientApplication,
  type Configuration,
} from "@azure/msal-browser";

const TENANT_ID = (import.meta.env.VITE_ENTRA_TENANT_ID as string | undefined) ?? "";
const CLIENT_ID = (import.meta.env.VITE_ENTRA_CLIENT_ID as string | undefined) ?? "";
const API_SCOPE = (import.meta.env.VITE_ENTRA_API_SCOPE as string | undefined) ?? "";

export interface EntraConfigured {
  status: "configured";
  tenantId: string;
  clientId: string;
  apiScope: string;
  msalInstance: PublicClientApplication;
}

export interface EntraAnonymous {
  status: "anonymous";
  /** Names of env vars that were missing — useful for the banner copy. */
  missing: string[];
}

export type EntraConfig = EntraConfigured | EntraAnonymous;

function buildMsalConfig(tenantId: string, clientId: string): Configuration {
  return {
    auth: {
      clientId,
      authority: `https://login.microsoftonline.com/${tenantId}`,
      // Must match the SPA redirect URI registered in Entra exactly,
      // including trailing slash — otherwise MSAL's popup handler treats
      // the redirect as foreign, skips postMessage back to the opener,
      // and the popup never closes.
      redirectUri: `${window.location.origin}/`,
    },
    cache: {
      // localStorage lets the operator avoid re-signing-in across tabs
      // and reloads. Tokens are still scoped per origin by the browser.
      cacheLocation: "localStorage",
    },
  };
}

function resolveConfig(): EntraConfig {
  const missing: string[] = [];
  if (!TENANT_ID) missing.push("VITE_ENTRA_TENANT_ID");
  if (!CLIENT_ID) missing.push("VITE_ENTRA_CLIENT_ID");
  if (!API_SCOPE) missing.push("VITE_ENTRA_API_SCOPE");
  if (missing.length > 0) {
    return { status: "anonymous", missing };
  }
  return {
    status: "configured",
    tenantId: TENANT_ID,
    clientId: CLIENT_ID,
    apiScope: API_SCOPE,
    msalInstance: new PublicClientApplication(
      buildMsalConfig(TENANT_ID, CLIENT_ID),
    ),
  };
}

/**
 * Module-level singleton — MSAL's PublicClientApplication is meant to be
 * instantiated once per app, and downstream code (api-client, settings
 * panel) imports this directly rather than threading it through props.
 */
export const entraConfig: EntraConfig = resolveConfig();

/**
 * Convenience: returns the MSAL instance only when configured, otherwise
 * `null`. Callers in api-client use this to decide whether to attach a
 * bearer token at all.
 */
export function getMsalInstance(): PublicClientApplication | null {
  return entraConfig.status === "configured" ? entraConfig.msalInstance : null;
}

/**
 * Convenience: the API scope string, or null in anonymous mode. Both
 * `acquireTokenSilent` and `loginPopup` need this passed as a single-element
 * array.
 */
export function getApiScope(): string | null {
  return entraConfig.status === "configured" ? entraConfig.apiScope : null;
}

/**
 * The Entra app role value granting admin privileges. Mirrors `ROLE_ADMIN`
 * in `crates/api-server/src/auth/mod.rs`.
 */
export const ROLE_ADMIN = "CmtraceOpen.Admin";

/**
 * Returns `true` when the currently signed-in MSAL account holds the
 * `CmtraceOpen.Admin` app role in its ID token claims.
 *
 * The `roles` claim is populated by Entra when the user has been assigned
 * to the Admin app role on the Enterprise Application. In anonymous mode
 * (no MSAL) this always returns `false` so `RoleGate` renders disabled
 * controls rather than no controls at all.
 */
export function isAdmin(): boolean {
  const instance = getMsalInstance();
  if (!instance) return false;
  const account = instance.getActiveAccount() ?? instance.getAllAccounts()[0] ?? null;
  if (!account) return false;
  const claims = account.idTokenClaims as Record<string, unknown> | undefined;
  if (!claims) return false;
  const roles = claims["roles"];
  if (!Array.isArray(roles)) return false;
  return (roles as unknown[]).includes(ROLE_ADMIN);
}
