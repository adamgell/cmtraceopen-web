import { useCallback, useState } from "react";
import type { ParseResult } from "../lib/log-types";
import { LocalMode } from "./LocalMode";
import { ApiMode } from "./ApiMode";
import { DevicesPanel } from "./DevicesPanel";
import { AuthSettings } from "./AuthSettings";

type Mode = "local" | "api" | "devices";

interface LoadedSummary {
  fileName: string;
  result: ParseResult;
}

/**
 * Top-level viewer shell.
 *
 * Since the `api-fetch` feature landed, the shell is little more than a
 * chrome (title bar, mode toggle) that routes to one of two children:
 *
 *   - `LocalMode` — the original drag-and-drop + WASM parse flow.
 *   - `ApiMode`   — fetch device → session → entries from the api-server.
 *
 * The shell itself no longer tracks parse state; each mode owns its own
 * lifecycle. The only cross-cutting concern the shell still handles is
 * the "loaded file" summary in the top bar, and even that is just a prop
 * callback the local flow writes into — API mode doesn't use it.
 */
export function ViewerShell() {
  const [mode, setMode] = useState<Mode>("local");
  const [loaded, setLoaded] = useState<LoadedSummary | null>(null);

  const handleLoaded = useCallback((info: LoadedSummary | null) => {
    setLoaded(info);
  }, []);

  // Clear any lingering local-mode summary when switching away so the top
  // bar doesn't advertise a file that's no longer on screen.
  const handleModeChange = useCallback((next: Mode) => {
    setMode(next);
    if (next !== "local") setLoaded(null);
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
      <TopBar
        mode={mode}
        onModeChange={handleModeChange}
        loaded={mode === "local" ? loaded : null}
        onClose={() => setLoaded(null)}
      />
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
        {mode === "local" ? <LocalMode onLoaded={handleLoaded} /> : mode === "devices" ? <DevicesPanel /> : <ApiMode />}
      </main>
    </div>
  );
}

function TopBar({
  mode,
  onModeChange,
  loaded,
  onClose,
}: {
  mode: Mode;
  onModeChange: (m: Mode) => void;
  loaded: LoadedSummary | null;
  onClose: () => void;
}) {
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
      <ModeToggle mode={mode} onChange={onModeChange} />
      {loaded && (
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
      )}
      <div style={{ flex: 1 }} />
      <AuthSettings />
      {loaded && (
        <button
          type="button"
          onClick={onClose}
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
      )}
    </header>
  );
}

function ModeToggle({
  mode,
  onChange,
}: {
  mode: Mode;
  onChange: (m: Mode) => void;
}) {
  const base: React.CSSProperties = {
    padding: "4px 10px",
    fontSize: 12,
    border: "1px solid #ccc",
    background: "white",
    cursor: "pointer",
  };
  const active: React.CSSProperties = {
    ...base,
    background: "#111",
    borderColor: "#111",
    color: "white",
  };
  return (
    <div
      role="tablist"
      aria-label="Viewer mode"
      style={{ display: "inline-flex", borderRadius: 4, overflow: "hidden" }}
    >
      <button
        type="button"
        role="tab"
        aria-selected={mode === "local"}
        onClick={() => onChange("local")}
        style={mode === "local" ? active : { ...base, borderRight: "none" }}
      >
        Local
      </button>
      <button
        type="button"
        role="tab"
        aria-selected={mode === "api"}
        onClick={() => onChange("api")}
        style={mode === "api" ? active : { ...base, borderRight: "none" }}
      >
        API
      </button>
      <button
        type="button"
        role="tab"
        aria-selected={mode === "devices"}
        onClick={() => onChange("devices")}
        style={mode === "devices" ? active : base}
      >
        Devices
      </button>
    </div>
  );
}
