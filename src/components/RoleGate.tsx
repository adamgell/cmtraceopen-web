// RoleGate
//
// Renders `children` only when the currently signed-in user has the required
// Entra app role in their ID token. When the role is absent (or the user is
// not signed in / MSAL is unconfigured) the `fallback` element is rendered
// instead — default is `null` (nothing).
//
// Usage:
//   <RoleGate role="CmtraceOpen.Admin">
//     <button>Dangerous admin action</button>
//   </RoleGate>
//
//   <RoleGate role="CmtraceOpen.Admin" fallback={<span>Requires Admin</span>}>
//     <button>Action</button>
//   </RoleGate>
//
// The gate re-evaluates whenever the MSAL account list changes (i.e. after
// sign-in / sign-out) because it reads from `useMsal()`.

import { useMsal } from "@azure/msal-react";
import { entraConfig } from "../lib/auth-config";

interface RoleGateProps {
  /** The Entra app-role value required to see `children`. */
  role: string;
  children: React.ReactNode;
  /** Rendered when the role requirement is not met. Defaults to `null`. */
  fallback?: React.ReactNode;
}

/**
 * Renders `children` when the active MSAL account holds the required app role,
 * otherwise renders `fallback` (default: nothing).
 *
 * In anonymous mode (no MSAL) the `fallback` is always rendered so the UI
 * degrades gracefully without a hard dependency on Entra being configured.
 */
export function RoleGate({ role, children, fallback = null }: RoleGateProps) {
  if (entraConfig.status === "anonymous") {
    return <>{fallback}</>;
  }
  return <ConfiguredRoleGate role={role} fallback={fallback}>{children}</ConfiguredRoleGate>;
}

function ConfiguredRoleGate({
  role,
  children,
  fallback,
}: RoleGateProps) {
  // Safe: MsalProvider is mounted when entraConfig.status === "configured".
  const { accounts } = useMsal();
  const account = accounts[0] ?? null;
  const claims = account?.idTokenClaims as Record<string, unknown> | undefined;
  const roles = claims?.["roles"];
  const hasRole = Array.isArray(roles) && (roles as unknown[]).includes(role);
  return <>{hasRole ? children : fallback}</>;
}
