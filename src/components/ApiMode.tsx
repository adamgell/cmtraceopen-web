import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useMemo,
  useState,
} from "react";
import { Button, tokens } from "@fluentui/react-components";
import {
  apiBase,
  listDevices,
  listEntries,
  listFiles,
  listSessions,
  type ListEntriesOptions,
} from "../lib/api-client";
import type {
  DeviceSummary,
  LogEntry,
  LogEntryDto,
  SessionFile,
  SessionSummary,
} from "../lib/log-types";
import { EntryList } from "./EntryList";
import { FilesPanel } from "./FilesPanel";
import {
  ALL_SEVERITIES,
  FilterBar,
  applyFilters,
  collectComponents,
  defaultFilters,
  type Filters,
} from "./FilterBar";

type FetchState<T> =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "ok"; data: T }
  | { status: "error"; error: string };

/**
 * Imperative handle exposed to the parent (ViewerShell / Toolbar) so that
 * global chrome controls can drive ApiMode without lifting its selection /
 * fetch state out of the component.
 *
 * Note: ApiMode has no `openFile` action because navigation is entirely
 * click-driven through its stacked panels (Devices → Sessions → Files →
 * Entries) — there is no external "open" verb for the parent to invoke.
 * FileMode exposes an `openFile`; ApiMode intentionally does not.
 */
export interface ApiModeHandle {
  /** Refetch the currently-selected layer (re-queries the server).
   *  If a file is selected: refetch entries. Else if a session is
   *  selected: refetch files. Else if a device is selected: refetch
   *  sessions. Else: refetch the device list. */
  reload: () => void;
  /** Clear the selection stack (selectedFile -> selectedSession ->
   *  selectedDevice) back to the top. */
  clear: () => void;
}

export type ApiModeProps = Record<string, never>;

/**
 * API-mode view: four stacked panels that walk the hierarchy
 *
 *     devices → sessions → files → entries
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
 *
 * Theming: all colors route through `tokens.*` from @fluentui/react-components
 * so the panel picks up dark / high-contrast / classic palettes without any
 * per-theme branches.
 */
