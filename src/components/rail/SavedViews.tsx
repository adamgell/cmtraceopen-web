// Saved views section, rendered under the device list when the rail is
// expanded. Entries are ★-prefixed KQL queries persisted in localStorage
// under `cmtrace.saved-views` as a JSON array of `{ name, query }`.
//
// Clicking a row invokes the `onRun(query)` callback — in Task 7 this is
// wired to `dispatch({ type: "set-fleet-query", query })`; Task 13's KqlBar
// will write entries here (and we tolerate cross-tab updates via the
// `storage` event). The read path is defensive: corrupt JSON, non-arrays
// and wrong-shape entries are all discarded silently so one bad key can't
// sink the rail.
//
// `readSavedViews` / `writeSavedViews` are exported so Task 13 can reuse
// them when it adds a "★ SAVE" button on the KQL bar.
import { useEffect, useState } from "react";
import { theme } from "../../lib/theme";

export interface SavedView {
  name: string;
  query: string;
}

const STORAGE_KEY = "cmtrace.saved-views";

export function readSavedViews(): SavedView[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    if (!Array.isArray(parsed)) return [];
    return parsed.filter(
      (v): v is SavedView =>
        typeof v === "object" &&
        v != null &&
        typeof v.name === "string" &&
        typeof v.query === "string"
    );
  } catch {
    return [];
  }
}

export function writeSavedViews(views: SavedView[]): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(views));
  } catch {
    // No-op on storage failure (private mode, quota exceeded, etc).
  }
}

interface Props {
  expanded: boolean;
  onRun: (query: string) => void;
}

export function SavedViews({ expanded, onRun }: Props) {
  const [views, setViews] = useState<SavedView[]>([]);

  useEffect(() => {
    setViews(readSavedViews());
    // Listen for storage events so opening another tab and saving there
    // updates this rail without a manual reload.
    function onStorage(e: StorageEvent) {
      if (e.key === STORAGE_KEY) setViews(readSavedViews());
    }
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, []);

  if (views.length === 0 || !expanded) return null;

  return (
    <div>
      <div
        style={{
          padding: "0.3rem 0.7rem",
          fontFamily: theme.font.mono,
          fontSize: "0.58rem",
          color: theme.textDim,
          textTransform: "uppercase",
          letterSpacing: "0.1em",
          borderTop: `1px solid ${theme.border}`,
          borderBottom: `1px solid ${theme.border}`,
          marginTop: "0.5rem",
        }}
      >
        SAVED VIEWS · {views.length}
      </div>
      {views.map((v) => (
        <button
          key={v.name}
          type="button"
          onClick={() => onRun(v.query)}
          title={v.query}
          style={{
            all: "unset",
            display: "block",
            width: "100%",
            padding: "0.4rem 0.7rem",
            color: theme.accent,
            fontFamily: theme.font.mono,
            fontSize: "0.7rem",
            cursor: "pointer",
            borderBottom: `1px solid ${theme.surface}`,
          }}
        >
          ★ {v.name}
        </button>
      ))}
    </div>
  );
}
