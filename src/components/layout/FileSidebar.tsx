import type { CSSProperties } from "react";
import { Badge, Tooltip, tokens } from "@fluentui/react-components";

/**
 * FileSidebar: left-nav list of open files / sources.
 *
 * Ported from the desktop app's `components/layout/FileSidebar.tsx`. The
 * desktop sidebar pulls from the log store (`activeSource`, `sourceEntries`,
 * `openFilePath`, ...) and uses the Tauri filesystem to enumerate
 * sibling files in a folder, watch for changes, and reload on demand.
 *
 * The web viewer has no filesystem: files arrive via drag-and-drop
 * (LocalMode) or the API server (ApiMode). The caller owns the list of
 * open sources and threads it through `items`.
 *
 * Visual feel:
 * - Fixed-width aside with `colorNeutralBackground2` backdrop and a right
 *   border, matching the desktop FileSidebar frame.
 * - Selected row gets a brand-colored left border accent + `Selected`
 *   background token.
 * - Each row has a primary label, an optional subtitle (size / component
 *   count / file path), a dirty dot (unsaved changes indicator), and a
 *   close "×" button shown on hover or when active.
 * - An empty state mirrors the desktop's `EmptyState` card.
 */
export interface FileSidebarItem {
  id: string;
  label: string;
  /** Secondary line under the label — desktop shows byte count + mtime here;
   * on the web, good candidates are file size, entry count, or source kind. */
  subtitle?: string;
  /** Dirty dot indicator. Rendered between the label and the close button. */
  dirty?: boolean;
  /** When true, the row is rendered in a "loading" state (spinner badge)
   * and not interactive. Desktop shows this when a file is being parsed. */
  loading?: boolean;
}

export interface FileSidebarProps {
  items: FileSidebarItem[];
  selectedId?: string;
  onSelect: (id: string) => void;
  /** Close a row. Desktop also exposes "remove from list" and "close
   * other tabs" via a context menu; not ported here — callers can layer it
   * on top if they need it. */
  onClose?: (id: string) => void;
  /** Width of the aside. Matches the desktop's 280px recommended default. */
  width?: number | string;
  /** Optional header slot — e.g. "Open Files", a count badge, or an
   * "Add file" button. */
  header?: React.ReactNode;
  /** Optional footer slot — desktop puts pause/resume/refresh controls here. */
  footer?: React.ReactNode;
  /** Collapse callback. When provided, a chevron button appears in the
   * top-right of the sidebar (matches the desktop behaviour). */
  onCollapse?: () => void;
  /** Body shown when `items` is empty. */
  emptyState?: {
    title: string;
    body?: string;
  };
}

const DEFAULT_WIDTH = 280;

export function FileSidebar({
  items,
  selectedId,
  onSelect,
  onClose,
  width = DEFAULT_WIDTH,
  header,
  footer,
  onCollapse,
  emptyState,
}: FileSidebarProps) {
  return (
    <aside
      aria-label="Open files"
      style={{
        ...asideStyle,
        width,
        minWidth: typeof width === "number" ? `${width}px` : width,
      }}
    >
      {/* TODO(web-port): desktop uses `getLogListMetrics(logListFontSize)` to
       * sync this aside's font-size with the grid. Web viewer has no
       * font-size store yet — sticks with the Fluent default. */}

      {(header || onCollapse) && (
        <div style={headerRowStyle}>
          <div style={{ flex: 1, minWidth: 0 }}>{header}</div>
          {onCollapse && (
            <Tooltip
              content="Collapse sidebar"
              relationship="label"
              withArrow
            >
              <button
                type="button"
                onClick={onCollapse}
                aria-label="Collapse sidebar"
                style={collapseButtonStyle}
              >
                <svg width="16" height="16" viewBox="0 0 16 16" fill="currentColor">
                  <path d="M10 3L5 8l5 5V3z" />
                </svg>
              </button>
            </Tooltip>
          )}
        </div>
      )}

      <div style={listStyle}>
        {items.length === 0 ? (
          <EmptyState
            title={emptyState?.title ?? "No files open"}
            body={
              emptyState?.body ??
              "Drop a log file anywhere in the window to open it."
            }
          />
        ) : (
          items.map((item) => (
            <FileRow
              key={item.id}
              item={item}
              isSelected={item.id === selectedId}
              onSelect={onSelect}
              onClose={onClose}
            />
          ))
        )}
      </div>

      {footer && <div style={footerRowStyle}>{footer}</div>}
    </aside>
  );
}

// ---------------------------------------------------------------------------
// Row

