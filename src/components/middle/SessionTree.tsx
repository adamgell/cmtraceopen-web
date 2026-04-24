// Device-mode body of the middle pane. Renders the sessions for the selected
// device as a flat list; each row expands in place to show its files when
// clicked. Clicking a file dispatches `select-file` so the right pane can
// load entries.
//
// Data flow:
//   1. useEffect([deviceId])          → listSessions(deviceId)
//   2. toggleSession(sessionId)       → listFiles(sessionId) on first expand
//                                       (cached in `filesBySession` thereafter)
//
// On deviceId change we reset local state (sessions, expanded set, cached
// files, filter) so a stale tree never leaks between devices. The bridge
// reducer already clears selectedSessionId / selectedFileId — this effect
// mirrors that on the local view state.
//
// Filter input scopes across all expanded sessions' files by substring.

import { useEffect, useState } from "react";
import { listFiles, listSessions } from "../../lib/api-client";
import type { SessionFile, SessionSummary } from "../../lib/log-types";
import { useBridgeState } from "../../lib/bridge-state";
import { theme, type PillState } from "../../lib/theme";

function formatCount(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return String(n);
}

function pillFor(state: string): PillState {
  switch (state) {
    case "ok": return "ok";
    case "ok-with-fallbacks": return "okFallbacks";
    case "partial": return "partial";
    case "failed": return "failed";
    default: return "pending";
  }
}

interface Props {
  deviceId: string;
}

export function SessionTree({ deviceId }: Props) {
  const { state, dispatch } = useBridgeState();
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [expanded, setExpanded] = useState<Set<string>>(new Set());
  const [filesBySession, setFilesBySession] = useState<Record<string, SessionFile[]>>({});
  const [filter, setFilter] = useState("");

  useEffect(() => {
    let cancelled = false;
    setLoading(true);
    setError(null);
    setSessions([]);
    setExpanded(new Set());
    setFilesBySession({});
    setFilter("");
    (async () => {
      try {
        const page = await listSessions(deviceId);
        if (!cancelled) setSessions(page.items);
      } catch (err) {
        if (!cancelled) setError(err instanceof Error ? err.message : String(err));
      } finally {
        if (!cancelled) setLoading(false);
      }
    })();
    return () => { cancelled = true; };
  }, [deviceId]);

  async function toggleSession(sessionId: string) {
    const next = new Set(expanded);
    if (next.has(sessionId)) {
      next.delete(sessionId);
      setExpanded(next);
      return;
    }
    next.add(sessionId);
    setExpanded(next);
    if (!filesBySession[sessionId]) {
      try {
        const page = await listFiles(sessionId);
        setFilesBySession((prev) => ({ ...prev, [sessionId]: page.items }));
      } catch {
        setFilesBySession((prev) => ({ ...prev, [sessionId]: [] }));
      }
    }
  }

  if (error) {
    return (
      <div style={{ padding: "0.7rem", color: theme.pill.failed.fg, fontFamily: theme.font.mono, fontSize: "0.7rem" }}>
        sessions unreachable: {error}
      </div>
    );
  }

  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100%" }}>
      <div style={{ padding: "0.3rem 0.5rem", borderBottom: `1px solid ${theme.border}` }}>
        <input
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          placeholder="filter files…"
          style={{
            width: "100%",
            background: theme.surface,
            border: `1px solid ${theme.border}`,
            color: theme.text,
            padding: "0.25rem 0.4rem",
            fontFamily: theme.font.mono,
            fontSize: "0.68rem",
            borderRadius: 3,
          }}
        />
      </div>
      <div style={{ flex: 1, overflow: "auto" }}>
        {loading && (
          <div style={{ padding: "0.7rem", color: theme.textDim, fontFamily: theme.font.mono, fontSize: "0.65rem" }}>
            loading sessions…
          </div>
        )}
        {sessions.map((s) => {
          const isOpen = expanded.has(s.sessionId);
          const chev = isOpen ? "▾" : "▸";
          const ts = new Date(s.ingestedUtc).toISOString().slice(11, 16) + "Z";
          const pill = pillFor(s.parseState);
          const files = filesBySession[s.sessionId] ?? [];
          const visibleFiles = filter.trim()
            ? files.filter((f) => f.relativePath.toLowerCase().includes(filter.toLowerCase()))
            : files;
          return (
            <div key={s.sessionId}>
              <button
                type="button"
                onClick={() => toggleSession(s.sessionId)}
                style={{
                  all: "unset",
                  display: "flex",
                  width: "100%",
                  padding: "0.25rem 0.6rem",
                  gap: "0.4rem",
                  alignItems: "center",
                  fontFamily: theme.font.mono,
                  fontSize: "0.68rem",
                  color: theme.text,
                  borderBottom: `1px solid ${theme.surfaceAlt}`,
                  cursor: "pointer",
                }}
              >
                <span style={{ color: theme.textFainter, fontSize: "0.55rem" }}>{chev}</span>
                <span>{ts}</span>
                <span
                  style={{
                    fontSize: "0.55rem",
                    padding: "0 5px",
                    borderRadius: 2,
                    background: theme.pill[pill].bg,
                    color: theme.pill[pill].fg,
                  }}
                >
                  {s.parseState}
                </span>
                <span style={{ marginLeft: "auto", fontSize: "0.6rem", color: theme.textDim }}>
                  {files.length || ""}
                </span>
              </button>
              {isOpen && visibleFiles.map((f) => {
                const active = state.selectedFileId === f.fileId;
                return (
                  <button
                    key={f.fileId}
                    type="button"
                    onClick={() => dispatch({ type: "select-file", sessionId: s.sessionId, fileId: f.fileId })}
                    style={{
                      all: "unset",
                      display: "flex",
                      width: "100%",
                      padding: "0.18rem 0.6rem 0.18rem 1.25rem",
                      gap: "0.4rem",
                      alignItems: "center",
                      fontFamily: theme.font.mono,
                      fontSize: "0.65rem",
                      color: active ? theme.accent : theme.text,
                      background: active ? theme.accentBg : "transparent",
                      borderLeft: active ? `2px solid ${theme.accent}` : "2px solid transparent",
                      cursor: "pointer",
                    }}
                  >
                    <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
                      {f.relativePath}
                    </span>
                    <span style={{ marginLeft: "auto", fontSize: "0.58rem", color: theme.textDim }}>
                      {formatCount(f.entryCount)}
                    </span>
                  </button>
                );
              })}
            </div>
          );
        })}
      </div>
    </div>
  );
}
