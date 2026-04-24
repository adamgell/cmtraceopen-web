// Tests for the SessionTree — device-mode body of the middle pane.
//
// Two cases:
//   1. Sessions for the selected device render with a formatted timestamp.
//   2. Clicking a session row loads its files and renders them with a
//      human-readable entry count.
//
// Mocks api-client the same way as DeviceRail.test.tsx: vi.doMock + resetModules
// so BridgeStateProvider is re-imported after the mock registers. Otherwise the
// provider and the SessionTree's useBridgeState() hit different module
// instances and the context lookup fails.

import { describe, it, expect, vi, beforeEach } from "vitest";
import { render, screen, waitFor, fireEvent } from "@testing-library/react";

beforeEach(() => vi.resetModules());

async function loadTree(opts: {
  sessions: Array<{ sessionId: string; ingestedUtc: string; parseState: string }>;
  filesBySession: Record<string, Array<{ fileId: string; relativePath: string; entryCount: number }>>;
}) {
  vi.doMock("../../lib/api-client", () => ({
    listSessions: async () => ({
      items: opts.sessions.map((s) => ({
        sessionId: s.sessionId,
        deviceId: "D",
        bundleId: "B",
        collectedUtc: null,
        ingestedUtc: s.ingestedUtc,
        sizeBytes: 0,
        parseState: s.parseState,
      })),
      nextCursor: null,
    }),
    listFiles: async (_sessionId: string) => ({
      items: opts.filesBySession[_sessionId] ?? [],
      nextCursor: null,
    }),
  }));
  const { SessionTree } = await import("./SessionTree");
  const { BridgeStateProvider } = await import("../../lib/bridge-state");
  return { SessionTree, BridgeStateProvider };
}

describe("SessionTree", () => {
  it("lists sessions for the selected device", async () => {
    const { SessionTree, BridgeStateProvider } = await loadTree({
      sessions: [
        { sessionId: "s1", ingestedUtc: "2026-04-24T00:28:00Z", parseState: "ok-with-fallbacks" },
        { sessionId: "s2", ingestedUtc: "2026-04-24T00:13:00Z", parseState: "partial" },
      ],
      filesBySession: {},
    });
    render(
      <BridgeStateProvider>
        <SessionTree deviceId="GELL-01AA310" />
      </BridgeStateProvider>
    );
    await waitFor(() => expect(screen.getByText(/00:28/)).toBeInTheDocument());
    expect(screen.getByText(/00:13/)).toBeInTheDocument();
  });

  it("loads files when a session row is clicked", async () => {
    const { SessionTree, BridgeStateProvider } = await loadTree({
      sessions: [{ sessionId: "s1", ingestedUtc: "2026-04-24T00:28:00Z", parseState: "ok" }],
      filesBySession: { s1: [{ fileId: "f1", relativePath: "logs/ccmexec.log", entryCount: 1300000 }] },
    });
    render(
      <BridgeStateProvider>
        <SessionTree deviceId="GELL-01AA310" />
      </BridgeStateProvider>
    );
    await waitFor(() => expect(screen.getByText(/00:28/)).toBeInTheDocument());
    fireEvent.click(screen.getByText(/00:28/));
    await waitFor(() => expect(screen.getByText(/ccmexec\.log/)).toBeInTheDocument());
    expect(screen.getByText(/1\.3M/)).toBeInTheDocument();
  });
});
