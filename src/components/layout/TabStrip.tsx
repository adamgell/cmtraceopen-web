import {
  type CSSProperties,
  type KeyboardEvent,
  type MouseEvent as ReactMouseEvent,
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";
import { Badge, tokens } from "@fluentui/react-components";

/**
 * TabStrip: horizontal tab bar for open log tabs.
 *
 * Ported from the desktop app's `components/layout/TabStrip.tsx`. The
 * desktop version reaches into the UI and log stores directly to switch /
 * close tabs and to pull the "merged tab" label. The web version is
 * prop-driven: the parent owns the tab list and responds to activate /
 * close callbacks.
 *
 * Preserved from the desktop:
 * - ResizeObserver-driven overflow: tabs shrink to `MIN_TAB_WIDTH` and
 *   surplus tabs move into an overflow chevron dropdown.
 * - Keyboard navigation (Arrow keys, Home / End, Enter / Space).
 * - Active-tab brand underline, close-"×" revealed on hover or active.
 * - Overflow dropdown lives outside the clipped tab strip so it's not cut
 *   off by the container.
 *
 * Dropped / stubbed:
 * - `sourceOpenMode` / `mergedTabState` / `closeDiff` branching. Web viewer
 *   has no merged or diff modes yet; callers can render their own label via
 *   the `label` field on each tab.
 */
export interface TabStripTab {
  id: string;
  label: string;
  /** Optional right-side numeric badge (e.g. entry count, unread marker). */
  badge?: number;
}

export interface TabStripProps {
  tabs: TabStripTab[];
  activeId: string;
  onActivate: (id: string) => void;
  onClose?: (id: string) => void;
}

/** Minimum width a tab can shrink to before being pushed to overflow. */
const MIN_TAB_WIDTH = 100;
/** Width reserved for the overflow chevron button. */
const OVERFLOW_BUTTON_WIDTH = 36;
/** Hard cap on tab width when the strip has plenty of room. */
const MAX_TAB_WIDTH = 200;

export function TabStrip({ tabs, activeId, onActivate, onClose }: TabStripProps) {
  const [hoveredId, setHoveredId] = useState<string | null>(null);
  const [overflowOpen, setOverflowOpen] = useState(false);
  const [visibleCount, setVisibleCount] = useState(tabs.length);

  const stripRef = useRef<HTMLDivElement>(null);
  const overflowRef = useRef<HTMLDivElement>(null);
  const tabRefs = useRef<Record<string, HTMLDivElement | null>>({});

  // Measure available width and compute how many tabs fit.
  useEffect(() => {
    const el = stripRef.current;
    if (!el) return;

    const computeVisible = () => {
      const containerWidth = el.clientWidth;
      if (tabs.length === 0) {
        setVisibleCount(0);
        return;
      }

      // Try fitting all tabs first.
      const widthPerTab = containerWidth / tabs.length;
      if (widthPerTab >= MIN_TAB_WIDTH) {
        setVisibleCount(tabs.length);
        return;
      }

      // Reserve space for the overflow button, then fit as many as possible.
      const availableWidth = containerWidth - OVERFLOW_BUTTON_WIDTH;
      const count = Math.max(1, Math.floor(availableWidth / MIN_TAB_WIDTH));
      setVisibleCount(Math.min(count, tabs.length));
    };

    computeVisible();

    const observer = new ResizeObserver(computeVisible);
    observer.observe(el);
    return () => observer.disconnect();
  }, [tabs.length]);

  // Close overflow dropdown when clicking outside.
  useEffect(() => {
    if (!overflowOpen) return;
    const handleDocumentClick = (e: MouseEvent) => {
      if (
        overflowRef.current &&
        !overflowRef.current.contains(e.target as Node)
      ) {
        setOverflowOpen(false);
      }
    };
    document.addEventListener("click", handleDocumentClick);
    return () => document.removeEventListener("click", handleDocumentClick);
  }, [overflowOpen]);

  const handleClose = useCallback(
    (e: ReactMouseEvent, id: string) => {
      e.stopPropagation();
      onClose?.(id);
    },
    [onClose]
  );

  const handleToggleOverflow = useCallback((e: ReactMouseEvent) => {
    e.stopPropagation();
    setOverflowOpen((prev) => !prev);
  }, []);

  const handleTabKeyDown = useCallback(
    (e: KeyboardEvent<HTMLDivElement>, index: number, visibleIds: string[]) => {
      const vc = visibleIds.length;
      if (vc === 0) return;
      const focusAt = (i: number) => {
        const id = visibleIds[i];
        if (id === undefined) return;
        onActivate(id);
        tabRefs.current[id]?.focus();
      };
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        const id = visibleIds[index];
        if (id !== undefined) onActivate(id);
      } else if (e.key === "ArrowRight") {
        e.preventDefault();
        focusAt((index + 1) % vc);
      } else if (e.key === "ArrowLeft") {
        e.preventDefault();
        focusAt((index - 1 + vc) % vc);
      } else if (e.key === "Home") {
        e.preventDefault();
        focusAt(0);
      } else if (e.key === "End") {
        e.preventDefault();
        focusAt(vc - 1);
      }
    },
    [onActivate]
  );

  if (tabs.length === 0) {
    return null;
  }

  const visibleTabs = tabs.slice(0, visibleCount);
  const overflowTabs = tabs.slice(visibleCount);
  const hasOverflow = overflowTabs.length > 0;
  const visibleIds = visibleTabs.map((t) => t.id);
  const activeVisibleIndex = visibleIds.indexOf(activeId);
  const focusableIndex = activeVisibleIndex >= 0 ? activeVisibleIndex : 0;

  return (
    <div style={outerStripStyle}>
      <div
        ref={stripRef}
        role="tablist"
        aria-label="Open log files"
        style={tabsAreaStyle}
      >
        {visibleTabs.map((tab, index) => {
          const isActive = tab.id === activeId;
          const isHovered = tab.id === hoveredId;
          return (
            <div
              key={tab.id}
              ref={(el) => {
                tabRefs.current[tab.id] = el;
              }}
              role="tab"
              aria-selected={isActive}
              tabIndex={index === focusableIndex ? 0 : -1}
              onClick={() => onActivate(tab.id)}
              onKeyDown={(e) => handleTabKeyDown(e, index, visibleIds)}
              onMouseEnter={() => setHoveredId(tab.id)}
              onMouseLeave={() => setHoveredId(null)}
              style={{
                ...tabStyle,
                ...(isActive ? activeTabStyle : inactiveTabStyle),
                flex: hasOverflow ? `0 0 ${MIN_TAB_WIDTH}px` : "1 1 0",
                maxWidth: hasOverflow ? undefined : MAX_TAB_WIDTH,
              }}
            >
              <span style={tabLabelStyle} title={tab.label}>
                {tab.label}
              </span>
              {tab.badge !== undefined && tab.badge > 0 && (
                <Badge
                  appearance="filled"
                  color={isActive ? "brand" : "informative"}
                  size="small"
                  style={{ flexShrink: 0 }}
                >
                  {tab.badge}
                </Badge>
              )}
              {onClose && (
                <button
                  type="button"
                  aria-label={`Close ${tab.label}`}
                  style={{
                    ...closeButtonBaseStyle,
                    visibility: isHovered || isActive ? "visible" : "hidden",
                  }}
                  onClick={(e) => handleClose(e, tab.id)}
                >
                  ×
                </button>
              )}
            </div>
          );
        })}
      </div>
      {hasOverflow && (
        <div ref={overflowRef} style={overflowContainerStyle}>
          <button
            type="button"
            style={overflowChevronStyle}
            aria-haspopup="listbox"
            aria-expanded={overflowOpen}
            aria-label={`${overflowTabs.length} more tabs`}
            title={`${overflowTabs.length} more tabs`}
            onClick={handleToggleOverflow}
          >
            <svg width="12" height="12" viewBox="0 0 12 12" fill="none">
              <path
                d="M2.5 4.5L6 8L9.5 4.5"
                stroke="currentColor"
                strokeWidth="1.5"
                fill="none"
                strokeLinecap="round"
                strokeLinejoin="round"
              />
            </svg>
          </button>
          {overflowOpen && (
            <div role="listbox" style={overflowDropdownStyle}>
              {overflowTabs.map((tab) => {
                const isActive = tab.id === activeId;
                return (
                  <div
                    key={tab.id}
                    role="option"
                    aria-selected={isActive}
                    style={{
                      ...overflowItemStyle,
                      backgroundColor: isActive
                        ? tokens.colorNeutralBackground1Selected
                        : "transparent",
                      fontWeight: isActive ? 600 : 400,
                    }}
                    onClick={() => {
                      onActivate(tab.id);
                      setOverflowOpen(false);
                    }}
                    onMouseEnter={(e) => {
                      (e.currentTarget as HTMLDivElement).style.backgroundColor =
                        tokens.colorNeutralBackground1Hover;
                    }}
                    onMouseLeave={(e) => {
                      (e.currentTarget as HTMLDivElement).style.backgroundColor =
                        isActive
                          ? tokens.colorNeutralBackground1Selected
                          : "transparent";
                    }}
                  >
                    <span style={overflowItemLabelStyle} title={tab.label}>
                      {tab.label}
                    </span>
                    {tab.badge !== undefined && tab.badge > 0 && (
                      <Badge
                        appearance="filled"
                        color={isActive ? "brand" : "informative"}
                        size="small"
                        style={{ flexShrink: 0 }}
                      >
                        {tab.badge}
                      </Badge>
                    )}
                    {onClose && (
                      <button
                        type="button"
                        aria-label={`Close ${tab.label}`}
                        style={overflowItemCloseStyle}
                        onClick={(e) => handleClose(e, tab.id)}
                      >
                        ×
                      </button>
                    )}
                  </div>
                );
              })}
            </div>
          )}
        </div>
      )}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Styles (ported from the desktop version; kept as standalone constants so
// they don't re-allocate per render).

const outerStripStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  height: 34,
  backgroundColor: tokens.colorNeutralBackground3,
  borderBottom: `1px solid ${tokens.colorNeutralStroke1}`,
  flexShrink: 0,
};

const tabsAreaStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  height: "100%",
  flex: 1,
  minWidth: 0,
  overflow: "hidden",
};

const tabStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  gap: 4,
  height: "100%",
  minWidth: 0,
  padding: "0 8px",
  cursor: "pointer",
  boxSizing: "border-box",
  userSelect: "none",
  fontSize: 12,
  fontFamily: "inherit",
};

