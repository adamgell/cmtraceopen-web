import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { initWasm, parse } from "../lib/wasm-bridge";
import type { ParseResult } from "../lib/log-types";
import { DropZone } from "./DropZone";
import { EntryList } from "./EntryList";
import {
  FilterBar,
  applyFilters,
  collectComponents,
  defaultFilters,
  type Filters,
} from "./FilterBar";

type State =
  | { tag: "init" }
  | { tag: "idle" }
  | { tag: "loading"; fileName: string }
  | { tag: "loaded"; fileName: string; result: ParseResult }
  | { tag: "error"; message: string };

export interface LocalModeProps {
  /**
   * Called whenever the loaded state changes so the shell's top bar can
   * render the file/entry summary. Passing `null` clears the summary.
   */
  onLoaded?: (info: { fileName: string; result: ParseResult } | null) => void;
}

/**
 * The original drag-drop → WASM parse → virtualized list flow, extracted
 * from ViewerShell so the shell can route between this and ApiMode.
 *
 * Owns its own state machine (init → idle → loading → loaded | error).
 * WASM is initialized once on mount; if the user toggles to API mode and
 * back, this component unmounts and remounts — that's fine, `initWasm()`
 * is idempotent.
 */
export function LocalMode({ onLoaded }: LocalModeProps) {
  const [state, setState] = useState<State>({ tag: "init" });
  // Filters are owned here (not in EntryList) so the bar can render its
  // result count against the same derived list the virtualizer sees.
  const [filters, setFilters] = useState<Filters>(() => defaultFilters());
  // Monotonic request counter used to discard stale parse results when a user
  // drops a new file before the previous parse resolves.
  const parseReqId = useRef(0);

  useEffect(() => {
    let cancelled = false;
    void (async () => {
      try {
        await initWasm();
        if (!cancelled) setState({ tag: "idle" });
      } catch (err) {
        if (!cancelled) {
          setState({ tag: "error", message: `WASM init failed: ${formatError(err)}` });
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  // Notify the shell about the currently loaded file so it can render the
  // summary row. Reset to null on any non-loaded state.
  useEffect(() => {
    if (!onLoaded) return;
    if (state.tag === "loaded") {
      onLoaded({ fileName: state.fileName, result: state.result });
    } else {
      onLoaded(null);
    }
  }, [state, onLoaded]);

  const handleFile = useCallback(async (file: File) => {
    const reqId = ++parseReqId.current;
    setState({ tag: "loading", fileName: file.name });
    try {
      const text = await file.text();
      const result = await parse(text, file.name, file.size);
      if (parseReqId.current !== reqId) return;
      // Reset filters on a fresh file — the "Networking" component from
      // the last log almost certainly doesn't exist in the new one.
      setFilters(defaultFilters());
      setState({ tag: "loaded", fileName: file.name, result });
    } catch (err) {
      if (parseReqId.current !== reqId) return;
      setState({ tag: "error", message: `Failed to parse ${file.name}: ${formatError(err)}` });
    }
  }, []);

  const handleDismiss = useCallback(() => {
    setState({ tag: "idle" });
  }, []);

  // Precompute the component set + filtered length only when loaded. We
  // compute `shown` here (not inside EntryList) so the FilterBar's count
  // reflects exactly what the virtualizer renders.
  const loadedEntries =
    state.tag === "loaded" ? state.result.entries : null;
  const knownComponents = useMemo(
    () => (loadedEntries ? collectComponents(loadedEntries) : []),
    [loadedEntries],
  );
  const shown = useMemo(
    () => (loadedEntries ? applyFilters(loadedEntries, filters).length : 0),
    [loadedEntries, filters],
  );

  return (
    <>
      {state.tag === "error" && (
        <ErrorBanner message={state.message} onDismiss={handleDismiss} />
      )}
      {state.tag === "init" && (
        <CenteredMessage text="Initializing WASM module…" />
      )}
      {(state.tag === "idle" || state.tag === "loading") && (
        <div style={{ maxWidth: 640, margin: "48px auto 0", width: "100%" }}>
          <DropZone onFile={handleFile} disabled={state.tag === "loading"} />
          {state.tag === "loading" && (
            <div
              style={{
                marginTop: 16,
                textAlign: "center",
                color: "#666",
                fontSize: 14,
              }}
            >
              Parsing {state.fileName}…
            </div>
          )}
        </div>
      )}
      {state.tag === "loaded" && (
        <>
          <FilterBar
            filters={filters}
            onChange={setFilters}
            total={state.result.entries.length}
            shown={shown}
            knownComponents={knownComponents}
          />
          <EntryList entries={state.result.entries} filters={filters} />
        </>
      )}
    </>
  );
}

function ErrorBanner({ message, onDismiss }: { message: string; onDismiss: () => void }) {
  return (
    <div
      style={{
        display: "flex",
        alignItems: "flex-start",
        gap: 12,
        padding: "10px 14px",
        background: "#fef2f2",
        border: "1px solid #fecaca",
        color: "#991b1b",
        borderRadius: 4,
        whiteSpace: "pre-wrap",
      }}
    >
      <div style={{ flex: 1, fontSize: 13 }}>{message}</div>
      <button
        type="button"
        onClick={onDismiss}
        style={{
          padding: "4px 10px",
          fontSize: 12,
          border: "1px solid #b91c1c",
          background: "white",
          color: "#b91c1c",
          borderRadius: 4,
          cursor: "pointer",
        }}
      >
        Dismiss
      </button>
    </div>
  );
}

function CenteredMessage({ text }: { text: string }) {
  return (
    <div
      style={{
        flex: 1,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        color: "#666",
      }}
    >
      {text}
    </div>
  );
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
