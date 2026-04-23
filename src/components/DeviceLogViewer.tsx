import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Badge,
  Button,
  Spinner,
  Tooltip,
  tokens,
} from "@fluentui/react-components";
import {
  listSessions as apiListSessions,
  listFiles as apiListFiles,
  listEntries as apiListEntries,
} from "../lib/api-client";
import type {
  DeviceSummary,
  LogEntry,
  SessionFile,
  SessionSummary,
} from "../lib/log-types";
import { EntryList } from "./EntryList";
import { FilterBar, defaultFilters, type Filters } from "./FilterBar";
import { useWorkspace } from "../lib/workspace-context";
import {
  LOG_MONOSPACE_FONT_FAMILY,
  LOG_UI_FONT_FAMILY,
} from "../lib/log-accessibility";

export interface DeviceLogViewerProps {
  /** Whichever DeviceSummary row the operator clicked "See logs" on. */
  device: DeviceSummary;
  /** Back to the Devices grid. */
  onClose: () => void;
}

/**
 * Full-screen forensics console for a single device.
 *
 * Visual intent: this is the "command bridge" view for an agent. The Devices
 * grid is the fleet map; clicking in takes you here, where the device is
 * front-and-centre and the session/file tree is arranged for fast scanning
 * and keyboard-driven triage. The treatment leans into a density-forward,
 * signal-heavy aesthetic: display-typography hostname banner, hairline
 * accent rule, dotted texture behind the header, monospace metadata
 * chips, and a left rail that reads like a terminal's file picker.
 *
 * Keyboard:
 *   Escape       - back to Devices
 *   ArrowUp/Down - move file cursor (across all files in all sessions)
 *   Enter        - load the cursor'd file
 *   p            - toggle pin on the focused file
 *
 * State model: the rail fetches sessions for the device, then lazily
 * fetches files per session on expand. Entries are fetched on file
 * select. FilterBar is per-load (resets when the file changes).
 */
