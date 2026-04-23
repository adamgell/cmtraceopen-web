import type { ReactNode } from "react";
import { tokens } from "@fluentui/react-components";
import { LOG_UI_FONT_FAMILY } from "../lib/log-accessibility";

/**
 * Connection / data-source state for the optional left-edge badge.
 *
 * - `idle`: no source attached — we render nothing.
 * - `loading`: actively parsing (local) or fetching (api).
 * - `connected`: data ready, no errors.
 * - `error`: terminal failure — the badge turns red.
 */
export type StatusBarConnectionState =
  | "idle"
  | "loading"
  | "connected"
  | "error";

/**
 * Props for {@link StatusBar}.
 *
 * This is intentionally a superset of what both ApiMode and LocalMode
 * will ever need — callers pass what they have and omit the rest. Every
 * field is optional so the same component can render a sparse skeleton
 * (e.g. "no file loaded yet") or a fully-populated status line.
 *
 * Mapping intent:
 * - LocalMode knows the file name, total entries / lines, parse errors,
 *   detected format, and (once WASM returns) can derive a selected row.
 * - ApiMode will typically know session + file name, filtered count, the
 *   selected row, and may add freeform `badges` for things like page /
 *   tail state that don't belong in LocalMode.
 */
export interface StatusBarProps {
  /** Currently loaded file name (local mode) or session/file combo (api mode). */
  sourceLabel?: string;
  /** Total entries in the dataset. */
  totalEntries?: number;
  /** Entries surviving the active filters. Omit when no filter is active. */
  filteredEntries?: number;
  /** Lines scanned from the raw source (local mode). */
  totalLines?: number;
  /** Parse errors. 0 or undefined hides the segment. */
  parseErrors?: number;
  /** Selected row index (0-based). Paired with filteredEntries/totalEntries for "row N of M". */
  selectedIndex?: number;
  /** Format detected by the parser ("CmtLog", "Iis", "Panther", …). */
  formatDetected?: string;
  /**
   * Short human-readable description of the active filter(s), e.g.
   * "Errors only", "search: installer". Empty / undefined = no filter.
   */
  activeFilterSummary?: string;
  /** Connection / source state for the left-edge dot badge. */
  connectionState?: StatusBarConnectionState;
  /**
   * Freeform content rendered at the far right, after the primary
   * right-side metrics. Useful for mode-specific badges (pagination
   * state, tailing indicator, auth identity, …).
   */
  badges?: ReactNode;
}

// ---------------------------------------------------------------------------
// Formatting helpers. Kept local to the file — they're trivial and the
// component is small enough that a separate util module would be overkill.

function formatCount(n: number): string {
  return n.toLocaleString();
}

function connectionLabel(state: StatusBarConnectionState): string {
  switch (state) {
    case "connected":
      return "Connected";
    case "loading":
      return "Loading";
    case "error":
      return "Error";
    case "idle":
    default:
      return "Idle";
  }
}

function connectionColor(state: StatusBarConnectionState): string {
  switch (state) {
    case "connected":
      return tokens.colorPaletteGreenForeground1;
    case "loading":
      return tokens.colorNeutralForeground3;
    case "error":
      return tokens.colorPaletteRedForeground1;
    case "idle":
    default:
      return tokens.colorNeutralForeground3;
  }
}

// ---------------------------------------------------------------------------

/**
 * Fixed-height status strip rendered at the bottom of the viewer shell.
 *
 * Visual contract (mirrors the desktop app's StatusBar):
 * - Left side: source label, detected format, entry / filtered counts,
 *   lines, parse-error count. Segments are joined with a middle-dot.
 * - Right side: current row / total, active filter summary, freeform
 *   caller-supplied `badges`.
 * - Parse errors render in the palette's red foreground so they read as
 *   an alert against the neutral chrome.
 *
 * The component is pure presentation — no store access, no effects. All
 * data flows in via props so both ApiMode and LocalMode can drive it
 * without sharing state machinery.
 */
