import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  Button,
  Dropdown,
  Menu,
  MenuButton,
  MenuItemRadio,
  MenuList,
  MenuPopover,
  MenuTrigger,
  Option,
  tokens,
} from "@fluentui/react-components";
import type { ParseResult } from "../lib/log-types";
import { LocalMode, type LocalModeHandle } from "./LocalMode";
import { ApiMode, type ApiModeHandle } from "./ApiMode";
import { DevicesPanel } from "./DevicesPanel";
import { AuthSettings } from "./AuthSettings";
import { StatusBar } from "./StatusBar";
import { TabStrip } from "./layout/TabStrip";
import { Toolbar } from "./layout/Toolbar";
import { FileSidebar } from "./layout/FileSidebar";
import { DiffView, type DiffViewState } from "./log-view/DiffView";
import type { DiffDisplayMode } from "./log-view/DiffHeader";
import type { LogEntry } from "../lib/log-types";
import { apiListEntries } from "../lib/api-client";
import { classifyEntries } from "../lib/diff-entries";
import { useTheme } from "../lib/theme-context";
import {
  useWorkspace,
  type PinnedItem,
} from "../lib/workspace-context";

type Mode = "local" | "api" | "devices" | "diff";

const MODE_TABS = [
  { id: "local", label: "Local" },
  { id: "api", label: "API" },
  { id: "devices", label: "Devices" },
  { id: "diff", label: "Diff" },
] as const;

const SIDEBAR_STORAGE_KEY = "cmtraceopen-web.sidebar-collapsed";

interface LoadedSummary {
  fileName: string;
  result: ParseResult;
}

function loadSidebarCollapsed(): boolean {
  if (typeof window === "undefined") return false;
  try {
    return window.localStorage.getItem(SIDEBAR_STORAGE_KEY) === "true";
  } catch {
    return false;
  }
}

function persistSidebarCollapsed(value: boolean): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(SIDEBAR_STORAGE_KEY, value ? "true" : "false");
  } catch {
    // Best-effort only.
  }
}

function subtitleForKind(kind: PinnedItem["kind"]): string {
  switch (kind) {
    case "local-file":
      return "Local file";
    case "api-session":
      return "API session";
    case "api-file":
      return "API file";
  }
}

function modeLabelFor(mode: Mode): string {
  switch (mode) {
    case "local":
      return "Local mode — drop or pick a CMTrace log";
    case "api":
      return "API mode — browse devices / sessions / files";
    case "devices":
      return "Devices mode — manage registered agents";
    case "diff":
      return "Diff mode — compare two pinned sources";
  }
}

/**
 * Top-level viewer shell.
 *
 * Hosts the title bar + mode tabs + theme picker + auth settings, and
 * routes the main region to LocalMode / ApiMode / DevicesPanel. All
 * colors come from the active theme's Fluent tokens, so switching
 * themes reflows the whole UI (including the CMTrace-style log grid).
 */
