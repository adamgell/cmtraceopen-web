import {
  useEffect,
  useRef,
  type CSSProperties,
  type KeyboardEvent as ReactKeyboardEvent,
} from "react";
import { Button, Input, Tooltip, tokens } from "@fluentui/react-components";

// ---------------------------------------------------------------------------
// Inline SVG icons.
//
// The desktop FindBar pulls these from `@fluentui/react-icons`, which is not
// a dependency of the web viewer (and the task says no new deps). These are
// size-24 viewBox glyphs rendered at 16px inline, stroke-less fill so they
// inherit the button's current color (`tokens.colorNeutralForeground*`).
// Shapes chosen to stay close to the Fluent `*Regular` set used by desktop.
// ---------------------------------------------------------------------------

const ICON_SIZE = 16;

function SearchIcon() {
  return (
    <svg
      width={ICON_SIZE}
      height={ICON_SIZE}
      viewBox="0 0 24 24"
      fill="none"
      aria-hidden="true"
    >
      <path
        d="M10 3a7 7 0 1 1-4.6 12.3l-3.1 3.1a1 1 0 0 1-1.4-1.4l3.1-3.1A7 7 0 0 1 10 3zm0 2a5 5 0 1 0 0 10 5 5 0 0 0 0-10z"
        fill="currentColor"
      />
    </svg>
  );
}

function ArrowUpIcon() {
  return (
    <svg
      width={ICON_SIZE}
      height={ICON_SIZE}
      viewBox="0 0 24 24"
      fill="none"
      aria-hidden="true"
    >
      <path
        d="M12 4a1 1 0 0 1 .7.3l6 6a1 1 0 1 1-1.4 1.4L13 7.4V19a1 1 0 1 1-2 0V7.4l-4.3 4.3a1 1 0 1 1-1.4-1.4l6-6A1 1 0 0 1 12 4z"
        fill="currentColor"
      />
    </svg>
  );
}

function ArrowDownIcon() {
  return (
    <svg
      width={ICON_SIZE}
      height={ICON_SIZE}
      viewBox="0 0 24 24"
      fill="none"
      aria-hidden="true"
    >
      <path
        d="M12 20a1 1 0 0 1-.7-.3l-6-6a1 1 0 1 1 1.4-1.4l4.3 4.3V5a1 1 0 1 1 2 0v11.6l4.3-4.3a1 1 0 1 1 1.4 1.4l-6 6A1 1 0 0 1 12 20z"
        fill="currentColor"
      />
    </svg>
  );
}

function DismissIcon() {
  return (
    <svg
      width={ICON_SIZE}
      height={ICON_SIZE}
      viewBox="0 0 24 24"
      fill="none"
      aria-hidden="true"
    >
      <path
        d="M4.3 4.3a1 1 0 0 1 1.4 0L12 10.6l6.3-6.3a1 1 0 1 1 1.4 1.4L13.4 12l6.3 6.3a1 1 0 0 1-1.4 1.4L12 13.4l-6.3 6.3a1 1 0 0 1-1.4-1.4L10.6 12 4.3 5.7a1 1 0 0 1 0-1.4z"
        fill="currentColor"
      />
    </svg>
  );
}

// ---------------------------------------------------------------------------
// Public API.
// ---------------------------------------------------------------------------

export interface FindBarProps {
  /** Controlled open state. Parent toggles with Ctrl/Cmd+F. */
  open: boolean;
  /** Fired when the X button or Esc closes the bar. */
  onClose: () => void;
  /** Current needle; parent stores it so it persists across open/close. */
  value: string;
  onChange: (next: string) => void;
  /** Optional — filter flags the bar surfaces. Omit the setter to hide the toggle. */
  matchCase?: boolean;
  onMatchCaseChange?: (v: boolean) => void;
  wholeWord?: boolean;
  onWholeWordChange?: (v: boolean) => void;
  regex?: boolean;
  onRegexChange?: (v: boolean) => void;
  /** Current match index (0-based) and total. Both undefined if no search. */
  currentMatch?: number;
  totalMatches?: number;
  /**
   * Optional error message for regex-mode failures (e.g. "Invalid regex").
   * Replaces the counter text and is rendered in the danger foreground color.
   */
  errorText?: string;
  /** Advance / retreat. Parent handles scrolling the list. */
  onNext: () => void;
  onPrev: () => void;
}

