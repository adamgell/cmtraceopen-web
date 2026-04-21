import { useMemo } from "react";
import type { Severity } from "../lib/log-types";

/**
 * Shape of the viewer filter state.
 *
 * Lives in the shell / mode component and is threaded down to `EntryList`
 * (client-side filtering) and, in API mode, to the query-building code
 * that maps a subset of these to server-side query params.
 *
 * - `severities`: multi-select set. Empty set = show nothing; "all three"
 *   = default. We keep this as a Set so membership checks are O(1) and
 *   toggling is a single add/delete.
 * - `search`: case-insensitive substring over `message`. Empty = no filter.
 * - `component`: optional substring over `component` (case-insensitive).
 * - `afterMs` / `beforeMs`: inclusive lower / exclusive upper bound on
 *   entry timestamp in epoch ms. Entries with no timestamp are dropped
 *   when either bound is active (they can't be placed on the timeline).
 */
export type Filters = {
  severities: Set<Severity>;
  search: string;
  component?: string;
  afterMs?: number;
  beforeMs?: number;
};

export const ALL_SEVERITIES: Severity[] = ["Info", "Warning", "Error"];

/** Default state: everything passes. */
export function defaultFilters(): Filters {
  return {
    severities: new Set<Severity>(ALL_SEVERITIES),
    search: "",
    component: undefined,
    afterMs: undefined,
    beforeMs: undefined,
  };
}

/**
 * True when the filter state is equivalent to "show everything" — used by
 * callers to skip the filtering pass entirely on large datasets.
 */
export function isEmptyFilters(f: Filters): boolean {
  return (
    f.severities.size === ALL_SEVERITIES.length &&
    f.search.trim() === "" &&
    !f.component &&
    f.afterMs == null &&
    f.beforeMs == null
  );
}

// ---------------------------------------------------------------------------
// Presentation

const SEVERITY_CHIP_STYLE: Record<
  Severity,
  { on: React.CSSProperties; off: React.CSSProperties; label: string }
> = {
  Info: {
    on: { background: "#6b7280", color: "white", borderColor: "#6b7280" },
    off: { background: "white", color: "#6b7280", borderColor: "#d1d5db" },
    label: "Info",
  },
  Warning: {
    on: { background: "#d97706", color: "white", borderColor: "#d97706" },
    off: { background: "white", color: "#b45309", borderColor: "#fcd34d" },
    label: "Warning",
  },
  Error: {
    on: { background: "#dc2626", color: "white", borderColor: "#dc2626" },
    off: { background: "white", color: "#b91c1c", borderColor: "#fca5a5" },
    label: "Error",
  },
};

export interface FilterBarProps {
  filters: Filters;
  onChange: (next: Filters) => void;
  /**
   * Total entries the filters are applied against and how many survive.
   * Rendered as "Showing N of M entries." — `shown === total` suppresses
   * the "of M" portion when no filters are active.
   */
  total: number;
  shown: number;
  /**
   * Optional list of known components in the current dataset — used to
   * populate a `<datalist>` for typeahead. Omit in API mode where the
   * full component set isn't pre-enumerated.
   */
  knownComponents?: string[];
  /** When true, render the time-range inputs (hidden in compact layouts). */
  showTimeRange?: boolean;
}

/**
 * Compact filter bar: severity chips, search box, optional component
 * typeahead, optional time range, clear-all, and a result count. Styling
 * is inline — no external CSS — to match the rest of the viewer.
 */
