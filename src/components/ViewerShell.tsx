import { useCallback, useState } from "react";
import {
  Button,
  Menu,
  MenuButton,
  MenuItemRadio,
  MenuList,
  MenuPopover,
  MenuTrigger,
  TabList,
  Tab,
  tokens,
  type SelectTabData,
  type SelectTabEvent,
  type TabValue,
} from "@fluentui/react-components";
import type { ParseResult } from "../lib/log-types";
import { LocalMode } from "./LocalMode";
import { ApiMode } from "./ApiMode";
import { DevicesPanel } from "./DevicesPanel";
import { AuthSettings } from "./AuthSettings";
import { useTheme } from "../lib/theme-context";

type Mode = "local" | "api" | "devices";

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
        ) : (
          <ApiMode />
        )}
      </main>
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
  const onTabSelect = (_e: SelectTabEvent, data: SelectTabData) => {
    onModeChange(data.value as Mode);
  };

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
      <TabList
        selectedValue={mode as TabValue}
        onTabSelect={onTabSelect}
        size="small"
      >
        <Tab value="local">Local</Tab>
        <Tab value="api">API</Tab>
        <Tab value="devices">Devices</Tab>
      </TabList>
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
