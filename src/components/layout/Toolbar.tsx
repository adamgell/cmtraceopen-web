import type { CSSProperties, ReactNode } from "react";
import {
  Button,
  Divider,
  Tooltip,
  tokens,
} from "@fluentui/react-components";

/**
 * Toolbar: modal actions above the log grid.
 *
 * Ported from the desktop app's `components/layout/Toolbar.tsx`. The desktop
 * toolbar pulls its state from several Zustand stores and drives Tauri
 * file-system operations directly. The web viewer has no filesystem and no
 * shared store, so this version is entirely prop-driven — the parent
 * (ViewerShell / LocalMode / ApiMode) owns the state and wires callbacks.
 *
 * Visual feel:
 * - Flex row, wrapping, `colorNeutralBackground2` strip with a bottom border.
 * - Inline SVG icons at 16px, paired with short labels; icon-only when
 *   `compact` is true.
 * - Primary-appearance toggle state for pane visibility buttons
 *   (`detailsVisible`, `infoVisible`) mirrors the desktop aria-pressed idiom.
 * - Dividers segment the bar into: source actions / find-and-filter /
 *   view toggles / trailing slot (caller-owned, e.g. ThemePicker).
 *
 * Not ported:
 * - Known-source catalog menus (desktop scans the FS for "known" logs — the
 *   web has no catalog). See TODO(web-port) in the source-actions group.
 * - dsregcmd paste/capture. Desktop-only: reads clipboard + spawns a process.
 * - Workspace picker. Web has a single workspace.
 * - Merge / Diff tabs. Delegated to the caller via `extras` if needed.
 */
export interface ToolbarProps {
  /** Open-file action. Triggers the browser file picker in LocalMode,
   * or the API "pick a device/log" flow in ApiMode. */
  onOpenFile?: () => void;
  /** Reload the active source. In LocalMode this re-parses the last dropped
   * file; in ApiMode it re-queries the server. */
  onReload?: () => void;
  /** Drop the active source / clear the grid. */
  onClear?: () => void;
  /** Show the find bar. Parent owns visibility + focus management. */
  onFind?: () => void;
  /** Toggle the details pane. */
  onToggleDetails?: () => void;
  /** Toggle the info pane. */
  onToggleInfo?: () => void;

  /** Gate the reload button. Desktop disables it while loading or when
   * there's no source to refresh. */
  canReload?: boolean;
  /** Gate the clear button — same as canReload in practice. */
  canClear?: boolean;
  /** Gate the find button (desktop requires entries > 0). */
  canFind?: boolean;
  /** Toggle-state for the details / info buttons. Rendered as the "primary"
   * Fluent appearance so the pressed state is visually obvious. */
  detailsVisible?: boolean;
  infoVisible?: boolean;

  /** Optional highlight box — desktop puts it on the toolbar so it's always
   * reachable. Leave unset and it won't render. */
  highlight?: {
    value: string;
    onChange: (next: string) => void;
  };

  /** Compact mode drops the text labels and keeps icons only, for narrow
   * windows. */
  compact?: boolean;

  /** Trailing slot for caller-provided controls (e.g. ThemePicker,
   * AuthSettings, Close-file button). Renders flush-right after a spacer. */
  extras?: ReactNode;
}

