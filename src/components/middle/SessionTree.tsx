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

import { useCallback, useEffect, useRef, useState } from "react";
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
  const [reloadNonce, setReloadNonce] = useState(0);
  const [hoveredId, setHoveredId] = useState<string | null>(null);
  const cancelledRef = useRef(false);
  const retry = useCallback(() => setReloadNonce((n) => n + 1), []);

  useEffect(() => {
    cancelledRef.current = false;
    setLoading(true);
    setError(null);
    setSessions([]);
    setExpanded(new Set());
    setFilesBySession({});
    setFilter("");
    (async () => {
      try {
        const page = await listSessions(deviceId);
        if (!cancelledRef.current) setSessions(page.items);
      } catch (err) {
        if (!cancelledRef.current) setError(err instanceof Error ? err.message : String(err));
      } finally {
        if (!cancelledRef.current) setLoading(false);
      }
    })();
    return () => { cancelledRef.current = true; };
  }, [deviceId, reloadNonce]);

  async function toggleSession(sessionId: string) {
    // Decide synchronously from the current render whether this click expands
    // or collapses, then commit via a functional setState so two clicks on
    // different rows can't clobber each other's expansion sets.
    const willExpand = !expanded.has(sessionId);
    setExpanded((prev) => {
      const next = new Set(prev);
      if (next.has(sessionId)) next.delete(sessionId);
      else next.add(sessionId);
      return next;
    });
    if (!willExpand) return;
    // Only fetch if we haven't already cached the files for this session.
    // Read filesBySession via a functional updater below so two concurrent
    // toggles don't clobber each other's entries.
    try {
      const page = await listFiles(sessionId);
      if (cancelledRef.current) return;
      setFilesBySession((prev) =>
        prev[sessionId] ? prev : { ...prev, [sessionId]: page.items }
      );
    } catch {
      if (cancelledRef.current) return;
      setFilesBySession((prev) =>
        prev[sessionId] ? prev : { ...prev, [sessionId]: [] }
      );
    }
  }

  if (error) {
    return (
      <div style={{ padding: "0.7rem", fontFamily: theme.font.mono, fontSize: "0.7rem" }}>
        <div style={{ color: theme.pill.failed.fg }}>sessions unreachable: {error}</div>
        <button
          type="button"
          onClick={retry}
          style={{
            marginTop: "0.5rem",
            padding: "0.25rem 0.6rem",
            background: theme.surface,
            border: `1px solid ${theme.border}`,
            color: theme.accent,
            fontFamily: theme.font.mono,
            fontSize: "0.65rem",
            cursor: "pointer",
          }}
        >
          retry
        </button>
      </div>
    );
  }

  return (
    <div style={{ display: "flex", flexDirection: "column", height: "100%" }}>
      <style>{`@keyframes cmtrace-spin { to { transform: rotate(360deg) } }`}</style>
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
          <div style={{ display: "flex", alignItems: "center", gap: "0.4rem", padding: "0.7rem", color: theme.textDim, fontFamily: theme.font.mono, fontSize: "0.65rem" }}>
            <span style={{ display: "inline-block", width: 12, height: 12, border: `2px solid ${theme.border}`, borderTopColor: theme.accent, borderRadius: "50%", animation: "cmtrace-spin 0.8s linear infinite" }} />
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
                onMouseEnter={() => setHoveredId(s.sessionId)}
                onMouseLeave={() => setHoveredId(null)}
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
                  background: hoveredId === s.sessionId ? theme.hoverBg : "transparent",
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
                    onMouseEnter={() => setHoveredId(f.fileId)}
                    onMouseLeave={() => setHoveredId(null)}
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
                      background: active ? theme.accentBg : hoveredId === f.fileId ? theme.hoverBg : "transparent",
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