/**
 * Floating find bar modeled after the desktop viewer's FindBar.
 *
 * Purely presentational: the component owns nothing beyond the input ref and
 * focus-on-open behavior. All state (needle, flags, match position) is lifted
 * to the parent so the bar can be closed without losing search context — the
 * desktop version does the same via a Zustand store; here it's props.
 *
 * When `open` is false the component returns `null` so the parent can keep it
 * mounted unconditionally without reserving layout.
 */
export function FindBar({
  open,
  onClose,
  value,
  onChange,
  matchCase,
  onMatchCaseChange,
  wholeWord,
  onWholeWordChange,
  regex,
  onRegexChange,
  currentMatch,
  totalMatches,
  errorText,
  onNext,
  onPrev,
}: FindBarProps) {
  const inputRef = useRef<HTMLInputElement>(null);

  // Focus + select on open so hitting Ctrl+F again with text already present
  // lets the user immediately retype. Mirrors desktop behavior.
  useEffect(() => {
    if (!open) return;
    const el = inputRef.current;
    if (!el) return;
    el.focus();
    el.select();
  }, [open]);

  if (!open) return null;

  const hasQuery = value.trim().length > 0;
  const total = totalMatches ?? 0;
  const hasMatches = hasQuery && total > 0;

  // Status text: either an error, "No results", or "N of M". Empty when the
  // needle itself is empty — avoids flashing "No results" before the user types.
  let statusText = "";
  let statusIsError = false;
  if (hasQuery && errorText) {
    statusText = errorText;
    statusIsError = true;
  } else if (hasQuery && total === 0) {
    statusText = "No results";
    statusIsError = true;
  } else if (hasQuery && total > 0 && currentMatch != null) {
    statusText = `${currentMatch + 1} of ${total}`;
  }

  const handleKeyDown = (event: ReactKeyboardEvent<HTMLInputElement>) => {
    if (event.key === "Escape") {
      event.preventDefault();
      onClose();
      return;
    }
    if (event.key === "Enter" || event.key === "F3") {
      event.preventDefault();
      event.stopPropagation();
      if (event.shiftKey) onPrev();
      else onNext();
    }
  };

  // Square toggle buttons that swap to brand bg when the flag is on. Kept as
  // inline styles rather than makeStyles because the rest of this codebase
  // (see FilterBar) uses the same inline-token pattern.
  const toggleButtonStyle = (active: boolean): CSSProperties => ({
    minWidth: 28,
    width: 28,
    height: 28,
    padding: 0,
    borderRadius: tokens.borderRadiusSmall,
    backgroundColor: active ? tokens.colorBrandBackground : "transparent",
    color: active
      ? tokens.colorNeutralForegroundOnBrand
      : tokens.colorNeutralForeground2,
    border: active
      ? `1px solid ${tokens.colorBrandBackground}`
      : `1px solid ${tokens.colorNeutralStroke1}`,
  });

  const navButtonStyle: CSSProperties = {
    minWidth: 28,
    width: 28,
    height: 28,
    padding: 0,
  };

  return (
    <div
      role="search"
      aria-label="Find in log"
      style={{
        display: "flex",
        alignItems: "center",
        gap: 4,
        padding: "4px 8px",
        backgroundColor: tokens.colorNeutralBackground2,
        border: `1px solid ${tokens.colorNeutralStroke2}`,
        borderRadius: tokens.borderRadiusMedium,
        minHeight: 36,
        flexShrink: 0,
      }}
    >
      <Input
        ref={inputRef}
        value={value}
        onChange={(_, data) => onChange(data.value)}
        onKeyDown={handleKeyDown}
        placeholder="Find…"
        size="small"
        aria-label="Find"
        contentBefore={
          <span
            aria-hidden="true"
            style={{
              display: "inline-flex",
              alignItems: "center",
              color: tokens.colorNeutralForeground3,
            }}
          >
            <SearchIcon />
          </span>
        }
        contentAfter={
          statusText ? (
            <span
              style={{
                fontSize: 11,
                color: statusIsError
                  ? tokens.colorPaletteRedForeground1
                  : tokens.colorNeutralForeground3,
                whiteSpace: "nowrap",
                paddingRight: 4,
              }}
            >
              {statusText}
            </span>
          ) : undefined
        }
        style={{ minWidth: 220, maxWidth: 320, flex: 1 }}
      />

      {onMatchCaseChange ? (
        <Tooltip content="Match case" relationship="label">
          <Button
            appearance="subtle"
            size="small"
            style={toggleButtonStyle(!!matchCase)}
            onClick={() => onMatchCaseChange(!matchCase)}
            aria-label="Match case"
            aria-pressed={!!matchCase}
          >
            <span
              style={{
                fontSize: 12,
                fontWeight: 600,
                fontFamily: tokens.fontFamilyBase,
              }}
            >
              Aa
            </span>
          </Button>
        </Tooltip>
      ) : null}

      {onWholeWordChange ? (
        <Tooltip content="Whole word" relationship="label">
          <Button
            appearance="subtle"
            size="small"
            style={toggleButtonStyle(!!wholeWord)}
            onClick={() => onWholeWordChange(!wholeWord)}
            aria-label="Whole word"
            aria-pressed={!!wholeWord}
          >
            <span
              style={{
                fontSize: 12,
                fontWeight: 600,
                fontFamily: tokens.fontFamilyBase,
                textDecoration: "underline",
              }}
            >
              ab
            </span>
          </Button>
        </Tooltip>
      ) : null}

      {onRegexChange ? (
        <Tooltip content="Use regular expression" relationship="label">
          <Button
            appearance="subtle"
            size="small"
            style={toggleButtonStyle(!!regex)}
            onClick={() => onRegexChange(!regex)}
            aria-label="Use regular expression"
            aria-pressed={!!regex}
          >
            <span
              style={{
                fontSize: 13,
                fontFamily: tokens.fontFamilyMonospace,
                fontWeight: 600,
              }}
            >
              .*
            </span>
          </Button>
        </Tooltip>
      ) : null}

      <div
        aria-hidden="true"
        style={{
          width: 1,
          height: 20,
          backgroundColor: tokens.colorNeutralStroke2,
          margin: "0 2px",
        }}
      />

      <Tooltip content="Previous match (Shift+Enter)" relationship="label">
        <Button
          appearance="subtle"
          size="small"
          icon={<ArrowUpIcon />}
          disabled={!hasMatches}
          onClick={onPrev}
          aria-label="Previous match"
          style={navButtonStyle}
        />
      </Tooltip>

      <Tooltip content="Next match (Enter)" relationship="label">
        <Button
          appearance="subtle"
          size="small"
          icon={<ArrowDownIcon />}
          disabled={!hasMatches}
          onClick={onNext}
          aria-label="Next match"
          style={navButtonStyle}
        />
      </Tooltip>

      <Tooltip content="Close (Esc)" relationship="label">
        <Button
          appearance="subtle"
          size="small"
          icon={<DismissIcon />}
          onClick={onClose}
          aria-label="Close find bar"
          style={navButtonStyle}
        />
      </Tooltip>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Hotkey helper.
// ---------------------------------------------------------------------------

/**
 * Installs a document-level Ctrl+F / Cmd+F listener that calls `onOpen` and
 * suppresses the browser's built-in find bar. Intended to be called from the
 * component that owns the FindBar's `open` state.
 *
 * Returns nothing — the installation happens inside a `useEffect` so React
 * handles teardown when the owner unmounts or `onOpen` changes.
 *
 * Note: we intentionally do NOT swallow the event when any modal dialog or
 * text field outside the viewer has focus — the host page may legitimately
 * want the browser's find bar in those contexts. Parents that need tighter
 * control should bypass this helper and wire their own handler.
 */
export function useFindBarHotkey(onOpen: () => void): void {
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      // Cmd+F on macOS, Ctrl+F elsewhere. Skip when another modifier is held
      // (Ctrl+Shift+F is "find in files" in many editors — not ours to steal).
      const isFindCombo =
        (e.ctrlKey || e.metaKey) &&
        !e.shiftKey &&
        !e.altKey &&
        (e.key === "f" || e.key === "F");
      if (!isFindCombo) return;
      e.preventDefault();
      e.stopPropagation();
      onOpen();
    };
    document.addEventListener("keydown", handler);
    return () => {
      document.removeEventListener("keydown", handler);
    };
  }, [onOpen]);
}
