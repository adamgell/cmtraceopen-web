// FilesPanel
//
// Step 3 of the API-mode cascade: devices → sessions → *files* → entries.
// Given a selected session, lists the files ingested as part of it and
// lets the operator pick one so entries can be loaded scoped to `?file=...`.
//
// Visual pattern mirrors the Sessions list inside ApiMode: a scrollable
// <ul> of RowButtons showing relative path + a meta line (size, parser,
// entry count). Colors flow through Fluent tokens so dark/classic/HC all
// render correctly.

import { Button, tokens } from "@fluentui/react-components";
import type { SessionFile } from "../lib/log-types";

type FetchState<T> =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "ok"; data: T }
  | { status: "error"; error: string };

export interface FilesPanelProps {
  state: FetchState<SessionFile[]>;
  selected: string | null;
  onSelect: (fileId: string) => void;
}

export function FilesPanel({ state, selected, onSelect }: FilesPanelProps) {
  if (state.status === "loading")
    return <CenteredText text="Loading files…" muted />;
  if (state.status === "error") return <ErrorText error={state.error} />;
  if (state.status === "idle") return null;
  if (state.data.length === 0)
    return <EmptyHint text="No files in this session." />;

  return (
    <ul
      style={{
        listStyle: "none",
        margin: 0,
        padding: 0,
        overflow: "auto",
        flex: 1,
        minHeight: 0,
      }}
    >
      {state.data.map((f) => (
        <li key={f.fileId}>
          <FileRow
            file={f}
            selected={selected === f.fileId}
            onSelect={() => onSelect(f.fileId)}
          />
        </li>
      ))}
    </ul>
  );
}

function FileRow({
  file,
  selected,
  onSelect,
}: {
  file: SessionFile;
  selected: boolean;
  onSelect: () => void;
}) {
  // Fluent's Button looks too "chrome-y" for a full-bleed list row. Keep a
  // native button here but token-color it so it still adapts to the theme —
  // matches the sibling row treatment in the Devices / Sessions lists.
  return (
    <Button
      appearance="subtle"
      onClick={onSelect}
      aria-pressed={selected}
      title={file.relativePath}
      style={{
        width: "100%",
        display: "flex",
        flexDirection: "column",
        alignItems: "flex-start",
        gap: 2,
        padding: "6px 10px",
        borderRadius: 0,
        borderTop: "none",
        borderLeft: "none",
        borderRight: "none",
        borderBottom: `1px solid ${tokens.colorNeutralStroke1}`,
        background: selected
          ? tokens.colorNeutralBackground1Selected
          : "transparent",
        color: tokens.colorNeutralForeground1,
        textAlign: "left",
        justifyContent: "flex-start",
        minWidth: 0,
      }}
    >
      <span
        style={{
          fontFamily: "ui-monospace, Menlo, Consolas, monospace",
          fontSize: 12,
          fontWeight: 500,
          color: tokens.colorNeutralForeground1,
          overflow: "hidden",
          textOverflow: "ellipsis",
          whiteSpace: "nowrap",
          maxWidth: "100%",
        }}
      >
        {file.relativePath}
      </span>
      <span
        style={{
          color: tokens.colorNeutralForeground2,
          fontSize: 11,
          fontWeight: 400,
        }}
      >
        {formatFileMeta(file)}
      </span>
    </Button>
  );
}

function CenteredText({ text, muted }: { text: string; muted?: boolean }) {
  return (
    <div
      style={{
        padding: 14,
        color: muted
          ? tokens.colorNeutralForeground3
          : tokens.colorNeutralForeground1,
        fontSize: 13,
      }}
    >
      {text}
    </div>
  );
}

function EmptyHint({ text }: { text: string }) {
  return (
    <div
      style={{
        padding: 12,
        color: tokens.colorNeutralForeground3,
        fontSize: 13,
      }}
    >
      {text}
    </div>
  );
}

function ErrorText({ error }: { error: string }) {
  return (
    <div
      style={{
        margin: 10,
        padding: "10px 12px",
        background: tokens.colorNeutralBackground2,
        border: `1px solid ${tokens.colorNeutralStroke1}`,
        color: tokens.colorPaletteRedForeground1,
        borderRadius: tokens.borderRadiusMedium,
        fontSize: 13,
        whiteSpace: "pre-wrap",
      }}
    >
      {error}
    </div>
  );
}

function formatFileMeta(f: SessionFile): string {
  const parts: string[] = [];
  parts.push(formatBytes(f.sizeBytes));
  if (f.entryCount > 0) {
    parts.push(
      `${f.entryCount.toLocaleString()} entr${f.entryCount === 1 ? "y" : "ies"}`,
    );
  }
  if (f.parseErrorCount > 0) {
    parts.push(
      `${f.parseErrorCount} parse error${f.parseErrorCount === 1 ? "" : "s"}`,
    );
  }
  if (f.parserKind) parts.push(f.parserKind);
  // `first_line_utc` / `last_line_utc` aren't currently emitted by the
  // api-server's FileSummary, but tolerate them if a future server adds them.
  if (f.firstLineUtc && f.lastLineUtc) {
    parts.push(`${formatUtc(f.firstLineUtc)} → ${formatUtc(f.lastLineUtc)}`);
  } else if (f.firstLineUtc) {
    parts.push(`from ${formatUtc(f.firstLineUtc)}`);
  } else if (f.lastLineUtc) {
    parts.push(`to ${formatUtc(f.lastLineUtc)}`);
  }
  return parts.join(" · ");
}

/** Humanize a byte count into a compact IEC-style string (KiB / MiB). */
function formatBytes(n: number): string {
  if (!Number.isFinite(n) || n < 0) return "?";
  if (n < 1024) return `${n} B`;
  const units = ["KiB", "MiB", "GiB", "TiB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  const precision = v >= 10 ? 0 : 1;
  return `${v.toFixed(precision)} ${units[i]}`;
}

function formatUtc(iso: string): string {
  return iso.replace("T", " ").replace(/\.\d+Z?$/, "").replace(/Z$/, "");
}
