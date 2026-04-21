import { useCallback, useState } from "react";
import type { ParseResult } from "../lib/log-types";
import { LocalMode } from "./LocalMode";

interface LoadedSummary {
  fileName: string;
  result: ParseResult;
}

/**
 * Top-level viewer shell. Renders the app chrome (title bar + file
 * summary) and delegates the actual drop/parse/list flow to `LocalMode`.
 *
 * Kept intentionally thin so a sibling fetch-from-API flow can plug in
 * next to `LocalMode` without the shell owning parse state.
 */
export function ViewerShell() {
  const [loaded, setLoaded] = useState<LoadedSummary | null>(null);

  const handleLoaded = useCallback((info: LoadedSummary | null) => {
    setLoaded(info);
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
      <TopBar loaded={loaded} onClose={() => setLoaded(null)} />
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
        <LocalMode onLoaded={handleLoaded} />
      </main>
    </div>
  );
}

function TopBar({
  loaded,
  onClose,
}: {
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
        </>
      )}
    </header>
  );
}