function FileRow({
  item,
  isSelected,
  onSelect,
  onClose,
}: {
  item: FileSidebarItem;
  isSelected: boolean;
  onSelect: (id: string) => void;
  onClose?: (id: string) => void;
}) {
  const disabled = item.loading === true;
  return (
    <div
      style={{
        position: "relative",
        borderBottom: `1px solid ${tokens.colorNeutralStroke2}`,
      }}
    >
      <button
        type="button"
        onClick={() => {
          if (!disabled) onSelect(item.id);
        }}
        disabled={disabled}
        aria-pressed={isSelected}
        title={item.subtitle ?? item.label}
        style={{
          ...rowButtonStyle,
          borderLeft: isSelected
            ? `3px solid ${tokens.colorCompoundBrandStroke}`
            : "3px solid transparent",
          backgroundColor: isSelected
            ? tokens.colorNeutralBackground1Selected
            : tokens.colorNeutralBackground1,
          cursor: disabled ? "default" : "pointer",
          opacity: disabled && !isSelected ? 0.7 : 1,
        }}
      >
        <div style={rowHeaderStyle}>
          <div
            style={{
              ...rowLabelStyle,
              fontWeight: isSelected ? 600 : 400,
            }}
          >
            {item.label}
          </div>
          {item.dirty && (
            <span
              aria-label="Unsaved changes"
              title="Unsaved changes"
              style={{
                width: 8,
                height: 8,
                borderRadius: "50%",
                backgroundColor: tokens.colorBrandBackground,
                flexShrink: 0,
              }}
            />
          )}
          {isSelected && (
            <Badge
              appearance="outline"
              color="brand"
              size="small"
              style={{ flexShrink: 0 }}
            >
              Active
            </Badge>
          )}
          {item.loading && !isSelected && (
            <Badge
              appearance="ghost"
              color="informative"
              size="small"
              style={{ flexShrink: 0 }}
            >
              Loading...
            </Badge>
          )}
        </div>
        {item.subtitle && <div style={rowSubtitleStyle}>{item.subtitle}</div>}
      </button>
      {onClose && (
        <button
          type="button"
          aria-label={`Close ${item.label}`}
          onClick={(e) => {
            e.stopPropagation();
            onClose(item.id);
          }}
          style={closeButtonStyle}
        >
          ×
        </button>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Empty state

function EmptyState({ title, body }: { title: string; body?: string }) {
  return (
    <div style={emptyStateStyle}>
      <div style={emptyTitleStyle}>{title}</div>
      {body && <div style={emptyBodyStyle}>{body}</div>}
      {/* TODO(web-port): desktop offers a "Pick folder" / "Pick file" button
       * here that invokes the Tauri open() dialog. On the web, the caller
       * should render their own picker (e.g. <DropZone>) above or below the
       * sidebar — this component stays purely presentational. */}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Styles

const asideStyle: CSSProperties = {
  display: "flex",
  flexDirection: "column",
  overflow: "hidden",
  backgroundColor: tokens.colorNeutralBackground2,
  borderRight: `1px solid ${tokens.colorNeutralStroke2}`,
};

const headerRowStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 6,
  padding: "8px 10px",
  borderBottom: `1px solid ${tokens.colorNeutralStroke2}`,
  backgroundColor: tokens.colorNeutralBackground2,
  flexShrink: 0,
};

const collapseButtonStyle: CSSProperties = {
  background: "none",
  border: "none",
  cursor: "pointer",
  padding: 4,
  borderRadius: 4,
  color: tokens.colorNeutralForeground3,
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  flexShrink: 0,
};

const listStyle: CSSProperties = {
  flex: 1,
  overflow: "auto",
  backgroundColor: tokens.colorNeutralBackground2,
};

const rowButtonStyle: CSSProperties = {
  width: "100%",
  textAlign: "left",
  padding: "8px 10px",
  paddingRight: 32, // leave room for the close button
  border: "none",
  background: "transparent",
  fontFamily: "inherit",
  fontSize: "inherit",
  color: tokens.colorNeutralForeground1,
};

const rowHeaderStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 8,
  minWidth: 0,
};

const rowLabelStyle: CSSProperties = {
  flex: 1,
  minWidth: 0,
  overflow: "hidden",
  textOverflow: "ellipsis",
  whiteSpace: "nowrap",
};

const rowSubtitleStyle: CSSProperties = {
  marginTop: 3,
  fontSize: 11,
  color: tokens.colorNeutralForeground3,
  overflow: "hidden",
  textOverflow: "ellipsis",
  whiteSpace: "nowrap",
};

const closeButtonStyle: CSSProperties = {
  position: "absolute",
  top: "50%",
  right: 6,
  transform: "translateY(-50%)",
  display: "inline-flex",
  alignItems: "center",
  justifyContent: "center",
  width: 20,
  height: 20,
  fontSize: 14,
  lineHeight: 1,
  borderRadius: 3,
  cursor: "pointer",
  border: "none",
  background: "none",
  padding: 0,
  color: tokens.colorNeutralForeground3,
};

const footerRowStyle: CSSProperties = {
  marginTop: "auto",
  padding: "6px 8px",
  borderTop: `1px solid ${tokens.colorNeutralStroke2}`,
  display: "flex",
  gap: 5,
  alignItems: "center",
  flexShrink: 0,
};

const emptyStateStyle: CSSProperties = {
  padding: "24px 16px",
  textAlign: "center",
  color: tokens.colorNeutralForeground2,
};

const emptyTitleStyle: CSSProperties = {
  fontSize: 13,
  fontWeight: 600,
  color: tokens.colorNeutralForeground1,
  marginBottom: 6,
};

const emptyBodyStyle: CSSProperties = {
  fontSize: 12,
  color: tokens.colorNeutralForeground3,
  lineHeight: 1.45,
};

// The sidebar stays layout-only — parents compose action controls via the
// `header` and `footer` slots, which keeps this file free of app-specific
// behaviour.