export function StatusBar({
  sourceLabel,
  totalEntries,
  filteredEntries,
  totalLines,
  parseErrors,
  selectedIndex,
  formatDetected,
  activeFilterSummary,
  connectionState,
  badges,
}: StatusBarProps) {
  // --- Left segments --------------------------------------------------------
  // Built as an array and joined with a middle-dot so every segment is
  // optional without littering the JSX with conditional separators.
  const leftSegments: ReactNode[] = [];

  if (sourceLabel) {
    leftSegments.push(
      <span key="source" title={sourceLabel}>
        {sourceLabel}
      </span>,
    );
  }

  if (formatDetected) {
    leftSegments.push(<span key="format">{formatDetected}</span>);
  }

  if (typeof totalEntries === "number") {
    // Show "X of Y entries" when a filter is narrowing the view, else
    // just the raw count. Matches FilterBar's phrasing for consistency.
    const entriesText =
      typeof filteredEntries === "number" && filteredEntries !== totalEntries
        ? `${formatCount(filteredEntries)} of ${formatCount(totalEntries)} entries`
        : `${formatCount(totalEntries)} entries`;
    leftSegments.push(<span key="entries">{entriesText}</span>);
  }

  if (typeof totalLines === "number") {
    leftSegments.push(
      <span key="lines">{`${formatCount(totalLines)} lines`}</span>,
    );
  }

  if (typeof parseErrors === "number" && parseErrors > 0) {
    leftSegments.push(
      <span
        key="parse-errors"
        style={{ color: tokens.colorPaletteRedForeground1, fontWeight: 600 }}
        title={`${parseErrors} parse error${parseErrors === 1 ? "" : "s"}`}
      >
        {`${formatCount(parseErrors)} parse error${parseErrors === 1 ? "" : "s"}`}
      </span>,
    );
  }

  // --- Right segments -------------------------------------------------------
  const rightSegments: ReactNode[] = [];

  // "Row N of M" — prefers filteredEntries (what the user actually sees)
  // but falls back to totalEntries when no filter is active.
  if (typeof selectedIndex === "number" && selectedIndex >= 0) {
    const denom =
      typeof filteredEntries === "number"
        ? filteredEntries
        : typeof totalEntries === "number"
          ? totalEntries
          : undefined;
    const rowText =
      typeof denom === "number"
        ? `Row ${formatCount(selectedIndex + 1)} of ${formatCount(denom)}`
        : `Row ${formatCount(selectedIndex + 1)}`;
    rightSegments.push(<span key="row">{rowText}</span>);
  }

  if (activeFilterSummary && activeFilterSummary.trim().length > 0) {
    rightSegments.push(
      <span
        key="filter"
        title={activeFilterSummary}
        style={{
          maxWidth: 240,
          overflow: "hidden",
          textOverflow: "ellipsis",
          whiteSpace: "nowrap",
        }}
      >
        {`Filter: ${activeFilterSummary}`}
      </span>,
    );
  }

  // --- Render ---------------------------------------------------------------
  return (
    <div
      role="status"
      aria-live="polite"
      style={{
        display: "flex",
        alignItems: "center",
        justifyContent: "space-between",
        gap: 10,
        height: 28,
        flexShrink: 0,
        padding: "0 10px",
        backgroundColor: tokens.colorNeutralBackground3,
        borderTop: `1px solid ${tokens.colorNeutralStroke1}`,
        color: tokens.colorNeutralForeground2,
        fontFamily: LOG_UI_FONT_FAMILY,
        fontSize: tokens.fontSizeBase100,
        lineHeight: 1,
        overflow: "hidden",
      }}
    >
      {/* Left cluster: connection dot + dot-separated source metrics. */}
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 8,
          minWidth: 0,
          overflow: "hidden",
          whiteSpace: "nowrap",
        }}
      >
        {connectionState && connectionState !== "idle" && (
          <span
            title={connectionLabel(connectionState)}
            aria-label={`Connection state: ${connectionLabel(connectionState)}`}
            style={{
              display: "inline-flex",
              alignItems: "center",
              gap: 4,
              color: connectionColor(connectionState),
              flexShrink: 0,
            }}
          >
            <span
              style={{
                width: 6,
                height: 6,
                borderRadius: "50%",
                backgroundColor: "currentColor",
                display: "inline-block",
              }}
            />
            <span>{connectionLabel(connectionState)}</span>
          </span>
        )}
        {leftSegments.length > 0 && (
          <span
            style={{
              minWidth: 0,
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
            }}
          >
            {leftSegments.map((seg, i) => (
              <span key={i}>
                {i > 0 && (
                  <span
                    aria-hidden
                    style={{
                      margin: "0 6px",
                      color: tokens.colorNeutralForeground4,
                    }}
                  >
                    ·
                  </span>
                )}
                {seg}
              </span>
            ))}
          </span>
        )}
      </div>

      {/* Right cluster: row position, filter summary, caller badges. */}
      <div
        style={{
          display: "flex",
          alignItems: "center",
          gap: 10,
          flexShrink: 0,
          whiteSpace: "nowrap",
        }}
      >
        {rightSegments.map((seg, i) => (
          <span key={i} style={{ display: "inline-flex", alignItems: "center" }}>
            {i > 0 && (
              <span
                aria-hidden
                style={{
                  marginRight: 10,
                  color: tokens.colorNeutralForeground4,
                }}
              >
                |
              </span>
            )}
            {seg}
          </span>
        ))}
        {badges && (
          <span style={{ display: "inline-flex", alignItems: "center", gap: 6 }}>
            {badges}
          </span>
        )}
      </div>
    </div>
  );
}
