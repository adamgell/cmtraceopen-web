// LogViewer — right-pane content. Reads the currently-selected session+file
// from bridge state, fetches entries via listEntries(), maps the wire DTOs
// into LogEntry shape via dtoToEntry, then renders:
//   FileCrumb ─ FilterBar ─ EntryGrid ─ StatusBar
// Owns the client-side filter set + derived `visible` list. Filters reset
// on file change so the user always sees a full view of a newly picked file.
//
// totalEntriesInFile is currently approximated by `mapped.length` — the
// server doesn't echo the full file entry count in the listEntries response.
// A future task can thread `file.entryCount` from the middle pane for a
// tighter number; the UI tolerates the approximation gracefully.

import { useEffect, useMemo, useState } from "react";
import { listEntries } from "../../lib/api-client";
import { dtoToEntry } from "../../lib/dto-to-entry";
import type { LogEntry } from "../../lib/log-types";
import { useBridgeState } from "../../lib/bridge-state";
import { theme } from "../../lib/theme";
import { EntryGrid } from "./EntryGrid";
import { FilterBar, type Filters } from "./FilterBar";
import { RowDetail } from "./RowDetail";
import { StatusBar } from "./StatusBar";

const DEFAULT_FILTERS: Filters = {
  info: true,
  warn: true,
  error: true,
  search: "",
  component: "",
};

export function LogViewer() {
  const { state } = useBridgeState();
  const [entries, setEntries] = useState<LogEntry[]>([]);
  const [totalEntriesInFile, setTotalEntriesInFile] = useState(0);
  const [filters, setFilters] = useState<Filters>(DEFAULT_FILTERS);
  const [status, setStatus] = useState<"idle" | "loading" | "ok" | "error">("idle");
  const [error, setError] = useState<string | null>(null);
  const [detailEntry, setDetailEntry] = useState<LogEntry | null>(null);

  useEffect(() => {
    setFilters(DEFAULT_FILTERS);
    setDetailEntry(null);
    if (!state.selectedSessionId || !state.selectedFileId) {
      setEntries([]);
      setStatus("idle");
      setTotalEntriesInFile(0);
      return;
    }
    let cancelled = false;
    setStatus("loading");
    setError(null);
    (async () => {
      try {
        const page = await listEntries(state.selectedSessionId!, {
          file: state.selectedFileId!,
          limit: 500,
        });
        if (!cancelled) {
          const mapped = page.items.map(dtoToEntry);
          setEntries(mapped);
          // Server doesn't return the full file entry count — use rendered
          // length as a lower bound. Task can tighten this by threading
          // file.entryCount from the middle pane later.
          setTotalEntriesInFile(mapped.length);
          setStatus("ok");
        }
      } catch (err) {
        if (!cancelled) {
          setError(err instanceof Error ? err.message : String(err));
          setStatus("error");
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [state.selectedSessionId, state.selectedFileId]);

  const visible = useMemo(() => {
    const q = filters.search.toLowerCase();
    const comp = filters.component.toLowerCase();
    return entries.filter((e) => {
      if (e.severity === "Info" && !filters.info) return false;
      if (e.severity === "Warning" && !filters.warn) return false;
      if (e.severity === "Error" && !filters.error) return false;
      if (q && !e.message.toLowerCase().includes(q)) return false;
      if (comp && !(e.component ?? "").toLowerCase().includes(comp)) return false;
      return true;
    });
  }, [entries, filters]);

  const warnCount = visible.filter((e) => e.severity === "Warning").length;
  const errCount = visible.filter((e) => e.severity === "Error").length;

  if (status === "idle") {
    return (
      <div
        style={{
          padding: "1rem",
          color: theme.textDim,
          fontFamily: theme.font.mono,
          fontSize: "0.7rem",
        }}
      >
        Pick a file in the middle pane to load entries.
      </div>
    );
  }
  if (status === "error") {
    return (
      <div
        style={{
          padding: "0.7rem",
          color: theme.pill.failed.fg,
          fontFamily: theme.font.mono,
          fontSize: "0.7rem",
        }}
      >
        entries unreachable: {error}
      </div>
    );
  }
  return (
    <div
      style={{
        position: "relative",
        display: "grid",
        gridTemplateRows: "auto auto 1fr auto",
        height: "100%",
        minHeight: 0,
      }}
    >
      <FileCrumb />
      <FilterBar
        filters={filters}
        totals={{ rendered: visible.length, total: totalEntriesInFile }}
        onChange={setFilters}
      />
      <EntryGrid entries={visible} onOpenRow={setDetailEntry} />
      <StatusBar
        rendered={visible.length}
        limit={500}
        total={totalEntriesInFile}
        warnCount={warnCount}
        errCount={errCount}
      />
      <RowDetail entry={detailEntry} onClose={() => setDetailEntry(null)} />
    </div>
  );
}

function FileCrumb() {
  const { state } = useBridgeState();
  return (
    <div
      style={{
        padding: "0.3rem 0.7rem",
        borderBottom: `1px solid ${theme.border}`,
        fontFamily: theme.font.mono,
        fontSize: "0.66rem",
        color: theme.textDim,
      }}
    >
      <span style={{ color: theme.accent }}>{state.selectedDeviceId ?? "—"}</span>
      {" / "}
      <span>{state.selectedSessionId?.slice(0, 8) ?? "—"}</span>
      {" / "}
      <span style={{ color: theme.accent }}>{state.selectedFileId?.slice(0, 12) ?? "—"}</span>
    </div>
  );
}