export function ViewerShell() {
  const [mode, setMode] = useState<Mode>("local");
  const [loaded, setLoaded] = useState<LoadedSummary | null>(null);
  const [sidebarCollapsed, setSidebarCollapsed] = useState<boolean>(() =>
    loadSidebarCollapsed(),
  );

  const localRef = useRef<LocalModeHandle>(null);
  const apiRef = useRef<ApiModeHandle>(null);

  const workspace = useWorkspace();

  const handleLoaded = useCallback((info: LoadedSummary | null) => {
    setLoaded(info);
  }, []);

  const handleModeChange = useCallback((next: Mode) => {
    setMode(next);
    if (next !== "local") setLoaded(null);
  }, []);

  const updateSidebarCollapsed = useCallback((next: boolean) => {
    setSidebarCollapsed(next);
    persistSidebarCollapsed(next);
  }, []);

  // Per-mode action wiring. The Toolbar is mounted unconditionally so the
  // bar itself never appears/disappears; we just gate which action props
  // are supplied based on the mode.
  const toolbarProps = useMemo(() => {
    if (mode === "local") {
      return {
        onOpenFile: () => localRef.current?.openFile(),
        onReload: loaded ? () => localRef.current?.reload() : undefined,
        onClear: loaded
          ? () => {
              localRef.current?.clear();
              setLoaded(null);
            }
          : undefined,
        canReload: !!loaded,
        canClear: !!loaded,
      };
    }
    if (mode === "api") {
      return {
        onReload: () => apiRef.current?.reload(),
        onClear: () => apiRef.current?.clear(),
        canReload: true,
        canClear: true,
      };
    }
    // devices / diff: extras-only, no actions.
    return {};
  }, [mode, loaded]);

  const sidebarItems = useMemo(
    () =>
      workspace.items.map((it) => ({
        id: it.id,
        label: it.label,
        subtitle: subtitleForKind(it.kind),
      })),
    [workspace.items],
  );

  return (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        height: "100vh",
        background: tokens.colorNeutralBackground1,
        color: tokens.colorNeutralForeground1,
      }}
    >
      <TopBar
        mode={mode}
        onModeChange={handleModeChange}
        loaded={mode === "local" ? loaded : null}
        onClose={() => setLoaded(null)}
      />
      <Toolbar
        {...toolbarProps}
        extras={
          <span
            style={{
              fontSize: tokens.fontSizeBase200,
              color: tokens.colorNeutralForeground3,
            }}
          >
            {modeLabelFor(mode)}
          </span>
        }
      />
      <div
        style={{
          flex: 1,
          minHeight: 0,
          display: "flex",
          flexDirection: "row",
        }}
      >
        {sidebarCollapsed ? (
          <div
            style={{
              display: "flex",
              alignItems: "flex-start",
              padding: "8px 4px",
              borderRight: `1px solid ${tokens.colorNeutralStroke2}`,
              background: tokens.colorNeutralBackground2,
            }}
          >
            <Button
              size="small"
              aria-label="Expand sidebar"
              title="Expand sidebar"
              onClick={() => updateSidebarCollapsed(false)}
            >
              {">"}
            </Button>
          </div>
        ) : (
          <FileSidebar
            items={sidebarItems}
            selectedId={workspace.activeId ?? undefined}
            onSelect={(id) => workspace.setActive(id)}
            onClose={(id) => workspace.unpin(id)}
            onCollapse={() => updateSidebarCollapsed(true)}
            header={
              <div
                style={{
                  fontSize: tokens.fontSizeBase200,
                  fontWeight: 600,
                  color: tokens.colorNeutralForeground1,
                }}
              >
                Workspace
              </div>
            }
            emptyState={{
              title: "No pinned items",
              body:
                "Pin a session or file from Local / API mode to see it here.",
            }}
          />
        )}
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
          {mode === "local" ? (
            <LocalMode ref={localRef} onLoaded={handleLoaded} />
          ) : mode === "devices" ? (
            <DevicesPanel />
          ) : mode === "diff" ? (
            <DiffMode />
          ) : (
            <ApiMode ref={apiRef} />
          )}
        </main>
      </div>
      <StatusBar
        sourceLabel={mode === "local" ? loaded?.fileName : undefined}
        totalEntries={
          mode === "local" ? loaded?.result.entries.length : undefined
        }
        totalLines={mode === "local" ? loaded?.result.totalLines : undefined}
        parseErrors={mode === "local" ? loaded?.result.parseErrors : undefined}
        formatDetected={
          mode === "local" && loaded
            ? String(loaded.result.formatDetected)
            : undefined
        }
        connectionState={
          mode === "local" && loaded ? "connected" : "idle"
        }
        badges={<span>mode: {mode}</span>}
      />
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
        padding: "8px 16px",
        borderBottom: `1px solid ${tokens.colorNeutralStroke1}`,
        background: tokens.colorNeutralBackground2,
      }}
    >
      <div
        style={{
          fontWeight: 600,
          fontSize: tokens.fontSizeBase400,
          color: tokens.colorNeutralForeground1,
        }}
      >
        CMTrace Open
      </div>
      <div style={{ flex: "0 1 420px", minWidth: 320 }}>
        <TabStrip
          tabs={MODE_TABS.map((t) => ({ id: t.id, label: t.label }))}
          activeId={mode}
          onActivate={(id) => onModeChange(id as Mode)}
        />
      </div>
      {loaded && (
        <div
          style={{
            color: tokens.colorNeutralForeground2,
            fontSize: tokens.fontSizeBase200,
          }}
        >
          <span
            style={{
              fontWeight: 500,
              color: tokens.colorNeutralForeground1,
            }}
          >
            {loaded.fileName}
          </span>
          <span style={{ color: tokens.colorNeutralForeground3, marginLeft: 12 }}>
            {loaded.result.entries.length.toLocaleString()} entries
            {" · "}
            {loaded.result.totalLines.toLocaleString()} lines
            {" · "}
            <span
              style={{
                color:
                  loaded.result.parseErrors > 0
                    ? tokens.colorPaletteRedForeground1
                    : tokens.colorNeutralForeground3,
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
      <ThemePicker />
      <AuthSettings />
      {loaded && (
        <Button size="small" onClick={onClose}>
          Close file
        </Button>
      )}
    </header>
  );
}

/**
 * Diff-mode picker.
 *
 * Consumes the workspace pinboard: the operator pins two files (local or
 * API) and picks which is Source A / Source B. The underlying `DiffView`
 * wants two `entries: LogEntry[]` arrays and the web viewer doesn't yet
 * have a unified "load entries for any pinned item" loader, so this pass
 * is picker-only — the "Load diff" button shows a stub explaining the
 * wiring will follow once that loader lands.
 */
interface LoadedDiff {
  state: DiffViewState;
  warnings: string[];
}

type DiffLoadState =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "error"; message: string }
  | { status: "ready"; diff: LoadedDiff };

const DIFF_ENTRY_CAP = 5000;
const DIFF_PAGE_SIZE = 500;

/**
 * Fetch up to `cap` entries for an api-file pin, paging through the
 * server's cursor until exhausted or the cap is hit. Returns both the
 * collected entries and whether the cap was reached (truncated flag).
 */
async function fetchAllApiEntries(
  sessionId: string,
  fileId: string,
  cap = DIFF_ENTRY_CAP,
): Promise<{ entries: LogEntry[]; truncated: boolean }> {
  const out: LogEntry[] = [];
  let cursor: string | null | undefined;
  while (out.length < cap) {
    const remaining = cap - out.length;
    const limit = Math.min(DIFF_PAGE_SIZE, remaining);
    const page = await apiListEntries(sessionId, {
      file: fileId,
      limit,
      cursor: cursor ?? undefined,
    });
    // Trust the api-client's DTO mapping: the shape matches LogEntry
    // shape closely enough that the viewer renders it verbatim.
    out.push(...(page.items as unknown as LogEntry[]));
    if (!page.nextCursor || page.items.length === 0) break;
    cursor = page.nextCursor;
  }
  return { entries: out, truncated: out.length >= cap };
}

function DiffMode() {
  const { items, getLocalEntries } = useWorkspace();
  const [sourceAId, setSourceAId] = useState<string | null>(null);
  const [sourceBId, setSourceBId] = useState<string | null>(null);
  const [load, setLoad] = useState<DiffLoadState>({ status: "idle" });
  const [selectedEntryId, setSelectedEntryId] = useState<number | null>(null);
  const [displayMode, setDisplayMode] = useState<DiffDisplayMode>("side-by-side");

  const diffable = useMemo(
    () =>
      items.filter(
        (it) => it.kind === "local-file" || it.kind === "api-file",
      ),
    [items],
  );

  const sourceA = diffable.find((it) => it.id === sourceAId) ?? null;
  const sourceB = diffable.find((it) => it.id === sourceBId) ?? null;
  const bothPicked = sourceA !== null && sourceB !== null;
  const sameSource =
    bothPicked && sourceA.id === sourceB.id;

  /** Fetch entries + truncation hint for one side of the diff.
   *
   * - api-file: paginate apiListEntries up to DIFF_ENTRY_CAP.
   * - local-file: read from the in-memory cache seeded by LocalMode's
   *   Pin-file button. The cache is dropped on page reload, so we
   *   surface a clear error when the operator re-opens the viewer and
   *   tries to diff a pin they created in a previous session.
   */
  const loadSourceEntries = useCallback(
    async (pin: PinnedItem): Promise<{ entries: LogEntry[]; truncated: boolean }> => {
      if (pin.kind === "api-file") {
        return fetchAllApiEntries(pin.sessionId, pin.fileId);
      }
      if (pin.kind === "local-file") {
        const cached = getLocalEntries(pin.id);
        if (!cached || cached.length === 0) {
          throw new Error(
            `Local file "${pin.fileName}" isn't in the in-memory cache (probably because the page was reloaded after pinning). Re-open it in Local mode and click Pin file again to refill the cache.`,
          );
        }
        // Cap local sources the same way as API ones so a >5000-entry
        // file doesn't blow up the classifier.
        const capped = cached.slice(0, DIFF_ENTRY_CAP);
        return {
          entries: capped as LogEntry[],
          truncated: cached.length > DIFF_ENTRY_CAP,
        };
      }
      throw new Error(`Unsupported pinned kind: ${(pin as PinnedItem).kind}`);
    },
    [getLocalEntries],
  );

  const handleLoadDiff = useCallback(async () => {
    if (!sourceA || !sourceB || sameSource) return;
    setLoad({ status: "loading" });
    setSelectedEntryId(null);
    try {
      const [loadedA, loadedB] = await Promise.all([
        loadSourceEntries(sourceA),
        loadSourceEntries(sourceB),
      ]);

      // Renumber ids so A and B live in non-overlapping id spaces; the
      // classifier's Map<entry.id, classification> collapses otherwise
      // when both sources happen to have the same auto-increment ids.
      const entriesA: LogEntry[] = loadedA.entries.map((e, i) => ({
        ...e,
        id: i + 1,
      }));
      const idOffset = entriesA.length;
      const entriesB: LogEntry[] = loadedB.entries.map((e, i) => ({
        ...e,
        id: idOffset + i + 1,
      }));

      const { entryClassification, stats } = classifyEntries(
        entriesA,
        entriesB,
      );

      const warnings: string[] = [];
      if (loadedA.truncated) {
        warnings.push(
          `Source A truncated at ${DIFF_ENTRY_CAP.toLocaleString()} entries; older lines are not included.`,
        );
      }
      if (loadedB.truncated) {
        warnings.push(
          `Source B truncated at ${DIFF_ENTRY_CAP.toLocaleString()} entries; older lines are not included.`,
        );
      }

      setLoad({
        status: "ready",
        diff: {
          state: {
            sourceA: { filePath: sourceA.label },
            sourceB: { filePath: sourceB.label },
            entriesA,
            entriesB,
            entryClassification,
            stats,
            displayMode,
          },
          warnings,
        },
      });
    } catch (e) {
      setLoad({
        status: "error",
        message: e instanceof Error ? e.message : String(e),
      });
    }
  }, [sourceA, sourceB, sameSource, displayMode, loadSourceEntries]);

  // Keep DiffViewState in sync with local displayMode toggle changes
  // without re-fetching (the classifier result is stable; only the
  // layout flag changes).
  useEffect(() => {
    setLoad((prev) =>
      prev.status === "ready"
        ? {
            status: "ready",
            diff: {
              ...prev.diff,
              state: { ...prev.diff.state, displayMode },
            },
          }
        : prev,
    );
  }, [displayMode]);

  // Drop any previous result when the picker changes.
  useEffect(() => {
    setLoad({ status: "idle" });
    setSelectedEntryId(null);
  }, [sourceAId, sourceBId]);

  const renderPicker = (
    label: string,
    value: string | null,
    onChange: (next: string | null) => void,
  ) => (
    <div
      style={{
        display: "flex",
        flexDirection: "column",
        gap: 4,
        minWidth: 260,
      }}
    >
      <label
        style={{
          fontSize: tokens.fontSizeBase200,
          color: tokens.colorNeutralForeground2,
          fontWeight: 500,
        }}
      >
        {label}
      </label>
      <Dropdown
        placeholder="Select a pinned file"
        value={
          value
            ? diffable.find((it) => it.id === value)?.label ?? ""
            : ""
        }
        selectedOptions={value ? [value] : []}
        onOptionSelect={(_e, data) => {
          onChange(data.optionValue ?? null);
        }}
      >
        {diffable.map((it) => (
          <Option key={it.id} value={it.id} text={it.label}>
            {it.label}
          </Option>
        ))}
      </Dropdown>
    </div>
  );

  return (
    <div
      style={{
        flex: 1,
        display: "flex",
        flexDirection: "column",
        gap: 16,
        padding: 24,
        background: tokens.colorNeutralBackground2,
        border: `1px dashed ${tokens.colorNeutralStroke1}`,
        borderRadius: tokens.borderRadiusMedium,
      }}
    >
      <div>
        <div
          style={{
            fontWeight: 600,
            fontSize: tokens.fontSizeBase400,
            color: tokens.colorNeutralForeground1,
            marginBottom: 4,
          }}
        >
          Diff two pinned sources
        </div>
        <div
          style={{
            fontSize: tokens.fontSizeBase200,
            color: tokens.colorNeutralForeground3,
          }}
        >
          Pick two files (local or API) from the workspace. Sessions can't be
          diffed directly — pin a specific file from a session instead.
        </div>
      </div>

      {diffable.length < 2 ? (
        <div
          style={{
            fontSize: tokens.fontSizeBase300,
            color: tokens.colorNeutralForeground2,
          }}
        >
          Pin at least two files to your workspace before using diff mode.
          {diffable.length === 1
            ? " You currently have one file pinned."
            : " Your workspace has no diffable files yet."}
        </div>
      ) : (
        <>
          <div style={{ display: "flex", flexWrap: "wrap", gap: 16 }}>
            {renderPicker("Source A", sourceAId, setSourceAId)}
            {renderPicker("Source B", sourceBId, setSourceBId)}
          </div>

          {!bothPicked && (
            <div
              style={{
                fontSize: tokens.fontSizeBase200,
                color: tokens.colorNeutralForeground3,
              }}
            >
              Pick a file for both sources to enable the diff.
            </div>
          )}
          {sameSource && (
            <div
              style={{
                fontSize: tokens.fontSizeBase200,
                color: tokens.colorPaletteRedForeground1,
              }}
            >
              Source A and Source B must be different files.
            </div>
          )}
          {bothPicked && !sameSource && (
            <>
              <div
                style={{
                  fontSize: tokens.fontSizeBase200,
                  color: tokens.colorNeutralForeground2,
                }}
              >
                Ready to diff{" "}
                <span style={{ color: tokens.colorNeutralForeground1 }}>
                  {sourceA!.label}
                </span>{" "}
                against{" "}
                <span style={{ color: tokens.colorNeutralForeground1 }}>
                  {sourceB!.label}
                </span>
                .
              </div>
              <div>
                <Button
                  appearance="primary"
                  size="small"
                  disabled={load.status === "loading"}
                  onClick={handleLoadDiff}
                >
                  {load.status === "loading" ? "Loading…" : "Load diff"}
                </Button>
              </div>
            </>
          )}

          {load.status === "error" && (
            <div
              style={{
                fontSize: tokens.fontSizeBase300,
                color: tokens.colorPaletteRedForeground1,
                padding: 12,
                background: tokens.colorPaletteRedBackground1,
                border: `1px solid ${tokens.colorPaletteRedBorder1}`,
                borderRadius: tokens.borderRadiusMedium,
              }}
            >
              {load.message}
            </div>
          )}

          {load.status === "ready" && (
            <>
              {load.diff.warnings.length > 0 && (
                <div
                  style={{
                    fontSize: tokens.fontSizeBase200,
                    color: tokens.colorPaletteDarkOrangeForeground1,
                    padding: 8,
                    background: tokens.colorPaletteYellowBackground2,
                    border: `1px solid ${tokens.colorPaletteYellowBorderActive}`,
                    borderRadius: tokens.borderRadiusMedium,
                  }}
                >
                  {load.diff.warnings.join(" · ")}
                </div>
              )}
              <div
                style={{
                  flex: 1,
                  minHeight: 400,
                  display: "flex",
                  flexDirection: "column",
                }}
              >
                <DiffView
                  diffState={load.diff.state}
                  selectedId={selectedEntryId}
                  onSelect={setSelectedEntryId}
                  onChangeDisplayMode={setDisplayMode}
                  onClose={() => {
                    setLoad({ status: "idle" });
                    setSelectedEntryId(null);
                  }}
                />
              </div>
            </>
          )}
        </>
      )}
    </div>
  );
}

function ThemePicker() {
  const { themeId, setThemeId, allThemes, theme } = useTheme();
  return (
    <Menu checkedValues={{ theme: [themeId] }}>
      <MenuTrigger disableButtonEnhancement>
        <MenuButton
          size="small"
          style={{ minWidth: 0 }}
          title={`Theme: ${theme.label}`}
        >
          <span
            aria-hidden
            style={{
              display: "inline-block",
              width: 10,
              height: 10,
              borderRadius: "50%",
              background: theme.swatchColor,
              marginRight: 6,
              verticalAlign: "middle",
              border: `1px solid ${tokens.colorNeutralStroke1}`,
            }}
          />
          {theme.label}
        </MenuButton>
      </MenuTrigger>
      <MenuPopover>
        <MenuList>
          {allThemes.map((t) => (
            <MenuItemRadio
              key={t.id}
              name="theme"
              value={t.id}
              onClick={() => setThemeId(t.id)}
            >
              <span
                aria-hidden
                style={{
                  display: "inline-block",
                  width: 10,
                  height: 10,
                  borderRadius: "50%",
                  background: t.swatchColor,
                  marginRight: 8,
                  verticalAlign: "middle",
                  border: `1px solid ${tokens.colorNeutralStroke1}`,
                }}
              />
              {t.label}
            </MenuItemRadio>
          ))}
        </MenuList>
      </MenuPopover>
    </Menu>
  );
}