const activeTabStyle: CSSProperties = {
  backgroundColor: tokens.colorNeutralBackground1,
  color: tokens.colorNeutralForeground1,
  borderBottom: `2px solid ${tokens.colorBrandBackground}`,
};

const inactiveTabStyle: CSSProperties = {
  backgroundColor: "transparent",
  color: tokens.colorNeutralForeground3,
};

const tabLabelStyle: CSSProperties = {
  overflow: "hidden",
  textOverflow: "ellipsis",
  whiteSpace: "nowrap",
  flex: 1,
  minWidth: 0,
};

const closeButtonBaseStyle: CSSProperties = {
  display: "inline-flex",
  alignItems: "center",
  justifyContent: "center",
  width: 16,
  height: 16,
  fontSize: 11,
  lineHeight: 1,
  borderRadius: 2,
  flexShrink: 0,
  cursor: "pointer",
  border: "none",
  background: "none",
  padding: 0,
  color: "inherit",
  fontFamily: "inherit",
};

const overflowContainerStyle: CSSProperties = {
  position: "relative",
  height: "100%",
  display: "flex",
  alignItems: "center",
  flexShrink: 0,
};

const overflowChevronStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  justifyContent: "center",
  width: OVERFLOW_BUTTON_WIDTH,
  height: "100%",
  cursor: "pointer",
  border: "none",
  background: "none",
  color: tokens.colorNeutralForeground3,
  padding: 0,
};