export function Toolbar({
  onOpenFile,
  onReload,
  onClear,
  onFind,
  onToggleDetails,
  onToggleInfo,
  canReload = true,
  canClear = true,
  canFind = true,
  detailsVisible = false,
  infoVisible = false,
  highlight,
  compact = false,
  extras,
}: ToolbarProps) {
  return (
    <div style={stripStyle}>
      {/* Source actions */}
      {onOpenFile && (
        <ToolbarButton
          label="Open"
          tooltip="Open a log file"
          icon={<IconOpenFile />}
          onClick={onOpenFile}
          compact={compact}
        />
      )}
      {onReload && (
        // TODO(web-port): desktop uses a FileSystemWatcher to auto-refresh;
        // web must always be a manual click. Caller is responsible for
        // re-parsing the dropped file / re-querying the API.
        <ToolbarButton
          label="Reload"
          tooltip="Reload the active source"
          icon={<IconReload />}
          onClick={onReload}
          disabled={!canReload}
          compact={compact}
        />
      )}
      {onClear && (
        <ToolbarButton
          label="Clear"
          tooltip="Close the active source"
          icon={<IconClear />}
          onClick={onClear}
          disabled={!canClear}
          compact={compact}
        />
      )}

      {/* TODO(web-port): known-source catalog menus, dsregcmd paste/capture,
       * and the "watch file" toggle are all desktop-only and intentionally
       * omitted here. */}

      {(onOpenFile || onReload || onClear) && (onFind || highlight) && (
        <Divider vertical />
      )}

      {/* Find + highlight */}
      {onFind && (
        <ToolbarButton
          label="Find"
          tooltip="Find in log (Ctrl+F)"
          icon={<IconFind />}
          onClick={onFind}
          disabled={!canFind}
          compact={compact}
        />
      )}
      {highlight && (
        <input
          type="search"
          value={highlight.value}
          onChange={(e) => highlight.onChange(e.target.value)}
          placeholder="Highlight..."
          aria-label="Highlight text"
          style={{
            padding: "4px 8px",
            fontSize: 12,
            border: `1px solid ${tokens.colorNeutralStroke1}`,
            borderRadius: tokens.borderRadiusMedium,
            background: tokens.colorNeutralBackground1,
            color: tokens.colorNeutralForeground1,
            width: 200,
            minWidth: 120,
          }}
        />
      )}

      {(onFind || highlight) && (onToggleDetails || onToggleInfo) && (
        <Divider vertical />
      )}

      {/* View toggles */}
      {onToggleDetails && (
        <ToolbarButton
          label="Details"
          tooltip="Show / Hide Details"
          icon={<IconDetails />}
          onClick={onToggleDetails}
          pressed={detailsVisible}
          compact={compact}
        />
      )}
      {onToggleInfo && (
        <ToolbarButton
          label="Info"
          tooltip="Toggle Info Pane"
          icon={<IconInfo />}
          onClick={onToggleInfo}
          pressed={infoVisible}
          compact={compact}
        />
      )}

      <div style={{ flex: 1 }} />
      {extras}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Internals

function ToolbarButton({
  label,
  tooltip,
  icon,
  onClick,
  disabled,
  pressed,
  compact,
}: {
  label: string;
  tooltip: string;
  icon: ReactNode;
  onClick: () => void;
  disabled?: boolean;
  pressed?: boolean;
  compact?: boolean;
}) {
  const btn = (
    <Button
      size="small"
      appearance={pressed ? "primary" : "secondary"}
      disabled={disabled}
      onClick={onClick}
      aria-pressed={pressed}
      aria-label={label}
      icon={
        <span
          aria-hidden
          style={{
            display: "inline-flex",
            alignItems: "center",
            justifyContent: "center",
          }}
        >
          {icon}
        </span>
      }
    >
      {compact ? null : label}
    </Button>
  );
  return (
    <Tooltip content={tooltip} relationship="label" withArrow>
      {btn}
    </Tooltip>
  );
}

const stripStyle: CSSProperties = {
  display: "flex",
  flexWrap: "wrap",
  alignItems: "center",
  gap: 10,
  padding: "10px 12px",
  backgroundColor: tokens.colorNeutralBackground2,
  borderBottom: `1px solid ${tokens.colorNeutralStroke2}`,
  flexShrink: 0,
};

// ---------------------------------------------------------------------------
// Icons (inline SVG, 16px). Kept self-contained so the toolbar has no
// dependency on `@fluentui/react-icons`.

function IconOpenFile() {
  return (
    <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
      <path d="M2 4h4l1.5 1.5H14V12a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1V4z" />
    </svg>
  );
}

function IconReload() {
  return (
    <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
      <path d="M13.5 8a5.5 5.5 0 1 1-1.61-3.89" />
      <path d="M13.5 2.5V5.5H10.5" />
    </svg>
  );
}

function IconClear() {
  return (
    <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
      <path d="M4 4l8 8M12 4l-8 8" />
    </svg>
  );
}

function IconFind() {
  return (
    <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
      <circle cx="7" cy="7" r="4.5" />
      <path d="M10.5 10.5L14 14" />
    </svg>
  );
}

function IconDetails() {
  return (
    <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
      <rect x="2" y="3" width="12" height="10" rx="1" />
      <path d="M2 7h12" />
      <path d="M10 7v6" />
    </svg>
  );
}

function IconInfo() {
  return (
    <svg width="16" height="16" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
      <circle cx="8" cy="8" r="6" />
      <path d="M8 7v4" />
      <circle cx="8" cy="5" r="0.5" fill="currentColor" />
    </svg>
  );
}
