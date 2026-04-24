// LogViewer — right-pane content. Reads the currently-selected
// session+file from bridge state, fetches entries via listEntries(), maps the
// wire DTOs into LogEntry shape via dtoToEntry, and renders them through
// EntryGrid. Idle/loading/error copy mirrors the rest of the shell.

import { useEffect, useState } from "react";
import { listEntries } from "../../lib/api-client";
import { dtoToEntry } from "../../lib/dto-to-entry";
import type { LogEntry } from "../../lib/log-types";
import { useBridgeState } from "../../lib/bridge-state";
import { theme } from "../../lib/theme";
import { EntryGrid } from "./EntryGrid";

export function LogViewer() {
  const { state } = useBridgeState();
  const [entries, setEntries] = useState<LogEntry[]>([]);
  const [status, setStatus] = useState<"idle" | "loading" | "ok" | "error">("idle");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!state.selectedSessionId || !state.selectedFileId) {
      setEntries([]);
      setStatus("idle");
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
          setEntries(page.items.map(dtoToEntry));
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
  return <EntryGrid entries={entries} />;
}