export function FilterBar({
  filters,
  onChange,
  total,
  shown,
  knownComponents,
  showTimeRange = true,
}: FilterBarProps) {
  const datalistId = "cmtrace-component-list";

  const toggleSeverity = (s: Severity) => {
    const next = new Set(filters.severities);
    if (next.has(s)) next.delete(s);
    else next.add(s);
    onChange({ ...filters, severities: next });
  };

  const setSearch = (v: string) => onChange({ ...filters, search: v });
  const setComponent = (v: string) =>
    onChange({ ...filters, component: v === "" ? undefined : v });
  const setAfter = (v: string) =>
    onChange({ ...filters, afterMs: localToMs(v) });
  const setBefore = (v: string) =>
    onChange({ ...filters, beforeMs: localToMs(v) });

  const clearAll = () => onChange(defaultFilters());

  const componentOptions = useMemo(() => {
    if (!knownComponents || knownComponents.length === 0) return null;
    // Cap the list to keep the DOM light — a 50k-entry log can easily hit
    // thousands of distinct components and there's no UX value in dumping
    // them all into a single datalist.
    const capped = knownComponents.slice(0, 500);
    return (
      <datalist id={datalistId}>
        {capped.map((c) => (
          <option key={c} value={c} />
        ))}
      </datalist>
    );
  }, [knownComponents]);

  return (
    <div
      style={{
        display: "flex",
        flexWrap: "wrap",
        alignItems: "center",
        gap: 8,
        padding: "8px 10px",
        border: "1px solid #e5e5e5",
        borderRadius: 4,
        background: "#fafafa",
        fontSize: 12,
      }}
    >
      {/* Severity chips */}
      <div style={{ display: "flex", gap: 4 }} role="group" aria-label="Severity">
        {ALL_SEVERITIES.map((s) => {
          const on = filters.severities.has(s);
          const spec = SEVERITY_CHIP_STYLE[s];
          const style: React.CSSProperties = {
            padding: "3px 10px",
            fontSize: 12,
            border: "1px solid",
            borderRadius: 999,
            cursor: "pointer",
            fontWeight: 500,
            ...(on ? spec.on : spec.off),
          };
          return (
            <button
              key={s}
              type="button"
              aria-pressed={on}
              onClick={() => toggleSeverity(s)}
              style={style}
            >
              {spec.label}
            </button>
          );
        })}
      </div>

      {/* Search */}
      <input
        type="search"
        value={filters.search}
        onChange={(e) => setSearch(e.target.value)}
        placeholder="Search messages…"
        aria-label="Search messages"
        style={{
          flex: "1 1 180px",
          minWidth: 140,
          padding: "4px 8px",
          fontSize: 12,
          border: "1px solid #ccc",
          borderRadius: 4,
        }}
      />

      {/* Component typeahead (optional) */}
      <input
        type="text"
        value={filters.component ?? ""}
        onChange={(e) => setComponent(e.target.value)}
        placeholder="Component…"
        aria-label="Component filter"
        list={componentOptions ? datalistId : undefined}
        style={{
          flex: "0 1 140px",
          minWidth: 100,
          padding: "4px 8px",
          fontSize: 12,
          border: "1px solid #ccc",
          borderRadius: 4,
        }}
      />
      {componentOptions}

      {/* Time range (optional) */}
      {showTimeRange && (
        <div style={{ display: "flex", alignItems: "center", gap: 4 }}>
          <input
            type="datetime-local"
            value={msToLocal(filters.afterMs)}
            onChange={(e) => setAfter(e.target.value)}
            aria-label="After"
            title="After (inclusive)"
            style={{
              padding: "3px 6px",
              fontSize: 12,
              border: "1px solid #ccc",
              borderRadius: 4,
            }}
          />
          <span style={{ color: "#888" }}>→</span>
          <input
            type="datetime-local"
            value={msToLocal(filters.beforeMs)}
            onChange={(e) => setBefore(e.target.value)}
            aria-label="Before"
            title="Before (exclusive)"
            style={{
              padding: "3px 6px",
              fontSize: 12,
              border: "1px solid #ccc",
              borderRadius: 4,
            }}
          />
        </div>
      )}

      {/* Clear all */}
      <button
        type="button"
        onClick={clearAll}
        disabled={isEmptyFilters(filters)}
        style={{
          padding: "3px 10px",
          fontSize: 12,
          border: "1px solid #ccc",
          background: "white",
          borderRadius: 4,
          cursor: isEmptyFilters(filters) ? "default" : "pointer",
          color: isEmptyFilters(filters) ? "#bbb" : "#333",
        }}
      >
        Clear
      </button>

      {/* Result count */}
      <div style={{ color: "#555", marginLeft: "auto", whiteSpace: "nowrap" }}>
        {shown === total
          ? `${total.toLocaleString()} entries`
          : `Showing ${shown.toLocaleString()} of ${total.toLocaleString()} entries`}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Client-side filter application (shared by Local + API modes).

/**
 * Apply `filters` to a list of entries and return the survivors. Callers
 * should memoize the result — this walks the full list.
 *
 * Empty filters short-circuit to the input array reference, so `useMemo`
 * consumers in `EntryList` won't thrash when the user hasn't narrowed.
 */
export function applyFilters<
  E extends {
    message: string;
    severity: Severity;
    component?: string;
    timestamp?: number;
  },
>(entries: E[], filters: Filters): E[] {
  if (isEmptyFilters(filters)) return entries;
  const needle = filters.search.trim().toLowerCase();
  const compNeedle = filters.component?.trim().toLowerCase();
  const { severities, afterMs, beforeMs } = filters;
  const hasTimeBound = afterMs != null || beforeMs != null;

  const out: E[] = [];
  for (let i = 0; i < entries.length; i++) {
    const e = entries[i]!;
    if (!severities.has(e.severity)) continue;
    if (needle && !e.message.toLowerCase().includes(needle)) continue;
    if (compNeedle) {
      if (!e.component) continue;
      if (!e.component.toLowerCase().includes(compNeedle)) continue;
    }
    if (hasTimeBound) {
      const ts = e.timestamp;
      if (ts == null) continue;
      if (afterMs != null && ts < afterMs) continue;
      if (beforeMs != null && ts >= beforeMs) continue;
    }
    out.push(e);
  }
  return out;
}

/**
 * Extract the distinct, sorted set of components in a dataset. Used to
 * populate the FilterBar typeahead in local mode.
 */
export function collectComponents(
  entries: ReadonlyArray<{ component?: string | null }>,
): string[] {
  const set = new Set<string>();
  for (const e of entries) {
    if (e.component) set.add(e.component);
  }
  return [...set].sort((a, b) => a.localeCompare(b));
}

// ---------------------------------------------------------------------------
// datetime-local <-> epoch-ms helpers.
//
// `<input type="datetime-local">` emits strings like "2024-01-15T14:30"
// interpreted in the user's local tz. We convert to epoch ms on write and
// back on read. An empty string clears the filter.

function localToMs(v: string): number | undefined {
  if (!v) return undefined;
  const ms = new Date(v).getTime();
  return Number.isFinite(ms) ? ms : undefined;
}

function msToLocal(ms: number | undefined): string {
  if (ms == null) return "";
  const d = new Date(ms);
  if (!Number.isFinite(d.getTime())) return "";
  // Pad each component to 2 digits and trim to minute precision — matches
  // the format the input control emits, so round-trips don't drift.
  const pad = (n: number) => String(n).padStart(2, "0");
  return (
    `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}` +
    `T${pad(d.getHours())}:${pad(d.getMinutes())}`
  );
}
