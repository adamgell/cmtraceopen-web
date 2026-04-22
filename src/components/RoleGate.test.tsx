// Tests for RoleGate.
//
// Covers the three branching cases the gate has to get right:
//   - anonymous mode → always renders fallback (no MSAL, no roles to check)
//   - configured + Admin role present → renders children
//   - configured + role missing (Operator-only) → renders fallback
//
// `entraConfig` is a module-level singleton in auth-config; we replace the
// module per-test with `vi.doMock` so each case sees the right shape, and
// `useMsal` is stubbed directly to return synthetic accounts.

import { describe, expect, it, vi, beforeEach } from "vitest";
import { render, screen, cleanup } from "@testing-library/react";

// Helpers to (re)mock both modules and re-import RoleGate fresh.
async function loadRoleGateWith(opts: {
  configured: boolean;
  roles?: string[];
}) {
  vi.resetModules();

  if (opts.configured) {
    vi.doMock("../lib/auth-config", () => ({
      entraConfig: { status: "configured" },
      ROLE_ADMIN: "CmtraceOpen.Admin",
    }));
    vi.doMock("@azure/msal-react", () => ({
      useMsal: () => ({
        accounts: [
          {
            idTokenClaims: opts.roles ? { roles: opts.roles } : {},
          },
        ],
      }),
    }));
  } else {
    vi.doMock("../lib/auth-config", () => ({
      entraConfig: { status: "anonymous", missing: [] },
      ROLE_ADMIN: "CmtraceOpen.Admin",
    }));
    vi.doMock("@azure/msal-react", () => ({
      useMsal: () => ({ accounts: [] }),
    }));
  }

  const mod = await import("./RoleGate");
  return mod.RoleGate;
}

describe("RoleGate", () => {
  beforeEach(() => {
    cleanup();
  });

  it("renders fallback in anonymous mode (no MSAL configured)", async () => {
    const RoleGate = await loadRoleGateWith({ configured: false });
    render(
      <RoleGate role="CmtraceOpen.Admin" fallback={<span>nope</span>}>
        <button>secret action</button>
      </RoleGate>,
    );
    expect(screen.queryByRole("button", { name: "secret action" })).toBeNull();
    expect(screen.getByText("nope")).toBeInTheDocument();
  });

  it("renders children when the active account holds the required role", async () => {
    const RoleGate = await loadRoleGateWith({
      configured: true,
      roles: ["CmtraceOpen.Admin", "CmtraceOpen.Operator"],
    });
    render(
      <RoleGate role="CmtraceOpen.Admin" fallback={<span>nope</span>}>
        <button>Disable</button>
      </RoleGate>,
    );
    expect(
      screen.getByRole("button", { name: "Disable" }),
    ).toBeInTheDocument();
    expect(screen.queryByText("nope")).toBeNull();
  });

  it("renders fallback for an Operator-only token (no Admin role)", async () => {
    const RoleGate = await loadRoleGateWith({
      configured: true,
      roles: ["CmtraceOpen.Operator"],
    });
    render(
      <RoleGate role="CmtraceOpen.Admin" fallback={<span>nope</span>}>
        <button>Disable</button>
      </RoleGate>,
    );
    expect(screen.queryByRole("button", { name: "Disable" })).toBeNull();
    expect(screen.getByText("nope")).toBeInTheDocument();
  });
});
