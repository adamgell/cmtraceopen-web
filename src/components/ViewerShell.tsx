import { useCallback, useState } from "react";
import {
  Button,
  Menu,
  MenuButton,
  MenuItemRadio,
  MenuList,
  MenuPopover,
  MenuTrigger,
  tokens,
} from "@fluentui/react-components";
import type { ParseResult } from "../lib/log-types";
import { LocalMode } from "./LocalMode";
import { ApiMode } from "./ApiMode";
import { DevicesPanel } from "./DevicesPanel";
import { AuthSettings } from "./AuthSettings";
import { StatusBar } from "./StatusBar";
import { TabStrip } from "./layout/TabStrip";
import { Toolbar } from "./layout/Toolbar";
import { DiffView } from "./log-view/DiffView";
import { useTheme } from "../lib/theme-context";

type Mode = "local" | "api" | "devices" | "diff";

const MODE_TABS = [
  { id: "local", label: "Local" },
  { id: "api", label: "API" },
  { id: "devices", label: "Devices" },
  { id: "diff", label: "Diff" },
] as const;

interface LoadedSummary {
  fileName: string;
  result: ParseResult;
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

  const handleLoaded = useCallback((info: LoadedSummary | null) => {
    setLoaded(info);
  }, []);

  const handleModeChange = useCallback((next: Mode) => {
    setMode(next);
    if (next !== "local") setLoaded(null);
  }, []);

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
      {/* Toolbar: mode-specific actions. For now only local-mode Close
         is wired; Open/Reload/Find etc. lift into a follow-up once the
         mode components expose those as imperative handles. */}
      {mode === "local" && loaded && (
        <Toolbar
          onClear={() => setLoaded(null)}
          canClear={true}
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
          <LocalMode onLoaded={handleLoaded} />
        ) : mode === "devices" ? (
          <DevicesPanel />
        ) : mode === "diff" ? (
          <DiffPlaceholder />
        ) : (
          <ApiMode />
        )}
      </main>
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
 * Diff mode placeholder.
 *
 * DiffView is landed as a standalone component (src/components/log-view/
 * DiffView.tsx) but the web viewer has no multi-file workspace yet, so
 * there's no source A / source B to feed it. When the workspace concept
 * lands (loaded sessions can be pinned as "diff targets"), this placeholder
 * gets replaced with a real two-session picker that hands the DiffView
 * two { entries, label } pairs.
 */
function DiffPlaceholder() {
  // Reference DiffView so the bundler keeps it imported even before the
  // placeholder is replaced with a real integration. Once that lands the
  // placeholder goes away entirely.
  void DiffView;
  return (
    <div
      style={{
        flex: 1,
        display: "flex",
        alignItems: "center",
        justifyContent: "center",
        color: tokens.colorNeutralForeground2,
        border: `1px dashed ${tokens.colorNeutralStroke1}`,
        borderRadius: tokens.borderRadiusMedium,
        background: tokens.colorNeutralBackground2,
        padding: 32,
        textAlign: "center",
      }}
    >
      <div>
        <div
          style={{
            fontWeight: 600,
            marginBottom: 4,
            color: tokens.colorNeutralForeground1,
          }}
        >
          Diff mode — coming soon
        </div>
        <div style={{ fontSize: tokens.fontSizeBase200 }}>
          Pick two sessions or files to compare side-by-side once the
          workspace pinboard ships. The underlying DiffView component is
          already in place; this tab will swap from a placeholder to the
          real picker when the two-source selection model lands.
        </div>
      </div>
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
