// One row in the device rail. Renders collapsed (icon + 4-char slug) at rail
// width 56px, or expanded (full deviceId + last-seen label) at rail width
// 220px. Both modes are a single <button> so keyboard focus + click-to-select
// behave the same. The `RailDevice` interface is exported because DeviceRail
// (and its test) builds arrays of these before passing them down.

import { theme, type PillState } from "../../lib/theme";

export interface RailDevice {
  deviceId: string;
  lastSeenLabel: string;
  health: PillState;
}

interface Props {
  device: RailDevice;
  expanded: boolean;
  active: boolean;
  onSelect: (deviceId: string) => void;
}

export function DeviceRow({ device, expanded, active, onSelect }: Props) {
  // Collapsed mode shows a 4-char slug built from the first A-Z0-9 run of the
  // deviceId (typically the hostname prefix). Pad with `·` so rows line up
  // visually when a device has a very short id.
  const slug = device.deviceId.replace(/[^A-Z0-9]/g, "").slice(0, 4).padEnd(4, "·");
  const dotColor = theme.pill[device.health].dot;
  const bg = active ? theme.accentBg : "transparent";
  const textColor = active ? theme.accent : theme.text;
  const borderLeft = active ? `2px solid ${theme.accent}` : "2px solid transparent";

  if (!expanded) {
    return (
      <button
        type="button"
        onClick={() => onSelect(device.deviceId)}
        title={`${device.deviceId} · ${device.lastSeenLabel}`}
        style={{
          all: "unset",
          display: "flex",
          flexDirection: "column",
          alignItems: "center",
          gap: "0.2rem",
          width: "44px",
          padding: "0.35rem 0",
          margin: "0 auto",
          background: bg,
          color: textColor,
          borderRadius: 3,
          cursor: "pointer",
          fontFamily: theme.font.mono,
          fontSize: "0.55rem",
        }}
      >
        <span style={{ width: 7, height: 7, borderRadius: "50%", background: dotColor }} />
        <span>{slug}</span>
      </button>
    );
  }

  return (
    <button
      type="button"
      onClick={() => onSelect(device.deviceId)}
      style={{
        all: "unset",
        display: "flex",
        gap: "0.6rem",
        alignItems: "center",
        padding: "0.55rem 0.7rem",
        background: bg,
        color: textColor,
        borderBottom: `1px solid ${theme.surface}`,
        borderLeft,
        cursor: "pointer",
        fontSize: "0.8rem",
      }}
    >
      <span style={{ width: 10, height: 10, borderRadius: "50%", background: dotColor, flexShrink: 0 }} />
      <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>{device.deviceId}</span>
      <span style={{ marginLeft: "auto", fontFamily: theme.font.mono, fontSize: "0.65rem", color: theme.textDim }}>
        {device.lastSeenLabel}
      </span>
    </button>
  );
}