export const ApiMode = forwardRef<ApiModeHandle, ApiModeProps>(function ApiMode(
  _props,
  ref,
) {
  const [devices, setDevices] = useState<FetchState<DeviceSummary[]>>({ status: "idle" });
  const [selectedDevice, setSelectedDevice] = useState<string | null>(null);

  const [sessions, setSessions] = useState<FetchState<SessionSummary[]>>({ status: "idle" });
  const [selectedSession, setSelectedSession] = useState<string | null>(null);

  const [files, setFiles] = useState<FetchState<SessionFile[]>>({ status: "idle" });
  const [selectedFile, setSelectedFile] = useState<string | null>(null);

  const [entries, setEntries] = useState<FetchState<LogEntry[]>>({ status: "idle" });

  // Per-layer reload nonces: bumping one forces the matching effect below to
  // re-run (because its dep list includes the nonce) without duplicating the
  // fetch code. Each layer gets its own counter so `reload` only re-queries
  // the selected layer — not the whole stack.
  const [devicesReloadNonce, setDevicesReloadNonce] = useState(0);
  const [sessionsReloadNonce, setSessionsReloadNonce] = useState(0);
  const [filesReloadNonce, setFilesReloadNonce] = useState(0);
  const [entriesReloadNonce, setEntriesReloadNonce] = useState(0);

  // Filter state is owned here so we can both (a) push a subset to the
  // server and (b) finish the rest client-side before handing to EntryList.
  const [filters, setFilters] = useState<Filters>(() => defaultFilters());
  // Debounced mirrors of the text inputs — we only refetch when the user
  // stops typing for DEBOUNCE_MS. Everything else (severity chips, time
  // range, clear) fires an immediate refetch.
  const debouncedSearch = useDebounced(filters.search, DEBOUNCE_MS);
  const debouncedComponent = useDebounced(filters.component ?? "", DEBOUNCE_MS);

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
  }, [devicesReloadNonce]);

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
  }, [selectedDevice, sessionsReloadNonce]);

  // Panel 3: files for the selected session — auto-fetch as soon as the
  // session changes so the operator sees the list without an extra click.
  useEffect(() => {
    if (!selectedSession) {
      setFiles({ status: "idle" });
      return;
    }
    let cancelled = false;
    setFiles({ status: "loading" });
    listFiles(selectedSession)
      .then((p) => {
        if (cancelled) return;
        setFiles({ status: "ok", data: p.items });
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        setFiles({ status: "error", error: formatError(err) });
      });
    return () => {
      cancelled = true;
    };
  }, [selectedSession, filesReloadNonce]);

  // Panel 4: entries for the selected file, with server-side filter
  // pushdown for everything the API supports. Component filter + multi-
  // severity are handled client-side.
  useEffect(() => {
    if (!selectedSession || !selectedFile) {
      setEntries({ status: "idle" });
      return;
    }
    const controller = new AbortController();
    let cancelled = false;
    setEntries({ status: "loading" });
    listEntries(selectedSession, {
      ...buildServerOptions(filters, debouncedSearch),
      file: selectedFile,
      signal: controller.signal,
    })
      .then((p) => {
        if (cancelled) return;
        setEntries({ status: "ok", data: p.items.map(dtoToEntry) });
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        // Swallow AbortError — it only fires when we cancelled on purpose.
        if (err instanceof DOMException && err.name === "AbortError") return;
        setEntries({ status: "error", error: formatError(err) });
      });
    return () => {
      cancelled = true;
      controller.abort();
    };
    // `debouncedComponent` is intentionally NOT in this dep list: the server
    // doesn't accept a component param yet, so component changes refilter
    // client-side only (see `displayFilters` below).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    selectedSession,
    selectedFile,
    debouncedSearch,
    filters.afterMs,
    filters.beforeMs,
    // Only the "exactly one selected" case changes the server query — pack
    // the severity set into a stable key so we don't over-refetch.
    severityServerKey(filters.severities),
    entriesReloadNonce,
  ]);

  // Expose reload/clear to the parent. `reload` walks the selection stack
  // top-down and bumps the nonce for the deepest layer that is actually
  // populated, which re-triggers the matching useEffect above (which already
  // calls the right `listDevices` / `listSessions` / `listFiles` /
  // `listEntries` helper). We also reset that layer's FetchState out of the
  // "error" branch so an error-retry style reload starts from a clean
  // loading state rather than flashing the stale error.
  useImperativeHandle(
    ref,
    () => ({
      reload: () => {
        if (selectedFile) {
          setEntries((s) => (s.status === "error" ? { status: "idle" } : s));
          setEntriesReloadNonce((n) => n + 1);
        } else if (selectedSession) {
          setFiles((s) => (s.status === "error" ? { status: "idle" } : s));
          setFilesReloadNonce((n) => n + 1);
        } else if (selectedDevice) {
          setSessions((s) => (s.status === "error" ? { status: "idle" } : s));
          setSessionsReloadNonce((n) => n + 1);
        } else {
          setDevices((s) => (s.status === "error" ? { status: "idle" } : s));
          setDevicesReloadNonce((n) => n + 1);
        }
      },
      clear: () => {
        // Drop selections top-down. The cascading reset effects already null
        // out any dependent fetch state when a parent selection clears, so
        // nulling the deepest selection first keeps each effect's cleanup
        // path consistent with what a user click would produce.
        setSelectedFile(null);
        setSelectedSession(null);
        setSelectedDevice(null);
      },
    }),
    [selectedDevice, selectedSession, selectedFile],
  );

  const handleSelectDevice = useCallback((id: string) => {
    setSelectedDevice(id);
    setSelectedSession(null); // clear cascaded selection
    setSelectedFile(null);
    setFilters(defaultFilters()); // stale filters almost never translate
  }, []);

  const handleSelectSession = useCallback((id: string) => {
    setSelectedSession(id);
    setSelectedFile(null);
    setFilters(defaultFilters());
  }, []);

  const handleSelectFile = useCallback((id: string) => {
    setSelectedFile(id);
    setFilters(defaultFilters());
  }, []);

  return (
    <div
      style={{
        flex: 1,
        minHeight: 0,
        display: "grid",
        // Four stacked panels: devices (compact), sessions (compact),
        // files (compact), entries (fills remaining height so EntryList's
        // virtualizer has room to work).
        gridTemplateRows:
          "minmax(120px, 1fr) minmax(120px, 1fr) minmax(120px, 1fr) 3fr",
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
      <Panel title={selectedSession ? `Files — ${selectedSession}` : "Files"}>
        {selectedSession ? (
          <FilesPanel
            state={files}
            selected={selectedFile}
            onSelect={handleSelectFile}
          />
        ) : (
          <EmptyHint text="Select a session to list its files." />
        )}
      </Panel>
      <Panel title={selectedFile ? `Entries — ${selectedFile}` : "Entries"} flex>
        {selectedFile ? (
          <EntriesPanel
            state={entries}
            filters={filters}
            onFiltersChange={setFilters}
            debouncedComponent={debouncedComponent}
          />
        ) : (
          <EmptyHint text="Select a file to load up to 500 entries." />
        )}
      </Panel>
    </div>
  );
});

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
        border: `1px solid ${tokens.colorNeutralStroke1}`,
        borderRadius: tokens.borderRadiusMedium,
        overflow: "hidden",
        background: tokens.colorNeutralBackground1,
      }}
    >
      <header
        style={{
          display: "flex",
          alignItems: "center",
          gap: 8,
          padding: "6px 10px",
          background: tokens.colorNeutralBackground2,
          borderBottom: `1px solid ${tokens.colorNeutralStroke1}`,
          fontSize: 12,
          fontWeight: 600,
          color: tokens.colorNeutralForeground1,
        }}
      >
        <span>{title}</span>
        {baseHint && (
          <span
            style={{
              fontWeight: 400,
              color: tokens.colorNeutralForeground3,
              fontSize: 11,
            }}
          >
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

function EntriesPanel({
  state,
  filters,
  onFiltersChange,
  debouncedComponent,
}: {
  state: FetchState<LogEntry[]>;
  filters: Filters;
  onFiltersChange: (f: Filters) => void;
  /**
   * Debounced component filter — `filters.component` updates on every
   * keystroke (for controlled-input responsiveness), but the derived list
   * only refilters when the debounced mirror catches up.
   */
  debouncedComponent: string;
}) {
  // Build the effective client-side filter. The server has already handled
  // search / time-range / (single-)severity, so we only need to finish:
  //   - component substring (no server param yet)
  //   - multi-severity narrowing when the user has a subset active
  //   - search as a defence-in-depth in case the debounced server query
  //     lags the controlled input by a frame or two
  const clientFilters: Filters = useMemo(
    () => ({
      severities: filters.severities,
      search: filters.search,
      component: debouncedComponent || undefined,
      // afterMs/beforeMs already enforced server-side; leaving them here
      // is a belt-and-braces guard that costs ~1 compare per row.
      afterMs: filters.afterMs,
      beforeMs: filters.beforeMs,
    }),
    [
      filters.severities,
      filters.search,
      filters.afterMs,
      filters.beforeMs,
      debouncedComponent,
    ],
  );

  const serverEntries = state.status === "ok" ? state.data : EMPTY_ENTRIES;
  const knownComponents = useMemo(
    () => collectComponents(serverEntries),
    [serverEntries],
  );
  const shown = useMemo(
    () => applyFilters(serverEntries, clientFilters).length,
    [serverEntries, clientFilters],
  );

  const bar = (
    <FilterBar
      filters={filters}
      onChange={onFiltersChange}
      total={serverEntries.length}
      shown={shown}
      knownComponents={knownComponents}
    />
  );

  const body = (() => {
    if (state.status === "loading")
      return <CenteredText text="Loading entries…" muted />;
    if (state.status === "error") return <ApiError error={state.error} />;
    if (state.status === "idle") return null;
    if (state.data.length === 0)
      return <EmptyHint text="No entries match the current filters." />;
    return <EntryList entries={state.data} filters={clientFilters} />;
  })();

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        flex: 1,
        minHeight: 0,
        gap: 8,
        padding: 8,
      }}
    >
      {bar}
      <div style={{ flex: 1, minHeight: 0, display: "flex" }}>{body}</div>
    </div>
  );
}

