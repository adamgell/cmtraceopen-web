// Monospace REPL bar at the top of the command-bridge shell.
//
// Features:
// - Controlled input with a token-coloured overlay (pointer-events:none so the
//   input still receives focus/selection). Overlay and input share the exact
//   monospace font + font-size, so the overlay aligns char-for-char.
// - Recent queries persisted to localStorage (cmtrace.recent-queries, capped at
//   MAX_RECENT) + Saved views read from Task 7's SavedViews module.
// - Dropdown surfaces recent + saved entries when the input is focused. The
//   dropdown rows use `onMouseDown + preventDefault` so they fire BEFORE the
//   input's blur timeout hides the dropdown (clicks on mousedown, not click).
// - Run button and Enter key both trigger onRun. Escape blurs. Save button
//   prompts for a name and writes through Task 7's writeSavedViews.
// - Running is a no-op for blank / whitespace-only input.
// - id="kql-input" is required by Task 16's ⌘/ focus shortcut — don't drop it.

import { useRef, useMemo, useState, type KeyboardEvent } from "react";
import { useBridgeState } from "../../lib/bridge-state";
import { suggest, type Suggestion } from "../../lib/kql-autocomplete";
import { tokenize, type Token } from "../../lib/kql-lexer";
import { theme } from "../../lib/theme";
import { readSavedViews, writeSavedViews } from "../rail/SavedViews";

const RECENT_KEY = "cmtrace.recent-queries";
const MAX_RECENT = 10;

function readRecent(): string[] {
  try {
    const raw = localStorage.getItem(RECENT_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    return Array.isArray(parsed) ? parsed.filter((v) => typeof v === "string") : [];
  } catch {
    return [];
  }
}

function writeRecent(queries: string[]) {
  try {
    localStorage.setItem(RECENT_KEY, JSON.stringify(queries.slice(0, MAX_RECENT)));
  } catch {
    // noop — private mode, quota exceeded, etc.
  }
}

const TOKEN_COLORS: Record<Token["kind"], string> = {
  table: theme.pill.okFallbacks.fg, // amber
  pipe: theme.accent,
  keyword: theme.syntax.keyword, // purple
  field: theme.textDim,
  operator: theme.accent,
  string: theme.pill.partial.fg, // orange
  function: theme.accent,
  number: theme.pill.okFallbacks.fg, // amber
  ident: theme.text,
  whitespace: theme.text,
};

interface Props {
  onRun: (query: string) => void;
}

export function KqlBar({ onRun }: Props) {
  const { state } = useBridgeState();
  const [query, setQuery] = useState(state.fleetQuery);
  const [focused, setFocused] = useState(false);
  const [cursor, setCursor] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);
  const tokens = useMemo(() => tokenize(query), [query]);
  const suggestions = useMemo(() => suggest(query, cursor), [query, cursor]);

  function runNow() {
    if (!query.trim()) return;
    const recent = readRecent();
    writeRecent([query, ...recent.filter((q) => q !== query)]);
    onRun(query);
  }

  function onKey(e: KeyboardEvent<HTMLInputElement>) {
    if (e.key === "Enter") runNow();
    if (e.key === "Escape") (e.target as HTMLInputElement).blur();
  }

  function saveCurrent() {
    if (!query.trim()) return;
    const name = window.prompt("Name this view", query.slice(0, 40)) ?? "";
    if (!name.trim()) return;
    const existing = readSavedViews();
    writeSavedViews([
      { name: name.trim(), query },
      ...existing.filter((v) => v.name !== name.trim()),
    ]);
  }

  return (
    <div
      style={{
        background: theme.bgDeep,
        borderBottom: `1px solid ${theme.border}`,
        padding: "0.55rem 0.75rem",
        display: "flex",
        gap: "0.6rem",
        alignItems: "center",
      }}
    >
      <span
        style={{
          color: theme.accent,
          fontFamily: theme.font.mono,
          fontSize: "0.78rem",
          letterSpacing: "0.08em",
        }}
      >
        ›_
      </span>
      <div style={{ flex: 1, position: "relative" }}>
        <input
          ref={inputRef}
          id="kql-input"
          value={query}
          onChange={(e) => {
            setQuery(e.target.value);
            setCursor(e.target.selectionStart ?? e.target.value.length);
          }}
          onSelect={(e) => setCursor((e.target as HTMLInputElement).selectionStart ?? cursor)}
          onFocus={() => setFocused(true)}
          onBlur={() => setTimeout(() => setFocused(false), 150)}
          onKeyDown={onKey}
          placeholder='DeviceLog | where parse_state == "failed" | where ingested_utc > ago(24h)'
          style={{
            width: "100%",
            background: theme.surface,
            border: `1px solid ${theme.border}`,
            borderRadius: 4,
            padding: "0.4rem 0.6rem",
            fontFamily: theme.font.mono,
            fontSize: "0.78rem",
            color: theme.textPrimary,
            caretColor: theme.accent,
          }}
        />
        <HighlightOverlay tokens={tokens} />
        {focused && (
          <Dropdown
            suggestions={suggestions}
            onPick={(q) => {
              setQuery(q);
              onRun(q);
            }}
            onAccept={(s) => {
              const before = query.slice(0, cursor);
              const after = query.slice(cursor);
              const tokens = tokenize(before);
              const meaningful = tokens.filter((t) => t.kind !== "whitespace");
              const last = meaningful[meaningful.length - 1];
              const replaceFrom = last && last.kind !== "pipe" && last.kind !== "operator"
                ? last.start : cursor;
              const next = query.slice(0, replaceFrom) + s.insert + " " + after.trimStart();
              setQuery(next);
              const newCursor = replaceFrom + s.insert.length + 1;
              setCursor(newCursor);
              setTimeout(() => {
                inputRef.current?.focus();
                inputRef.current?.setSelectionRange(newCursor, newCursor);
              }, 0);
            }}
          />
        )}
      </div>
      <button
        type="button"
        onClick={runNow}
        aria-label="Run query"
        title="Run query (⏎)"
        style={{
          background: theme.accent,
          border: `1px solid ${theme.accent}`,
          color: theme.bgDeep,
          padding: "0.4rem 0.95rem",
          borderRadius: 4,
          fontFamily: theme.font.mono,
          fontSize: "0.72rem",
          fontWeight: 700,
          letterSpacing: "0.08em",
          cursor: "pointer",
          display: "inline-flex",
          alignItems: "center",
          gap: "0.4rem",
        }}
      >
        <span aria-hidden="true" style={{ fontSize: "0.6rem" }}>▶</span>
        RUN
      </button>
      <button
        type="button"
        onClick={saveCurrent}
        style={{
          background: theme.surface,
          border: `1px solid ${theme.border}`,
          color: theme.textDim,
          padding: "0.35rem 0.6rem",
          borderRadius: 4,
          fontFamily: theme.font.mono,
          fontSize: "0.65rem",
          cursor: "pointer",
        }}
      >
        ★ SAVE
      </button>
    </div>
  );
}

