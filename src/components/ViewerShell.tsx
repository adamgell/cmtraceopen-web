import { useCallback, useEffect, useRef, useState } from "react";
import { initWasm, parse } from "../lib/wasm-bridge";
import type { ParseResult } from "../lib/log-types";
import { DropZone } from "./DropZone";
import { EntryList } from "./EntryList";

type State =
  | { tag: "init" }
  | { tag: "idle" }
  | { tag: "loading"; fileName: string }
  | { tag: "loaded"; fileName: string; result: ParseResult }
  | { tag: "error"; message: string };

/**
 * Top-level state machine for the v0 viewer: init → idle → loading →
 * loaded | error. WASM is initialized once on mount; file loads
 * read the file as text and dispatch to the parser.
 */
export function ViewerShell() {
  const [state, setState] = useState<State>({ tag: "init" });
  // Monotonic request counter used to discard stale parse results when a user
  // drops a new file before the previous parse resolves. Each handleFile call
  // increments this and captures its own id; on resolution we only apply the
  // setState if the captured id is still the latest.
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

  const handleFile = useCallback(async (file: File) => {
    const reqId = ++parseReqId.current;
    setState({ tag: "loading", fileName: file.name });
    try {
      const text = await file.text();
      const result = await parse(text, file.name, file.size);
      // Guard against a stale parse: a newer handleFile call has superseded us.
      if (parseReqId.current !== reqId) return;
      setState({ tag: "loaded", fileName: file.name, result });
    } catch (err) {
      if (parseReqId.current !== reqId) return;
      setState({ tag: "error", message: `Failed to parse ${file.name}: ${formatError(err)}` });
    }
  }, []);

  const handleReset = useCallback(() => {
    setState({ tag: "idle" });
  }, []);

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        height: "100vh",
        fontFamily: "system-ui, -apple-system, Segoe UI, sans-serif",
        color: "#222",
      }}
    >
      <TopBar state={state} onReset={handleReset} />
      <main
        style={{
          flex: 1,
          minHeight: 0,
          display: "flex",
          flexDirection: "column",
          padding: 16,
          gap: 12,
        }}
      >
        {state.tag === "error" && (
          <ErrorBanner message={state.message} onDismiss={handleReset} />
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
        {state.tag === "loaded" && <EntryList entries={state.result.entries} />}
      </main>
    </div>
  );
}

function TopBar({ state, onReset }: { state: State; onReset: () => void }) {
  const loaded = state.tag === "loaded" ? state : null;
  return (
    <header
      style={{
        display: "flex",
        alignItems: "center",
        gap: 16,
        padding: "10px 16px",
        borderBottom: "1px solid #e5e5e5",
        background: "#fafafa",
      }}
    >
      <div style={{ fontWeight: 600 }}>CMTrace Open — Web</div>
      {loaded && (
        <>
          <div style={{ color: "#555", fontSize: 13 }}>
            <span style={{ fontWeight: 500 }}>{loaded.fileName}</span>
            <span style={{ color: "#888", marginLeft: 12 }}>
              {loaded.result.entries.length.toLocaleString()} entries
              {" · "}
              {loaded.result.totalLines.toLocaleString()} lines
              {" · "}
              <span
                style={{
                  color: loaded.result.parseErrors > 0 ? "#b91c1c" : "#888",
                }}
              >
                {loaded.result.parseErrors} parse errors
              </span>
              {" · "}
              format: {String(loaded.result.formatDetected)}
            </span>
          </div>
          <div style={{ flex: 1 }} />
          <button
            type="button"
            onClick={onReset}
            style={{
              padding: "6px 12px",
              fontSize: 13,
              border: "1px solid #ccc",
              background: "white",
              borderRadius: 4,
              cursor: "pointer",
            }}
          >
            Close file
          </button>
        </>
      )}
    </header>
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
