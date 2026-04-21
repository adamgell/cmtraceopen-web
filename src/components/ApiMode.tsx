import { useCallback, useEffect, useState } from "react";
import {
  apiBase,
  listDevices,
  listEntries,
  listSessions,
} from "../lib/api-client";
import type {
  DeviceSummary,
  LogEntry,
  LogEntryDto,
  SessionSummary,
} from "../lib/log-types";
import { EntryList } from "./EntryList";

type FetchState<T> =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "ok"; data: T }
  | { status: "error"; error: string };

/**
 * API-mode view: three stacked panels that walk the hierarchy
 *
 *     devices → sessions → entries
 *
 * Each panel owns its own FetchState<T>. Selecting a row in panel N resets
 * panels > N so stale selections never render against a new parent.
 *
 * Entries come back as `LogEntryDto` (server wire format) and are mapped
 * to the WASM parser's `LogEntry` shape so we can reuse `EntryList.tsx`
 * unchanged. The mapping is lossy — server-side entries carry no format
 * specialization, so fields like `filePath` get synthesized from `fileId`
 * and `format` is hardcoded to "Plain". That's fine for v1: the viewer
 * just renders line/timestamp/severity/component/thread/message columns.
 */
export function ApiMode() {
  const [devices, setDevices] = useState<FetchState<DeviceSummary[]>>({ status: "idle" });
  const [selectedDevice, setSelectedDevice] = useState<string | null>(null);

  const [sessions, setSessions] = useState<FetchState<SessionSummary[]>>({ status: "idle" });
  const [selectedSession, setSelectedSession] = useState<string | null>(null);

  const [entries, setEntries] = useState<FetchState<LogEntry[]>>({ status: "idle" });

  // Panel 1: devices (fetched once on mount).
  useEffect(() => {
    let cancelled = false;
    setDevices({ status: "loading" });
    listDevices()
      .then((p) => {
        if (cancelled) return;
        setDevices({ status: "ok", data: p.items });
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        setDevices({ status: "error", error: formatError(err) });
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // Panel 2: sessions for the selected device.
  useEffect(() => {
    if (!selectedDevice) {
      setSessions({ status: "idle" });
      return;
    }
    let cancelled = false;
    setSessions({ status: "loading" });
    listSessions(selectedDevice)
      .then((p) => {
        if (cancelled) return;
        setSessions({ status: "ok", data: p.items });
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        setSessions({ status: "error", error: formatError(err) });
      });
    return () => {
      cancelled = true;
    };
  }, [selectedDevice]);

  // Panel 3: entries for the selected session.
  useEffect(() => {
    if (!selectedSession) {
      setEntries({ status: "idle" });
      return;
    }
    let cancelled = false;
    setEntries({ status: "loading" });
    listEntries(selectedSession, { limit: 500 })
      .then((p) => {
        if (cancelled) return;
        setEntries({ status: "ok", data: p.items.map(dtoToEntry) });
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        setEntries({ status: "error", error: formatError(err) });
      });
    return () => {
      cancelled = true;
    };
  }, [selectedSession]);

  const handleSelectDevice = useCallback((id: string) => {
    setSelectedDevice(id);
    setSelectedSession(null); // clear cascaded selection
  }, []);

  const handleSelectSession = useCallback((id: string) => {
    setSelectedSession(id);
  }, []);

  return (
    <div
      style={{
        flex: 1,
        minHeight: 0,
        display: "grid",
        // Three stacked panels: devices (compact), sessions (compact),
        // entries (fills remaining height so EntryList's virtualizer has
        // room to work).
        gridTemplateRows: "minmax(120px, 1fr) minmax(120px, 1fr) 3fr",
        gap: 12,
      }}
    >
      <Panel title="Devices" baseHint>
        <DeviceList
          state={devices}
          selected={selectedDevice}
          onSelect={handleSelectDevice}
        />
      </Panel>
      <Panel title={selectedDevice ? `Sessions — ${selectedDevice}` : "Sessions"}>
        {selectedDevice ? (
          <SessionList
            state={sessions}
            selected={selectedSession}
            onSelect={handleSelectSession}
          />
        ) : (
          <EmptyHint text="Select a device to list its sessions." />
        )}
      </Panel>
      <Panel title={selectedSession ? `Entries — ${selectedSession}` : "Entries"} flex>
        {selectedSession ? (
          <EntriesPanel state={entries} />
        ) : (
          <EmptyHint text="Select a session to load up to 500 entries." />
        )}
      </Panel>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Panels

function Panel({
  title,
  children,
  baseHint,
  flex,
}: {
  title: string;
  children: React.ReactNode;
  /** When true and the API base is empty, show the effective base-URL hint. */
  baseHint?: boolean;
  /** When true, the body region gets `flex: 1; minHeight: 0` so virtualized
   *  children (EntryList) size correctly. */
  flex?: boolean;
}) {
  return (
    <section
      style={{
        display: "flex",
        flexDirection: "column",
        minHeight: 0,
        border: "1px solid #ddd",
        borderRadius: 4,
        overflow: "hidden",
        background: "white",
      }}
    >
      <header
        style={{
          display: "flex",
          alignItems: "center",
          gap: 8,
          padding: "6px 10px",
          background: "#f5f5f5",
          borderBottom: "1px solid #ddd",
          fontSize: 12,
          fontWeight: 600,
          color: "#333",
        }}
      >
        <span>{title}</span>
        {baseHint && (
          <span style={{ fontWeight: 400, color: "#888", fontSize: 11 }}>
            base: {apiBase || "(same-origin)"}
          </span>
        )}
      </header>
      <div
        style={{
          flex: flex ? 1 : undefined,
          minHeight: 0,
          overflow: flex ? "hidden" : "auto",
          display: "flex",
          flexDirection: "column",
        }}
      >
        {children}
      </div>
    </section>
  );
}

function DeviceList({
  state,
  selected,
  onSelect,
}: {
  state: FetchState<DeviceSummary[]>;
  selected: string | null;
  onSelect: (id: string) => void;
}) {
  if (state.status === "loading") return <CenteredText text="Loading devices…" muted />;
  if (state.status === "error") return <ApiError error={state.error} />;
  if (state.status === "idle") return null;
  if (state.data.length === 0) return <EmptyHint text="No devices have reported yet." />;

  return (
    <ul style={listStyle}>
      {state.data.map((d) => (
        <li key={d.deviceId}>
          <RowButton
            selected={selected === d.deviceId}
            onClick={() => onSelect(d.deviceId)}
          >
            <span style={{ fontWeight: 500 }}>{d.deviceId}</span>
            <span style={metaStyle}>
              {d.sessionCount} session{d.sessionCount === 1 ? "" : "s"}
              {" · last seen "}
              {formatUtc(d.lastSeenUtc)}
              {d.hostname ? ` · ${d.hostname}` : ""}
            </span>
          </RowButton>
        </li>
      ))}
    </ul>
  );
}

function SessionList({
  state,
  selected,
  onSelect,
}: {
  state: FetchState<SessionSummary[]>;
  selected: string | null;
  onSelect: (id: string) => void;
}) {
  if (state.status === "loading") return <CenteredText text="Loading sessions…" muted />;
  if (state.status === "error") return <ApiError error={state.error} />;
  if (state.status === "idle") return null;
  if (state.data.length === 0) return <EmptyHint text="No sessions for this device." />;

  return (
    <ul style={listStyle}>
      {state.data.map((s) => (
        <li key={s.sessionId}>
          <RowButton
            selected={selected === s.sessionId}
            onClick={() => onSelect(s.sessionId)}
          >
            <span style={{ fontFamily: "ui-monospace, Menlo, Consolas, monospace" }}>
              {s.sessionId}
            </span>
            <span style={metaStyle}>
              ingested {formatUtc(s.ingestedUtc)} · parse: {s.parseState}
            </span>
          </RowButton>
        </li>
      ))}
    </ul>
  );
}

function EntriesPanel({ state }: { state: FetchState<LogEntry[]> }) {
  if (state.status === "loading") return <CenteredText text="Loading entries…" muted />;
  if (state.status === "error") return <ApiError error={state.error} />;
  if (state.status === "idle") return null;
  if (state.data.length === 0) return <EmptyHint text="No entries in this session." />;
  return <EntryList entries={state.data} />;
}

// ---------------------------------------------------------------------------
// Leaf UI bits (no dependencies, inline CSS only)

function RowButton({
  selected,
  onClick,
  children,
}: {
  selected: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      style={{
        width: "100%",
        display: "flex",
        flexDirection: "column",
        alignItems: "flex-start",
        gap: 2,
        padding: "6px 10px",
        border: "none",
        borderBottom: "1px solid #f0f0f0",
        background: selected ? "#eef4ff" : "transparent",
        color: "#222",
        fontSize: 13,
        textAlign: "left",
        cursor: "pointer",
      }}
    >
      {children}
    </button>
  );
}

function ApiError({ error }: { error: string }) {
  // Heuristic: surface a friendlier message when the fetch itself failed
  // (DNS / refused / offline). We can't detect this structurally from a
  // thrown Error, so match on the TypeError signature fetch emits.
  const unreachable =
    /failed to fetch|networkerror|fetch failed/i.test(error);
  return (
    <div
      style={{
        margin: 10,
        padding: "10px 12px",
        background: "#fef2f2",
        border: "1px solid #fecaca",
        color: "#991b1b",
        borderRadius: 4,
        fontSize: 13,
        whiteSpace: "pre-wrap",
      }}
    >
      {unreachable
        ? `Cannot reach API at ${apiBase || "(same-origin)"}. Is the api-server running?`
        : error}
    </div>
  );
}

function EmptyHint({ text }: { text: string }) {
  return (
    <div style={{ padding: 12, color: "#777", fontSize: 13 }}>{text}</div>
  );
}

function CenteredText({ text, muted }: { text: string; muted?: boolean }) {
  return (
    <div
      style={{
        padding: 14,
        color: muted ? "#777" : "#222",
        fontSize: 13,
      }}
    >
      {text}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Helpers

const listStyle: React.CSSProperties = {
  listStyle: "none",
  margin: 0,
  padding: 0,
  overflow: "auto",
  flex: 1,
  minHeight: 0,
};

const metaStyle: React.CSSProperties = {
  color: "#777",
  fontSize: 11,
};

function formatUtc(iso: string): string {
  // Avoid Intl overhead: the API already sends RFC3339; trim the TZ suffix
  // and use the "YYYY-MM-DD HH:MM:SS" slice for compactness.
  return iso.replace("T", " ").replace(/\.\d+Z?$/, "").replace(/Z$/, "");
}

function formatError(err: unknown): string {
  if (err instanceof Error) return err.message;
  if (typeof err === "string") return err;
  try {
    return JSON.stringify(err);
  } catch {
    return String(err);
  }
}

/**
 * Map the server `LogEntryDto` to the WASM parser's `LogEntry` shape so
 * `EntryList` can render both without branching. Fields absent on the
 * server side (format, specialization, error-code spans) are filled with
 * conservative defaults — the list UI only consumes the common columns.
 */
function dtoToEntry(dto: LogEntryDto): LogEntry {
  const timestamp = dto.tsMs;
  const timestampDisplay =
    typeof timestamp === "number"
      ? new Date(timestamp).toISOString().replace("T", " ").replace(/\.\d+Z$/, "")
      : undefined;
  // `thread` is a string on the wire but a number on the LogEntry side;
  // preserve the original via threadDisplay so we don't lose info like
  // "tid-42" or hex ids the server may emit.
  const threadNum =
    typeof dto.thread === "string" && /^\d+$/.test(dto.thread)
      ? Number(dto.thread)
      : undefined;
  return {
    id: dto.entryId,
    lineNumber: dto.lineNumber,
    message: dto.message,
    component: dto.component,
    timestamp,
    timestampDisplay,
    severity: dto.severity,
    thread: threadNum,
    threadDisplay: dto.thread,
    sourceFile: undefined,
    format: "Plain",
    filePath: dto.fileId,
    timezoneOffset: undefined,
  };
}