const overflowDropdownStyle: CSSProperties = {
  position: "absolute",
  top: "100%",
  right: 0,
  minWidth: 200,
  maxWidth: 300,
  backgroundColor: tokens.colorNeutralBackground1,
  border: `1px solid ${tokens.colorNeutralStroke1}`,
  borderRadius: 4,
  boxShadow: tokens.shadow8,
  zIndex: 1000,
  padding: "4px 0",
};

const overflowItemStyle: CSSProperties = {
  display: "flex",
  alignItems: "center",
  padding: "6px 12px",
  fontSize: 12,
  cursor: "pointer",
  color: tokens.colorNeutralForeground1,
  gap: 8,
};

const overflowItemLabelStyle: CSSProperties = {
  overflow: "hidden",
  textOverflow: "ellipsis",
  whiteSpace: "nowrap",
  flex: 1,
  minWidth: 0,
};

const overflowItemCloseStyle: CSSProperties = {
  display: "inline-flex",
  alignItems: "center",
  justifyContent: "center",
  width: 16,
  height: 16,
  fontSize: 11,
  lineHeight: 1,
  borderRadius: 2,
  flexShrink: 0,
  cursor: "pointer",
  border: "none",
  background: "none",
  padding: 0,
  color: tokens.colorNeutralForeground3,
};