function HighlightOverlay({ tokens }: { tokens: Token[] }) {
  // Non-interactive overlay rendered on top of the input for visual-only
  // coloring. Since the input uses the same monospace font + font-size, the
  // overlay aligns char-for-char. pointer-events:none so clicks reach the input.
  return (
    <div
      aria-hidden
      style={{
        position: "absolute",
        inset: 0,
        padding: "0.4rem 0.6rem",
        fontFamily: theme.font.mono,
        fontSize: "0.78rem",
        pointerEvents: "none",
        whiteSpace: "pre",
        color: "transparent",
      }}
    >
      {tokens.map((t, i) => (
        <span key={i} style={{ color: TOKEN_COLORS[t.kind] }}>
          {t.text}
        </span>
      ))}
    </div>
  );
}

const SUGGESTION_KIND_COLORS: Record<Suggestion["kind"], string> = {
  table: theme.pill.okFallbacks.fg,
  keyword: theme.syntax.keyword,
  field: theme.textDim,
  operator: theme.accent,
  function: theme.accent,
  value: theme.pill.partial.fg,
};

function Dropdown({
  suggestions,
  onPick,
  onAccept,
}: {
  suggestions: Suggestion[];
  onPick: (q: string) => void;
  onAccept: (s: Suggestion) => void;
}) {
  const recent = readRecent();
  const saved = readSavedViews();
  if (suggestions.length === 0 && recent.length === 0 && saved.length === 0) return null;
  return (
    <div
      style={{
        position: "absolute",
        left: 0,
        right: 0,
        top: "100%",
        background: theme.bg,
        border: `1px solid ${theme.border}`,
        borderTop: "none",
        borderRadius: "0 0 4px 4px",
        padding: "0.3rem 0",
        fontFamily: theme.font.mono,
        fontSize: "0.68rem",
        zIndex: 10,
        maxHeight: "260px",
        overflow: "auto",
      }}
    >
      {suggestions.length > 0 && (
        <>
          <div
            style={{
              padding: "0.2rem 0.75rem",
              color: theme.textDim,
              fontSize: "0.55rem",
              letterSpacing: "0.1em",
              textTransform: "uppercase",
            }}
          >
            Schema
          </div>
          {suggestions.map((s) => (
            <button
              key={`${s.kind}-${s.label}`}
              type="button"
              onMouseDown={(e) => {
                e.preventDefault();
                onAccept(s);
              }}
              style={{
                all: "unset",
                display: "flex",
                width: "100%",
                padding: "0.25rem 0.75rem",
                gap: "0.6rem",
                alignItems: "center",
                color: theme.text,
                cursor: "pointer",
              }}
            >
              <span style={{ color: SUGGESTION_KIND_COLORS[s.kind], minWidth: "3.5rem", fontSize: "0.55rem", textTransform: "uppercase" }}>
                {s.kind}
              </span>
              <span>{s.label}</span>
            </button>
          ))}
        </>
      )}
      {recent.length > 0 && (
        <>
          <div
            style={{
              padding: "0.2rem 0.75rem",
              color: theme.textDim,
              fontSize: "0.55rem",
              letterSpacing: "0.1em",
              textTransform: "uppercase",
              marginTop: suggestions.length > 0 ? "0.3rem" : 0,
            }}
          >
            Recent
          </div>
          {recent.slice(0, 5).map((q) => (
            <button
              key={q}
              type="button"
              onMouseDown={(e) => {
                e.preventDefault();
                onPick(q);
              }}
              style={{
                all: "unset",
                display: "block",
                width: "100%",
                padding: "0.25rem 0.75rem",
                color: theme.text,
                cursor: "pointer",
              }}
            >
              {q}
            </button>
          ))}
        </>
      )}
      {saved.length > 0 && (
        <>
          <div
            style={{
              padding: "0.2rem 0.75rem",
              color: theme.textDim,
              fontSize: "0.55rem",
              letterSpacing: "0.1em",
              textTransform: "uppercase",
              marginTop: "0.3rem",
            }}
          >
            Saved views
          </div>
          {saved.slice(0, 5).map((v) => (
            <button
              key={v.name}
              type="button"
              onMouseDown={(e) => {
                e.preventDefault();
                onPick(v.query);
              }}
              style={{
                all: "unset",
                display: "block",
                width: "100%",
                padding: "0.25rem 0.75rem",
                color: theme.accent,
                cursor: "pointer",
              }}
            >
              ★ {v.name}
            </button>
          ))}
        </>
      )}
    </div>
  );
}
