// UserChip tests.
//
// UserChip branches on `entraConfig.status` at module load and consumes
// `useMsal` in the configured branch. Both are module-scope imports so we
// replace them per-test via `vi.doMock` + `vi.resetModules()`.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, cleanup, fireEvent } from "@testing-library/react";

beforeEach(() => {
  vi.resetModules();
  cleanup();
});

async function loadChip(opts: {
  status: "anonymous" | "configured";
  accounts?: Array<{ name?: string; username: string }>;
  loginPopup?: () => Promise<void>;
}) {
  if (opts.status === "anonymous") {
    vi.doMock("../../lib/auth-config", () => ({
      entraConfig: { status: "anonymous", missing: ["VITE_ENTRA_TENANT_ID"] },
      getApiScope: () => null,
      ROLE_ADMIN: "CmtraceOpen.Admin",
    }));
    vi.doMock("@azure/msal-react", () => ({
      useMsal: () => ({ accounts: [], instance: {}, inProgress: "None" }),
    }));
  } else {
    vi.doMock("../../lib/auth-config", () => ({
      entraConfig: { status: "configured" },
      getApiScope: () => "api://test/access",
      ROLE_ADMIN: "CmtraceOpen.Admin",
    }));
    vi.doMock("@azure/msal-react", () => ({
      useMsal: () => ({
        accounts: opts.accounts ?? [],
        instance: {
          loginPopup: opts.loginPopup ?? (async () => undefined),
          logoutPopup: async () => undefined,
        },
        inProgress: "None",
      }),
    }));
  }
  const { UserChip } = await import("./UserChip");
  return UserChip;
}

describe("UserChip", () => {
  it("renders an anonymous badge when Entra isn't configured", async () => {
    const UserChip = await loadChip({ status: "anonymous" });
    render(<UserChip />);
    expect(screen.getByText(/anonymous/i)).toBeInTheDocument();
  });

  it("renders a Sign in button when configured with no account", async () => {
    const UserChip = await loadChip({ status: "configured", accounts: [] });
    render(<UserChip />);
    expect(screen.getByRole("button", { name: /sign in/i })).toBeInTheDocument();
  });

  it("calls loginPopup when Sign in is clicked", async () => {
    const loginPopup = vi.fn(async () => undefined);
    const UserChip = await loadChip({ status: "configured", accounts: [], loginPopup });
    render(<UserChip />);
    fireEvent.click(screen.getByRole("button", { name: /sign in/i }));
    expect(loginPopup).toHaveBeenCalled();
  });

  it("renders the signed-in account name when accounts are present", async () => {
    const UserChip = await loadChip({
      status: "configured",
      accounts: [{ name: "Daisy Gell", username: "daisy@example.com" }],
    });
    render(<UserChip />);
    expect(screen.getByText("Daisy Gell")).toBeInTheDocument();
  });

  it("opens the account menu on click and exposes a Sign out menuitem", async () => {
    const UserChip = await loadChip({
      status: "configured",
      accounts: [{ name: "Daisy Gell", username: "daisy@example.com" }],
    });
    render(<UserChip />);
    fireEvent.click(screen.getByRole("button", { name: /daisy gell/i }));
    expect(screen.getByRole("menuitem", { name: /sign out/i })).toBeInTheDocument();
  });
});