const EMPTY_ENTRIES: LogEntry[] = [];

// ---------------------------------------------------------------------------
// Leaf UI bits

function RowButton({
  selected,
  onClick,
  children,
}: {
  selected: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) {
  // Fluent's Button with `appearance="subtle"` keeps full-row click targets
  // looking like list rows (no heavy chrome) while routing color/hover
  // states through the token palette.
  return (
    <Button
      appearance="subtle"
      size="small"
      onClick={onClick}
      aria-pressed={selected}
      style={{
        width: "100%",
        display: "flex",
        flexDirection: "column",
        alignItems: "flex-start",
        gap: 2,
        padding: "6px 10px",
        borderRadius: 0,
        borderTop: "none",
        borderLeft: "none",
        borderRight: "none",
        borderBottom: `1px solid ${tokens.colorNeutralStroke1}`,
        background: selected
          ? tokens.colorNeutralBackground1Selected
          : "transparent",
        color: tokens.colorNeutralForeground1,
        fontSize: 13,
        textAlign: "left",
        justifyContent: "flex-start",
        minWidth: 0,
      }}
    >
      {children}
    </Button>
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
        background: tokens.colorNeutralBackground2,
        border: `1px solid ${tokens.colorNeutralStroke1}`,
        color: tokens.colorPaletteRedForeground1,
        borderRadius: tokens.borderRadiusMedium,
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
    <div
      style={{
        padding: 12,
        color: tokens.colorNeutralForeground3,
        fontSize: 13,
      }}
    >
      {text}
    </div>
  );
}

function CenteredText({ text, muted }: { text: string; muted?: boolean }) {
  return (
    <div
      style={{
        padding: 14,
        color: muted
          ? tokens.colorNeutralForeground3
          : tokens.colorNeutralForeground1,
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
  color: tokens.colorNeutralForeground2,
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

const DEBOUNCE_MS = 200;

/**
 * `setTimeout`-based debounce — no new dependency, just the standard
 * "update after the user stops typing" pattern. Returns the last value
 * passed in that held steady for `ms` milliseconds.
 */
function useDebounced<T>(value: T, ms: number): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const id = setTimeout(() => setDebounced(value), ms);
    return () => clearTimeout(id);
  }, [value, ms]);
  return debounced;
}

/**
 * Stable string key derived from the severity set that only changes when
 * the "exactly one selected" state — the only case we push to the server
 * — changes. Prevents re-fetches when the user toggles a third chip on
 * and off.
 */
function severityServerKey(set: Set<LogEntry["severity"]>): string {
  if (set.size !== 1) return "multi";
  return [...set][0] ?? "none";
}

/**
 * Translate the client filter state into server query params. Only the
 * subset the api-server understands is emitted:
 *
 *   - `severity` — sent only when exactly one tier is active (server
 *     treats it as a floor, not equality, so multi-select can't be
 *     faithfully represented and is handled client-side instead).
 *   - `after_ts` / `before_ts` — passed through as-is.
 *   - `q` — the debounced search string.
 *
 * Component filter is omitted: the server has no component param yet,
 * so it's applied purely client-side in `EntriesPanel`.
 */
function buildServerOptions(
  filters: Filters,
  debouncedSearch: string,
): Omit<ListEntriesOptions, "signal" | "file"> {
  const opts: Omit<ListEntriesOptions, "signal" | "file"> = { limit: 500 };
  if (filters.severities.size === 1) {
    // Safe: size === 1 guarantees exactly one element.
    opts.severity = [...filters.severities][0]!;
  } else if (filters.severities.size === ALL_SEVERITIES.length) {
    // No pushdown when all three are active — default server behaviour.
  } else {
    // 2 out of 3 selected → no single server param can express "both but
    // not the third"; fetch everything and filter client-side.
  }
  if (filters.afterMs != null) opts.afterMs = filters.afterMs;
  if (filters.beforeMs != null) opts.beforeMs = filters.beforeMs;
  const q = debouncedSearch.trim();
  if (q !== "") opts.q = q;
  return opts;
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