export function DeviceLogViewer({ device, onClose }: DeviceLogViewerProps) {
  // --- Sessions ------------------------------------------------------------
  const [sessionsState, setSessionsState] = useState<
    | { status: "loading" }
    | { status: "ok"; items: SessionSummary[] }
    | { status: "error"; message: string }
  >({ status: "loading" });

  useEffect(() => {
    let alive = true;
    setSessionsState({ status: "loading" });
    apiListSessions(device.deviceId)
      .then((page) => {
        if (!alive) return;
        setSessionsState({ status: "ok", items: page.items });
      })
      .catch((e: unknown) => {
        if (!alive) return;
        setSessionsState({
          status: "error",
          message: e instanceof Error ? e.message : String(e),
        });
      });
    return () => {
      alive = false;
    };
  }, [device.deviceId]);

  // Which sessions are expanded in the rail. Default: first one open.
  const [expandedSessionIds, setExpandedSessionIds] = useState<Set<string>>(
    new Set(),
  );
  useEffect(() => {
    if (sessionsState.status === "ok" && sessionsState.items.length > 0) {
      setExpandedSessionIds((prev) =>
        prev.size === 0 ? new Set([sessionsState.items[0]!.sessionId]) : prev,
      );
    }
  }, [sessionsState]);

  // Per-session file map (lazily populated on expand).
  const [filesByS, setFilesByS] = useState<
    Map<
      string,
      | { status: "loading" }
      | { status: "ok"; items: SessionFile[] }
      | { status: "error"; message: string }
    >
  >(() => new Map());

  const loadFilesFor = useCallback(
    async (sessionId: string) => {
      setFilesByS((prev) => {
        const next = new Map(prev);
        next.set(sessionId, { status: "loading" });
        return next;
      });
      try {
        const page = await apiListFiles(sessionId, { limit: 500 });
        setFilesByS((prev) => {
          const next = new Map(prev);
          next.set(sessionId, { status: "ok", items: page.items });
          return next;
        });
      } catch (e) {
        setFilesByS((prev) => {
          const next = new Map(prev);
          next.set(sessionId, {
            status: "error",
            message: e instanceof Error ? e.message : String(e),
          });
          return next;
        });
      }
    },
    [],
  );

  useEffect(() => {
    for (const sessionId of expandedSessionIds) {
      if (!filesByS.has(sessionId)) {
        void loadFilesFor(sessionId);
      }
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [expandedSessionIds]);

  // --- Active file + entries ----------------------------------------------
  const [activeFileKey, setActiveFileKey] = useState<
    { sessionId: string; fileId: string } | null
  >(null);

  const [entriesState, setEntriesState] = useState<
    | { status: "idle" }
    | { status: "loading" }
    | { status: "ok"; entries: LogEntry[]; truncated: boolean }
    | { status: "error"; message: string }
  >({ status: "idle" });

  const [filters, setFilters] = useState<Filters>(defaultFilters);

  useEffect(() => {
    if (!activeFileKey) {
      setEntriesState({ status: "idle" });
      return;
    }
    let alive = true;
    setEntriesState({ status: "loading" });
    setFilters(defaultFilters());
    apiListEntries(activeFileKey.sessionId, {
      file: activeFileKey.fileId,
      limit: 500,
    })
      .then((page) => {
        if (!alive) return;
        setEntriesState({
          status: "ok",
          entries: page.items as unknown as LogEntry[],
          truncated: page.nextCursor != null,
        });
      })
      .catch((e: unknown) => {
        if (!alive) return;
        setEntriesState({
          status: "error",
          message: e instanceof Error ? e.message : String(e),
        });
      });
    return () => {
      alive = false;
    };
  }, [activeFileKey?.sessionId, activeFileKey?.fileId]);

  // --- Flat file index for keyboard navigation -----------------------------
  interface RailFileRef {
    sessionId: string;
    file: SessionFile;
  }
  const flatFiles: RailFileRef[] = useMemo(() => {
    const out: RailFileRef[] = [];
    if (sessionsState.status !== "ok") return out;
    for (const s of sessionsState.items) {
      const bucket = filesByS.get(s.sessionId);
      if (!bucket || bucket.status !== "ok") continue;
      for (const f of bucket.items) {
        out.push({ sessionId: s.sessionId, file: f });
      }
    }
    return out;
  }, [sessionsState, filesByS]);

  const activeFlatIdx = useMemo(() => {
    if (!activeFileKey) return -1;
    return flatFiles.findIndex(
      (x) =>
        x.sessionId === activeFileKey.sessionId &&
        x.file.fileId === activeFileKey.fileId,
    );
  }, [flatFiles, activeFileKey]);

  // --- Workspace (pinning) -------------------------------------------------
  const workspace = useWorkspace();

  const isPinned = useCallback(
    (sessionId: string, file: SessionFile): boolean =>
      !!workspace.findExisting({
        kind: "api-file",
        label: file.relativePath,
        deviceId: device.deviceId,
        sessionId,
        fileId: file.fileId,
        relativePath: file.relativePath,
      }),
    [workspace, device.deviceId],
  );

  const togglePin = useCallback(
    (sessionId: string, file: SessionFile) => {
      const input = {
        kind: "api-file" as const,
        label: `${device.hostname ?? device.deviceId} / ${sessionId.slice(0, 8)} / ${file.relativePath}`,
        deviceId: device.deviceId,
        sessionId,
        fileId: file.fileId,
        relativePath: file.relativePath,
      };
      const existing = workspace.findExisting(input);
      if (existing) {
        workspace.unpin(existing);
      } else {
        workspace.pin(input);
      }
    },
    [workspace, device.hostname, device.deviceId],
  );

  // --- Keyboard nav --------------------------------------------------------
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        onClose();
        return;
      }
      if (flatFiles.length === 0) return;
      if (e.key === "ArrowDown" && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        const next = Math.min(flatFiles.length - 1, activeFlatIdx + 1);
        const pick = flatFiles[next];
        if (pick) {
          setActiveFileKey({ sessionId: pick.sessionId, fileId: pick.file.fileId });
        }
      } else if (e.key === "ArrowUp" && (e.metaKey || e.ctrlKey)) {
        e.preventDefault();
        const next = Math.max(0, activeFlatIdx - 1);
        const pick = flatFiles[next];
        if (pick) {
          setActiveFileKey({ sessionId: pick.sessionId, fileId: pick.file.fileId });
        }
      } else if (e.key === "p" && activeFileKey && !e.metaKey && !e.ctrlKey && !e.altKey) {
        // Plain `p` — only fire when focus isn't inside a text input.
        const target = e.target as HTMLElement | null;
        if (
          target &&
          (target.tagName === "INPUT" ||
            target.tagName === "TEXTAREA" ||
            target.isContentEditable)
        ) {
          return;
        }
        const pick = flatFiles[activeFlatIdx];
        if (pick) {
          togglePin(pick.sessionId, pick.file);
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose, activeFlatIdx, flatFiles, togglePin, activeFileKey]);

  // --- Render --------------------------------------------------------------
  const lastSeenDelta = useMemo(
    () => humanRelative(device.lastSeenUtc),
    [device.lastSeenUtc],
  );

  const totalFileCount = useMemo(() => {
    let n = 0;
    for (const [, state] of filesByS) {
      if (state.status === "ok") n += state.items.length;
    }
    return n;
  }, [filesByS]);

  return (
    <div
      style={{
        position: "fixed",
        inset: 0,
        zIndex: 100,
        display: "flex",
        flexDirection: "column",
        background: tokens.colorNeutralBackground1,
        color: tokens.colorNeutralForeground1,
        // Subtle slide-up on mount. No library; pure CSS keyframe on the
        // parent so screen readers aren't bombarded.
        animation: "cmtrace-devlog-enter 220ms cubic-bezier(.18,.79,.35,1) both",
      }}
    >
      {/* inline keyframes — no global stylesheet needed */}
      <style>{`
        @keyframes cmtrace-devlog-enter {
          from { opacity: 0; transform: translateY(12px); }
          to   { opacity: 1; transform: translateY(0); }
        }
      `}</style>

      <DeviceHeader
        device={device}
        lastSeenDelta={lastSeenDelta}
        fileCount={totalFileCount}
        activeFile={
          activeFileKey && sessionsState.status === "ok"
            ? findActiveFile(sessionsState.items, filesByS, activeFileKey)
            : null
        }
        onClose={onClose}
      />

      <div
        style={{
          flex: 1,
          minHeight: 0,
          display: "grid",
          gridTemplateColumns: "320px 1fr",
        }}
      >
        <SessionRail
          state={sessionsState}
          filesByS={filesByS}
          expandedSessionIds={expandedSessionIds}
          activeFileKey={activeFileKey}
          onToggleSession={(id) =>
            setExpandedSessionIds((prev) => {
              const next = new Set(prev);
              if (next.has(id)) next.delete(id);
              else next.add(id);
              return next;
            })
          }
          onSelectFile={(sessionId, file) =>
            setActiveFileKey({ sessionId, fileId: file.fileId })
          }
          onPin={(sessionId, file) => togglePin(sessionId, file)}
          isPinned={isPinned}
        />

        <MainArea
          entriesState={entriesState}
          filters={filters}
          setFilters={setFilters}
        />
      </div>
    </div>
  );
}

/* ──────────────────────── Header ──────────────────────── */

function DeviceHeader({
  device,
  lastSeenDelta,
  fileCount,
  activeFile,
  onClose,
}: {
  device: DeviceSummary;
  lastSeenDelta: string;
  fileCount: number;
  activeFile: { sessionId: string; file: SessionFile } | null;
  onClose: () => void;
}) {
  return (
    <header
      style={{
        position: "relative",
        padding: "20px 24px 16px",
        borderBottom: `1px solid ${tokens.colorNeutralStroke1}`,
        // Dotted texture behind the header — subtle, theme-aware. Rendered
        // via a layered radial-gradient so it respects the active theme's
        // foreground color without needing a separate SVG asset.
        backgroundImage: `radial-gradient(${tokens.colorNeutralStroke2} 1px, transparent 1px)`,
        backgroundSize: "12px 12px",
        backgroundPosition: "0 0",
      }}
    >
      {/* Hairline accent rule at the very top of the header. Uses the
          active theme's brand ramp so it flips with the theme picker. */}
      <div
        style={{
          position: "absolute",
          inset: "0 0 auto 0",
          height: 2,
          background: `linear-gradient(90deg, ${tokens.colorBrandBackground}, ${tokens.colorBrandBackground2}, transparent 80%)`,
          pointerEvents: "none",
        }}
        aria-hidden
      />

      <div
        style={{
          display: "flex",
          alignItems: "flex-start",
          gap: 20,
          flexWrap: "wrap",
        }}
      >
        <div style={{ flex: 1, minWidth: 280 }}>
          <div
            style={{
              fontFamily: LOG_UI_FONT_FAMILY,
              fontSize: 11,
              fontWeight: 600,
              letterSpacing: "0.16em",
              textTransform: "uppercase",
              color: tokens.colorBrandForeground1,
              marginBottom: 4,
            }}
          >
            Device · Logs
          </div>
          <h1
            style={{
              margin: 0,
              fontFamily: LOG_UI_FONT_FAMILY,
              fontSize: 30,
              fontWeight: 800,
              letterSpacing: "-0.015em",
              lineHeight: 1.05,
              color: tokens.colorNeutralForeground1,
            }}
          >
            {device.hostname ?? device.deviceId}
          </h1>
          <div
            style={{
              marginTop: 8,
              display: "flex",
              flexWrap: "wrap",
              gap: 6,
              alignItems: "center",
              fontFamily: LOG_MONOSPACE_FONT_FAMILY,
              fontSize: 12,
              color: tokens.colorNeutralForeground3,
            }}
          >
            <MetaChip label="ID" value={device.deviceId} mono />
            <MetaChip label="Last seen" value={lastSeenDelta} />
            <MetaChip
              label="Sessions"
              value={String(device.sessionCount ?? 0)}
            />
            {fileCount > 0 && (
              <MetaChip label="Files loaded" value={String(fileCount)} />
            )}
            {device.status && device.status !== "active" && (
              <Badge appearance="tint" color="danger" size="small">
                {device.status}
              </Badge>
            )}
          </div>
          {activeFile && (
            <div
              style={{
                marginTop: 10,
                fontFamily: LOG_MONOSPACE_FONT_FAMILY,
                fontSize: 12,
                color: tokens.colorNeutralForeground2,
              }}
            >
              <span
                style={{
                  color: tokens.colorBrandForeground1,
                  marginRight: 6,
                }}
              >
                ▸
              </span>
              Showing{" "}
              <span style={{ color: tokens.colorNeutralForeground1 }}>
                {activeFile.file.relativePath}
              </span>{" "}
              from{" "}
              <span
                style={{
                  color: tokens.colorNeutralForeground2,
                  opacity: 0.8,
                }}
              >
                {activeFile.sessionId.slice(0, 8)}…
              </span>
            </div>
          )}
        </div>

        <div
          style={{
            display: "flex",
            flexDirection: "column",
            alignItems: "flex-end",
            gap: 8,
          }}
        >
          <Button appearance="subtle" size="small" onClick={onClose}>
            ← Back to devices
          </Button>
          <div
            style={{
              fontFamily: LOG_UI_FONT_FAMILY,
              fontSize: 10,
              fontWeight: 500,
              letterSpacing: "0.12em",
              textTransform: "uppercase",
              color: tokens.colorNeutralForeground4,
              textAlign: "right",
            }}
          >
            <div>Esc · back</div>
            <div>⌘↑ / ⌘↓ · next file</div>
            <div>p · pin</div>
          </div>
        </div>
      </div>
    </header>
  );
}

function MetaChip({
  label,
  value,
  mono,
}: {
  label: string;
  value: string;
  mono?: boolean;
}) {
  return (
    <span
      style={{
        display: "inline-flex",
        alignItems: "baseline",
        gap: 6,
        padding: "2px 8px",
        borderRadius: tokens.borderRadiusSmall,
        background: tokens.colorNeutralBackground3,
        border: `1px solid ${tokens.colorNeutralStroke2}`,
      }}
    >
      <span
        style={{
          fontFamily: LOG_UI_FONT_FAMILY,
          fontSize: 10,
          fontWeight: 600,
          letterSpacing: "0.08em",
          textTransform: "uppercase",
          color: tokens.colorNeutralForeground3,
        }}
      >
        {label}
      </span>
      <span
        style={{
          fontFamily: mono ? LOG_MONOSPACE_FONT_FAMILY : LOG_UI_FONT_FAMILY,
          fontSize: 12,
          color: tokens.colorNeutralForeground1,
        }}
      >
        {value}
      </span>
    </span>
  );
}

/* ──────────────────────── Rail ──────────────────────── */

function SessionRail({
  state,
  filesByS,
  expandedSessionIds,
  activeFileKey,
  onToggleSession,
  onSelectFile,
  onPin,
  isPinned,
}: {
  state:
    | { status: "loading" }
    | { status: "ok"; items: SessionSummary[] }
    | { status: "error"; message: string };
  filesByS: Map<
    string,
    | { status: "loading" }
    | { status: "ok"; items: SessionFile[] }
    | { status: "error"; message: string }
  >;
  expandedSessionIds: Set<string>;
  activeFileKey: { sessionId: string; fileId: string } | null;
  onToggleSession: (sessionId: string) => void;
  onSelectFile: (sessionId: string, file: SessionFile) => void;
  onPin: (sessionId: string, file: SessionFile) => void;
  isPinned: (sessionId: string, file: SessionFile) => boolean;
}) {
  return (
    <aside
      style={{
        borderRight: `1px solid ${tokens.colorNeutralStroke1}`,
        background: tokens.colorNeutralBackground2,
        overflow: "auto",
        fontFamily: LOG_UI_FONT_FAMILY,
        fontSize: 13,
        color: tokens.colorNeutralForeground1,
      }}
    >
      <div
        style={{
          padding: "10px 14px",
          borderBottom: `1px solid ${tokens.colorNeutralStroke2}`,
          fontSize: 11,
          fontWeight: 600,
          letterSpacing: "0.14em",
          textTransform: "uppercase",
          color: tokens.colorNeutralForeground3,
          position: "sticky",
          top: 0,
          background: tokens.colorNeutralBackground2,
          zIndex: 1,
        }}
      >
        Sessions
      </div>

      {state.status === "loading" && (
        <div style={{ padding: 16, display: "flex", gap: 8, alignItems: "center" }}>
          <Spinner size="tiny" />
          <span style={{ color: tokens.colorNeutralForeground2 }}>
            Loading…
          </span>
        </div>
      )}
      {state.status === "error" && (
        <div
          style={{
            padding: 16,
            color: tokens.colorPaletteRedForeground1,
            fontFamily: LOG_MONOSPACE_FONT_FAMILY,
            fontSize: 12,
          }}
        >
          {state.message}
        </div>
      )}
      {state.status === "ok" && state.items.length === 0 && (
        <div
          style={{
            padding: 16,
            color: tokens.colorNeutralForeground3,
            fontStyle: "italic",
          }}
        >
          No sessions ingested for this device yet.
        </div>
      )}
      {state.status === "ok" &&
        state.items.map((session) => {
          const isOpen = expandedSessionIds.has(session.sessionId);
          const bucket = filesByS.get(session.sessionId);
          return (
            <div
              key={session.sessionId}
              style={{
                borderBottom: `1px solid ${tokens.colorNeutralStroke2}`,
              }}
            >
              <button
                type="button"
                onClick={() => onToggleSession(session.sessionId)}
                style={{
                  display: "flex",
                  alignItems: "center",
                  gap: 8,
                  width: "100%",
                  textAlign: "left",
                  padding: "10px 14px",
                  background: "transparent",
                  color: tokens.colorNeutralForeground1,
                  border: "none",
                  cursor: "pointer",
                  fontFamily: LOG_UI_FONT_FAMILY,
                  fontSize: 12,
                }}
                aria-expanded={isOpen}
              >
                <span
                  aria-hidden
                  style={{
                    display: "inline-block",
                    transform: isOpen ? "rotate(90deg)" : "rotate(0deg)",
                    transition: "transform 140ms ease",
                    color: tokens.colorNeutralForeground3,
                    fontFamily: LOG_MONOSPACE_FONT_FAMILY,
                  }}
                >
                  ▸
                </span>
                <div style={{ flex: 1, minWidth: 0 }}>
                  <div
                    style={{
                      fontFamily: LOG_MONOSPACE_FONT_FAMILY,
                      fontSize: 12,
                      color: tokens.colorNeutralForeground1,
                      overflow: "hidden",
                      textOverflow: "ellipsis",
                      whiteSpace: "nowrap",
                    }}
                  >
                    {session.sessionId.slice(0, 8)}…
                    {session.sessionId.slice(-4)}
                  </div>
                  <div
                    style={{
                      fontSize: 11,
                      color: tokens.colorNeutralForeground3,
                      marginTop: 2,
                      overflow: "hidden",
                      textOverflow: "ellipsis",
                      whiteSpace: "nowrap",
                    }}
                  >
                    {humanRelative(session.ingestedUtc)} ·{" "}
                    {(session.sizeBytes / 1024).toFixed(0)} KB
                  </div>
                </div>
                <SessionParseBadge state={session.parseState} />
              </button>

              {isOpen && (
                <div style={{ background: tokens.colorNeutralBackground1 }}>
                  {!bucket && null}
                  {bucket?.status === "loading" && (
                    <div
                      style={{
                        padding: "8px 14px 8px 34px",
                        color: tokens.colorNeutralForeground3,
                      }}
                    >
                      <Spinner size="tiny" />
                    </div>
                  )}
                  {bucket?.status === "error" && (
                    <div
                      style={{
                        padding: "8px 14px 8px 34px",
                        color: tokens.colorPaletteRedForeground1,
                        fontFamily: LOG_MONOSPACE_FONT_FAMILY,
                        fontSize: 11,
                      }}
                    >
                      {bucket.message}
                    </div>
                  )}
                  {bucket?.status === "ok" && bucket.items.length === 0 && (
                    <div
                      style={{
                        padding: "8px 14px 8px 34px",
                        color: tokens.colorNeutralForeground3,
                        fontStyle: "italic",
                        fontSize: 12,
                      }}
                    >
                      No files in this session.
                    </div>
                  )}
                  {bucket?.status === "ok" &&
                    bucket.items.map((file) => {
                      const isActive =
                        activeFileKey?.sessionId === session.sessionId &&
                        activeFileKey.fileId === file.fileId;
                      const pinned = isPinned(session.sessionId, file);
                      return (
                        <div
                          key={file.fileId}
                          onClick={() => onSelectFile(session.sessionId, file)}
                          onKeyDown={(e) => {
                            if (e.key === "Enter" || e.key === " ") {
                              e.preventDefault();
                              onSelectFile(session.sessionId, file);
                            }
                          }}
                          role="option"
                          aria-selected={isActive}
                          tabIndex={0}
                          style={{
                            display: "grid",
                            gridTemplateColumns: "1fr auto",
                            gap: 8,
                            padding: "6px 12px 6px 32px",
                            cursor: "pointer",
                            background: isActive
                              ? tokens.colorBrandBackground2
                              : "transparent",
                            borderLeft: isActive
                              ? `3px solid ${tokens.colorBrandForeground1}`
                              : "3px solid transparent",
                            color: tokens.colorNeutralForeground1,
                            alignItems: "center",
                          }}
                        >
                          <div style={{ minWidth: 0 }}>
                            <div
                              style={{
                                fontFamily: LOG_MONOSPACE_FONT_FAMILY,
                                fontSize: 12,
                                overflow: "hidden",
                                textOverflow: "ellipsis",
                                whiteSpace: "nowrap",
                                color: isActive
                                  ? tokens.colorBrandForeground1
                                  : tokens.colorNeutralForeground1,
                              }}
                            >
                              {file.relativePath}
                            </div>
                            <div
                              style={{
                                fontSize: 11,
                                color: tokens.colorNeutralForeground3,
                                marginTop: 1,
                              }}
                            >
                              {file.entryCount?.toLocaleString()} entries
                              {file.parseErrorCount ? (
                                <span
                                  style={{
                                    marginLeft: 6,
                                    color: tokens.colorPaletteRedForeground1,
                                  }}
                                >
                                  {file.parseErrorCount} parse errors
                                </span>
                              ) : null}
                            </div>
                          </div>
                          <Tooltip
                            content={pinned ? "Unpin from workspace" : "Pin to workspace"}
                            relationship="label"
                            withArrow
                          >
                            <button
                              type="button"
                              onClick={(e) => {
                                e.stopPropagation();
                                onPin(session.sessionId, file);
                              }}
                              aria-pressed={pinned}
                              aria-label={pinned ? "Unpin" : "Pin"}
                              style={{
                                background: "transparent",
                                border: "none",
                                color: pinned
                                  ? tokens.colorStatusWarningForeground1
                                  : tokens.colorNeutralForeground3,
                                cursor: "pointer",
                                padding: 4,
                                lineHeight: 0,
                                borderRadius: tokens.borderRadiusSmall,
                              }}
                            >
                              {/* Inline star — filled when pinned. */}
                              <svg width="14" height="14" viewBox="0 0 24 24" fill={pinned ? "currentColor" : "none"} stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                                <polygon points="12 2 15 8.5 22 9.3 17 14 18.5 21 12 17.8 5.5 21 7 14 2 9.3 9 8.5 12 2" />
                              </svg>
                            </button>
                          </Tooltip>
                        </div>
                      );
                    })}
                </div>
              )}
            </div>
          );
        })}
    </aside>
  );
}

function SessionParseBadge({ state }: { state: string }) {
  const lower = state.toLowerCase();
  const tone: "neutral" | "warn" | "err" | "ok" =
    lower === "ok" || lower === "complete"
      ? "ok"
      : lower === "partial"
        ? "warn"
        : lower === "error" || lower === "failed"
          ? "err"
          : "neutral";
  const bg =
    tone === "ok"
      ? tokens.colorPaletteGreenBackground2
      : tone === "warn"
        ? tokens.colorPaletteYellowBackground2
        : tone === "err"
          ? tokens.colorPaletteRedBackground2
          : tokens.colorNeutralBackground3;
  const fg =
    tone === "ok"
      ? tokens.colorPaletteGreenForeground1
      : tone === "warn"
        ? tokens.colorPaletteDarkOrangeForeground1
        : tone === "err"
          ? tokens.colorPaletteRedForeground1
          : tokens.colorNeutralForeground3;
  return (
    <span
      style={{
        fontSize: 10,
        fontFamily: LOG_UI_FONT_FAMILY,
        fontWeight: 600,
        textTransform: "uppercase",
        letterSpacing: "0.08em",
        padding: "2px 8px",
        borderRadius: tokens.borderRadiusCircular,
        background: bg,
        color: fg,
        whiteSpace: "nowrap",
      }}
    >
      {state}
    </span>
  );
}

/* ──────────────────────── Main ──────────────────────── */

function MainArea({
  entriesState,
  filters,
  setFilters,
}: {
  entriesState:
    | { status: "idle" }
    | { status: "loading" }
    | { status: "ok"; entries: LogEntry[]; truncated: boolean }
    | { status: "error"; message: string };
  filters: Filters;
  setFilters: (f: Filters) => void;
}) {
  if (entriesState.status === "idle") {
    return (
      <div
        style={{
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          justifyContent: "center",
          padding: 32,
          textAlign: "center",
          color: tokens.colorNeutralForeground3,
          fontFamily: LOG_UI_FONT_FAMILY,
        }}
      >
        <div
          style={{
            fontSize: 48,
            marginBottom: 12,
            opacity: 0.25,
            fontFamily: LOG_MONOSPACE_FONT_FAMILY,
          }}
          aria-hidden
        >
          ▸
        </div>
        <div
          style={{
            fontSize: 14,
            fontWeight: 600,
            color: tokens.colorNeutralForeground2,
          }}
        >
          Pick a file from the rail
        </div>
        <div
          style={{
            fontSize: 12,
            marginTop: 4,
            maxWidth: 360,
          }}
        >
          Expand a session on the left to see the files it bundled, then click one to stream its entries here.
        </div>
      </div>
    );
  }
  if (entriesState.status === "loading") {
    return (
      <div
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "center",
          color: tokens.colorNeutralForeground2,
          fontFamily: LOG_UI_FONT_FAMILY,
        }}
      >
        <Spinner label="Loading entries…" size="small" />
      </div>
    );
  }
  if (entriesState.status === "error") {
    return (
      <div
        style={{
          padding: 24,
          color: tokens.colorPaletteRedForeground1,
          fontFamily: LOG_MONOSPACE_FONT_FAMILY,
        }}
      >
        {entriesState.message}
      </div>
    );
  }
  const total = entriesState.entries.length;
  const shown = total;
  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        padding: 16,
        gap: 12,
        minWidth: 0,
        minHeight: 0,
      }}
    >
      {entriesState.truncated && (
        <div
          style={{
            padding: "6px 10px",
            background: tokens.colorPaletteYellowBackground2,
            color: tokens.colorPaletteDarkOrangeForeground1,
            border: `1px solid ${tokens.colorPaletteYellowBorderActive}`,
            borderRadius: tokens.borderRadiusMedium,
            fontSize: 12,
            fontFamily: LOG_UI_FONT_FAMILY,
          }}
        >
          Showing first 500 entries. Older lines from this file aren't loaded.
        </div>
      )}
      <FilterBar
        filters={filters}
        onChange={setFilters}
        total={total}
        shown={shown}
      />
      <EntryList entries={entriesState.entries} filters={filters} />
    </div>
  );
}

