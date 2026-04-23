import { useCallback, useMemo, useRef, useState } from "react";
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
function DiffMode() {
  const { items } = useWorkspace();
  const [sourceAId, setSourceAId] = useState<string | null>(null);
  const [sourceBId, setSourceBId] = useState<string | null>(null);
  const [stub, setStub] = useState<string | null>(null);

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
          setStub(null);
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
                  onClick={() =>
                    setStub(
                      `Diff between ${sourceA!.label} and ${sourceB!.label} will render here once the two-source entry loader lands.`,
                    )
                  }
                >
                  Load diff
                </Button>
              </div>
            </>
          )}

          {stub && (
            <div
              style={{
                fontSize: tokens.fontSizeBase300,
                color: tokens.colorNeutralForeground2,
                padding: 12,
                background: tokens.colorNeutralBackground1,
                border: `1px solid ${tokens.colorNeutralStroke1}`,
                borderRadius: tokens.borderRadiusMedium,
              }}
            >
              {stub}
            </div>
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
