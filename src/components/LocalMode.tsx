import {
  forwardRef,
  useCallback,
  useEffect,
  useImperativeHandle,
  useMemo,
  useRef,
  useState,
} from "react";
import { Button, Tooltip, tokens } from "@fluentui/react-components";
import { initWasm, parse } from "../lib/wasm-bridge";
import type { ParseResult } from "../lib/log-types";
import { useWorkspace } from "../lib/workspace-context";
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
 * Imperative handle exposed to the parent (ViewerShell) so toolbar
 * buttons can drive LocalMode without LocalMode having to know about
 * the toolbar. Each method is safe to call in any state.
 */
export interface LocalModeHandle {
  /** Open the OS file picker. Resolves when the user picks or cancels. */
  openFile: () => void;
  /** Re-parse the last loaded file. No-op when nothing is loaded. */
  reload: () => void;
  /** Drop the loaded state and go back to the initial idle screen. */
  clear: () => void;
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
export const LocalMode = forwardRef<LocalModeHandle, LocalModeProps>(
  function LocalMode({ onLoaded }, ref) {
    const [state, setState] = useState<State>({ tag: "init" });
    // Filters are owned here (not in EntryList) so the bar can render its
    // result count against the same derived list the virtualizer sees.
    const [filters, setFilters] = useState<Filters>(() => defaultFilters());
    // Most recently loaded file, kept so `reload` can re-parse it without
    // bothering the user to pick the file again.
    const [lastFile, setLastFile] = useState<File | null>(null);
    const workspace = useWorkspace();
    // Monotonic request counter used to discard stale parse results when a user
    // drops a new file before the previous parse resolves.
    const parseReqId = useRef(0);
    // Hidden file input used by the imperative `openFile` handle. DropZone
    // has its own internal input for the "Choose file…" button, but it
    // doesn't expose a ref to it, so LocalMode carries its own.
    const fileInputRef = useRef<HTMLInputElement>(null);

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
      setLastFile(file);
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

    // Imperative actions wired to ViewerShell's toolbar.
    const openFile = useCallback(() => {
      fileInputRef.current?.click();
    }, []);

    const reload = useCallback(() => {
      if (!lastFile) return;
      // Clear any error banner before kicking off the re-parse; if the
      // reload fails, handleFile will set a fresh error.
      void handleFile(lastFile);
    }, [lastFile, handleFile]);

    const clear = useCallback(() => {
      // Bump the parse counter so any in-flight parse is discarded.
      parseReqId.current++;
      setLastFile(null);
      setFilters(defaultFilters());
      setState({ tag: "idle" });
      // onLoaded(null) is fired by the state-sync effect above, but call
      // it explicitly too so the parent's StatusBar clears immediately
      // even in the edge case where state was already non-loaded.
      onLoaded?.(null);
    }, [onLoaded]);

    useImperativeHandle(
      ref,
      () => ({ openFile, reload, clear }),
      [openFile, reload, clear],
    );

    const handleHiddenInputChange = useCallback(
      (e: React.ChangeEvent<HTMLInputElement>) => {
        const file = e.target.files?.[0];
        if (file) void handleFile(file);
        // Reset so picking the same file twice in a row re-fires the event.
        e.target.value = "";
      },
      [handleFile],
    );

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
        {/* Hidden input driven by the imperative `openFile` handle. */}
        <input
          ref={fileInputRef}
          type="file"
          onChange={handleHiddenInputChange}
          style={{ display: "none" }}
        />
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
                  color: tokens.colorNeutralForeground2,
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
            <div
              style={{
                display: "flex",
                alignItems: "center",
                gap: 8,
                justifyContent: "flex-end",
              }}
            >
              {(() => {
                const candidate = {
                  kind: "local-file" as const,
                  label: state.fileName,
                  fileName: state.fileName,
                };
                const existing = workspace.findExisting(candidate);
                return (
                  <Tooltip
                    content={existing ? "Already pinned" : "Pin to workspace"}
                    relationship="label"
                    withArrow
                  >
                    <Button
                      size="small"
                      appearance="subtle"
                      disabled={!!existing}
                      onClick={() => {
                        if (!existing) workspace.pin(candidate);
                      }}
                    >
                      {existing ? "Pinned" : "Pin file"}
                    </Button>
                  </Tooltip>
                );
              })()}
            </div>
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
  },
);

function ErrorBanner({ message, onDismiss }: { message: string; onDismiss: () => void }) {
  return (
    <div
      style={{
        display: "flex",
        alignItems: "flex-start",
        gap: 12,
        padding: "10px 14px",
        background: tokens.colorPaletteRedBackground1,
        border: `1px solid ${tokens.colorPaletteRedBorder1}`,
        color: tokens.colorPaletteRedForeground1,
        borderRadius: tokens.borderRadiusMedium,
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
          border: `1px solid ${tokens.colorPaletteRedBorderActive}`,
          background: tokens.colorNeutralBackground1,
          color: tokens.colorPaletteRedForeground1,
          borderRadius: tokens.borderRadiusMedium,
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
        color: tokens.colorNeutralForeground2,
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