/* ──────────────────────── helpers ──────────────────────── */

function findActiveFile(
  sessions: SessionSummary[],
  filesByS: Map<
    string,
    | { status: "loading" }
    | { status: "ok"; items: SessionFile[] }
    | { status: "error"; message: string }
  >,
  key: { sessionId: string; fileId: string },
): { sessionId: string; file: SessionFile } | null {
  const bucket = filesByS.get(key.sessionId);
  if (!bucket || bucket.status !== "ok") return null;
  const file = bucket.items.find((f) => f.fileId === key.fileId);
  if (!file) return null;
  // sessions param kept for future enrichment (e.g. include session label)
  void sessions;
  return { sessionId: key.sessionId, file };
}

function humanRelative(isoUtc: string | undefined): string {
  if (!isoUtc) return "—";
  const then = Date.parse(isoUtc);
  if (!Number.isFinite(then)) return "—";
  const deltaMs = Date.now() - then;
  const abs = Math.abs(deltaMs);
  const MIN = 60_000;
  const HOUR = 60 * MIN;
  const DAY = 24 * HOUR;
  if (abs < MIN) return `${Math.round(abs / 1000)}s ago`;
  if (abs < HOUR) return `${Math.round(abs / MIN)}m ago`;
  if (abs < DAY) return `${Math.round(abs / HOUR)}h ago`;
  return `${Math.round(abs / DAY)}d ago`;
}

