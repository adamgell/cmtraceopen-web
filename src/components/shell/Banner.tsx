// Dotted-texture banner. Kicker + hostname on the left, monospace chips
// under the hostname, kbd strip pinned to the right. Empty-state renders
// a plain em-dash so the header height stays stable.

import { theme, type PillState } from "../../lib/theme";

export interface BannerDevice {
  deviceId: string;
  lastSeenLabel: string;      // e.g. "11h", "2m", "stale"
  sessionCount: number;
  fileCount: number;
  parseState: string;         // raw string — mapped to pill via mapPillState
}

function mapPillState(raw: string): PillState {
  switch (raw) {
    case "ok": return "ok";
    case "ok-with-fallbacks": return "okFallbacks";
    case "partial": return "partial";
    case "failed": return "failed";
    case "pending": return "pending";
    default: return "pending";
  }
}

interface Props {
  device: BannerDevice | null;
}

export function Banner({ device }: Props) {
  const kicker = device ? "DEVICE · LOGS" : "DEVICE";
  return (
    <div
      data-testid="banner"
      style={{
        padding: "0.5rem 0.9rem",
        borderBottom: `1px solid ${theme.border}`,
        background: theme.bg,
        backgroundImage: theme.pattern.dots,
        display: "flex",
        alignItems: "center",
        gap: "0.9rem",
        fontFamily: theme.font.ui,
        minWidth: 0,
      }}
    >
      <div style={{ display: "flex", flexDirection: "column", gap: "0.15rem" }}>
        <span style={{ fontSize: "0.58rem", letterSpacing: "0.18em", color: theme.accent, textTransform: "uppercase" }}>
          {kicker}
        </span>
        <span style={{ fontSize: "0.95rem", fontWeight: 700, color: theme.textPrimary, letterSpacing: "-0.01em" }}>
          {device ? device.deviceId : "—"}
        </span>
      </div>
      {device && (
        <div
          data-testid="banner-chips"
          style={{
            display: "flex",
            gap: "0.4rem",
            alignItems: "center",
            flexWrap: "nowrap",
            overflow: "hidden",
            minWidth: 0,
            // TODO(task-4-followup): Spec §3 calls for a `…` expander when the
            // chips row overflows. For now we hard-clip so the kbd strip stays
            // visible — the expander UI is a downstream follow-up.
          }}
        >
          <Chip k="LAST SEEN" v={device.lastSeenLabel} />
          <Chip k="SESSIONS" v={String(device.sessionCount)} />
          <Chip k="FILES" v={String(device.fileCount)} />
          <Chip k="PARSE" v={device.parseState} pill={mapPillState(device.parseState)} />
        </div>
      )}
      <div style={{ marginLeft: "auto", display: "flex", gap: "0.7rem", fontFamily: theme.font.mono, fontSize: "0.58rem", color: theme.textDim }}>
        <Kbd k="⌘/" label="focus query" />
        <Kbd k="⌘B" label="rail" />
        <Kbd k="⌘K" label="jump" />
        <Kbd k="⌘↑↓" label="next file" />
      </div>
    </div>
  );
}

function Chip({ k, v, pill }: { k: string; v: string; pill?: PillState }) {
  const color = pill ? theme.pill[pill].fg : theme.text;
  return (
    <span
      style={{
        padding: "0.15rem 0.45rem",
        background: theme.surface,
        border: `1px solid ${theme.border}`,
        borderRadius: 3,
        fontFamily: theme.font.mono,
        fontSize: "0.64rem",
        whiteSpace: "nowrap",
      }}
    >
      <span style={{ color: theme.textDim }}>{k} </span>
      <span style={{ color }}>{v}</span>
    </span>
  );
}

function Kbd({ k, label }: { k: string; label: string }) {
  return (
    <span>
      <b style={{ color: theme.accent, fontWeight: 600 }}>{k}</b> {label}
    </span>
  );
}
