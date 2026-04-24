# Viewer command-bridge shell

**Status**: Design, pending review
**Author**: Daisy + Claude (brainstorming session 2026-04-24)
**Scope**: UI shell redesign for `cmtraceopen-web`. Theme unification + three-pane dense layout + KQL global query bar (UI only for v1; executor stubbed). mTLS hardening is a separate follow-up spec.

## Motivation

Today the viewer has two unrelated visual personas:

1. **Main app** (ViewerShell with local / api / devices / diff modes): Fluent UI defaults — blue accent, rounded pills, loose padding. Feels like a generic admin console.
2. **DeviceLogViewer overlay** (full-screen, reached via "See logs" on a devices grid row): a deliberately dense "command bridge" aesthetic with a display-typography hostname banner, dotted header, monospace metadata chips, keyboard-shortcut strip, and a teal accent.

The DeviceLogViewer look is better at conveying "this is a forensics console" and the operator said to carry it fleet-wide. Four device-drill pain points motivated the rest of the redesign:

- **B · Device list doesn't scale**. No search, no sort, no filter. A fleet of 50 is already painful.
- **D · Grid is signal-poor**. No at-a-glance health indicator — you can't tell which devices are failing, partial, or silent.
- **E · Session tree is slow/awkward**. Deep nesting, context loss on device switch.
- **F · No cross-device views**. "Show me ccmexec.log across the fleet" and "all devices with parse_state=failed in the last 24h" are both essentially unreachable today.

## Out of scope

- mTLS hardening + testing (next spec)
- Real KQL execution (this spec delivers the UI + a stubbed executor; real parser and query compiler land in a follow-up)
- Agent-side changes. Everything here is viewer-only.
- **Redesign** of DiffView or LocalMode — they stay functionally as-is, reachable via the shell (see §7). A token-adoption styling pass on those two components is in scope as the final build step; their feature surfaces are not.
- Theming for the api-server status page at `:8080` (already colorized in 2026-04-23 work).

## Design

### 1. Shell architecture

A single top-level `<CommandBridge>` component replaces today's ViewerShell-mode-switching branch tree. It owns four stacked regions, with the status bar nested inside the right pane rather than spanning the full width:

```
┌──────────────────────────────────────────────────────────────────────────┐
│  KQL query bar (global, always visible)                                  │
├──────────────────────────────────────────────────────────────────────────┤
│  Device banner (dotted texture, hostname, chips, kbd strip)              │
├──────────┬────────────────┬──────────────────────────────────────────────┤
│  Left    │  Middle pane   │  Right pane (log viewer)                     │
│  rail    │  (device ▾     │                                              │
│  56px    │   fleet tabs)  │  ~72% of viewport width                      │
│ (icon)   │    220px       │                                              │
│          │                ├──────────────────────────────────────────────┤
│          │                │  Status bar (row count, warn/err, shortcuts) │
└──────────┴────────────────┴──────────────────────────────────────────────┘
```

Regions are independently scrollable. On a fresh load the three panes split `56 : 220 : 1fr` (rail collapsed). `⌘B` toggles the rail to expanded mode, changing the split to `220 : 220 : 1fr` without touching the middle and right pane ratios — the right pane loses 164px of width when the rail expands. State is persisted in localStorage so the rail preference survives reloads.

The three panes communicate through a single shell-level store (`useBridgeState()` — a lightweight React context, no Redux) containing: `{ selectedDevice, selectedSession, selectedFile, fleetFilter, kqlQuery, kqlResult, ui: { railExpanded, railSort } }`.

Mode switching disappears. `local` mode (drag-drop a log file) and `diff` mode become shell-level overlays the operator invokes from the KQL bar or a menu — they don't take over the shell. Both are carried forward as-is; they're not redesigned here.

### 2. KQL query bar

**Location**: topmost row. Full width. Monospace input.

**Syntax**: KQL-flavored — pipelined `table | where cond | summarize …` form. Schema (stubbed in v1):

- `DeviceLog` → `sessions` table. Fields: `device_id`, `parse_state`, `ingested_utc`, `size_bytes`, `collected_utc`.
- `File` → `files` table. Fields: `session_id`, `relative_path`, `parser_kind`, `entry_count`, `parse_error_count`.
- `Entry` → `entries` table. Fields: `file_id`, `line_number`, `ts_ms`, `severity`, `component`, `message`.

**Token highlighting** (applied via a tiny client-side lexer, not a general KQL parser):

| Token class | Example | Color |
|---|---|---|
| Table | `DeviceLog` | amber |
| Pipe | `\|` | teal |
| Keyword | `where`, `summarize`, `project`, `extend`, `join`, `take`, `count`, `order by` | purple |
| Field ident | `parse_state` | blue-gray |
| Operator | `==`, `!=`, `>`, `has`, `contains`, `startswith` | teal |
| String literal | `"failed"` | orange |
| Function call | `ago(24h)`, `count()`, `now()` | teal |
| Numeric literal | `24h`, `1024`, `7d` | amber |

**Autocomplete dropdown** (opens on focus, closes on `Esc` or blur). Three sections:

1. **Schema** — resolves based on cursor position: after `where` inside a `DeviceLog` pipeline, suggests `DeviceLog` fields with types + example values.
2. **Recent** — last 10 queries (localStorage-backed, not server-side).
3. **Saved views** — ★-prefixed entries. Clicking a saved view pins it to the left rail under "Saved views".

Docs link at the bottom: `? → KQL quick reference` opens a side panel with the supported syntax subset.

**Actions** (right side of the bar):

- `RUN · ⏎` — primary, teal outline.
- `EXPLAIN` — open a side panel showing the parsed pipeline, inferred schema, and what each stage filters.
- `★ SAVE` — prompt for a name, persist to localStorage for v1.

**Keyboard**: `⌘/` focuses the bar from anywhere. `⏎` runs. `⌘↩` runs without touching the device pane (keeps current device visible while fleet results appear in the middle). `↑/↓` navigate autocomplete. `Esc` closes dropdown.

**Executor in v1**: stubbed. The bar tokenizes and autocompletes but running a query returns canned shape `{ matches: number, devices: number, sessions: number, files: number, groupBy: string }` based on the query's structure — it does NOT actually filter data. The result strip appears, clicking "open in fleet pane →" switches the middle pane's tab to FLEET with a placeholder list. Real execution is a follow-up spec.

**Result strip**: appears below the bar when a query has run. Monospace. Shows counts and a "group by" pill (inferred from `summarize`). The strip is the only UI that proves a run happened; no overlay or notification.

### 3. Device banner

Replaces the current top region of ViewerShell and the DeviceLogViewer header. Always visible when a device is selected.

- **Dotted texture**: radial gradient (12px grid). Render via a CSS `background-image` so no SVG assets.
- **Kicker**: a contextual label (not a mode selector). Derived from the shell store's current selection: `DEVICE · LOGS` when a file is active, `FLEET · QUERY` when the middle pane is in FLEET mode with no file selected, `LOCAL · FILE` when the local-mode overlay is open. 0.58rem, letter-spacing 0.18em, teal.
- **Name**: the device hostname when a device is selected; the query head (`parse_state:failed · 24h`) when in fleet mode; the filename when in local mode; `—` when nothing is selected. 0.95rem, font-weight 700, white.
- **Chips** (compact, single-line, monospace): `LAST SEEN 11h`, `SESSIONS 44`, `FILES 15`, `PARSE <state>`. State chip color matches the pill palette (§4.2). Overflow: hide behind a `…` expander past viewport width — don't wrap. Chips are only rendered when a device is selected; the row is empty otherwise.
- **Kbd strip** (right-aligned): `⌘/ focus query`, `⌘B rail`, `⌘K jump`, `⌘↑↓ next file`. 0.58rem monospace, teal-keys. Always visible. `⌘K` opens the device-rail search palette (see §4.3) — not a generic command palette.

### 4. Left rail (devices)

#### 4.1 Collapsed state (56px, default)

Vertical list of rows. Each row:

- 7px health dot (color encodes `parse_state` of the most-recent session for that device; see §4.2)
- 4-char device slug (first 4 chars of `device_id`, uppercase)

Active row: teal background, teal text, 2px teal left border.

Hover a row: tooltip with full device_id + last_seen + session count. Does NOT expand the rail.

`⌘B` toggles persistent expanded mode.

#### 4.2 Expanded state (220px)

Full row per device:

```
● GELL-01AA310           11h
```

- 10px health dot
- Full device_id (truncate with ellipsis if > 14 chars)
- Right-aligned last-seen delta (`2m`, `11h`, `2d`, `stale`)

**Health-dot semantics**:

| Color | State | Trigger |
|---|---|---|
| Green `#5ee3c5` | `ok` | Most recent session had zero fallbacks |
| Amber `#f3c37f` | `ok-with-fallbacks` | Recent session produced entries with some parser fallback noise (steady state) |
| Orange `#e08a45` | `partial` | Recent session had at least one broken file |
| Red `#f38c8c` | `failed` | Recent session's parse worker crashed |
| Gray `#5a6878` | `stale` | No bundle in > 24h |

The health dot is computed client-side from the device list + the device's most-recent `parse_state`. No new API — use the existing `/v1/devices` response plus `/v1/devices/{id}/sessions?limit=1` per device. This is N+1 for a fleet of N — 50 devices = 51 initial calls. Acceptable for current fleet sizes; a bulk `/v1/devices/health` endpoint is flagged as an open question for larger fleets.

#### 4.3 Header + controls

- Search input (top of rail, visible only when expanded). When the rail is collapsed, `⌘K` first expands the rail and then focuses the search input — the palette IS the rail search, there's no floating popover.
- Section head: `DEVICES · <n>` with a sort indicator (`last-seen` default; alternates: `device-id`, `health`, `session-count`).
- Below the device list, a **Saved views** section: each entry is a ★-prefixed KQL query pinned from the query bar.

### 5. Middle pane (device ↔ fleet)

Tab toggle at top: `DEVICE · <n> sessions` | `FLEET · filter`.

#### 5.1 Device mode

Sessions as collapsed lines (most-recent first). Each session:

- Chevron (▸ collapsed, ▾ expanded)
- `HH:MMZ` ingested_utc (minute resolution)
- `parse_state` tag pill (`ok`/`ok-w/fb`/`partial`/`failed`/`pending`), color-matched to rail
- Right-aligned file count

Clicking the chevron toggles; clicking the line also toggles. Files render one per line under the session:

- File path (truncated mid-string if too long, keeping basename)
- Right-aligned entry count (formatted `1.3M` / `4.7k` / `522`)

Active file: teal background, teal text, 2px teal left border.

Session-level filter above the list: `filter files…` input. Filters by filename substring (case-insensitive) across all sessions of the current device. Filter chip cleared on device switch.

#### 5.2 Fleet mode

Triggered by clicking the `FLEET` tab OR by "open in fleet pane →" from the KQL result strip.

Shows a flat list of matches (session rows or file rows depending on the KQL query's terminal stage). Each row:

- Device ID (teal) · file path · parse_state tag · elapsed time

Clicking a row pins the device in the left rail (if not already) and loads the entry into the right pane. The fleet list stays in the middle pane — it's cleared ONLY by one of: (a) clicking the `DEVICE` tab, (b) running a new KQL query that replaces the result set, (c) pressing `Esc` while the fleet tab has focus. Provides breadcrumb back-and-forth between fleet query and specific device.

### 6. Right pane (log viewer)

The dense log grid. Six columns:

| Col | Width | Content |
|---|---|---|
| Severity glyph | 22px | `·` info, `⚠` warn, `✖` error |
| Line # | 52px | Right-aligned, dim |
| Timestamp | 156px | `YY-MM-DD HH:MM:SS.sss` or `HH:MM:SS.sss` if same-day as previous row |
| Component | 130px | Teal, truncate with ellipsis |
| Severity label | 56px | `INFO`/`WARN`/`ERROR` color-coded |
| Message | 1fr | Monospace, no wrap, ellipsis on overflow; full content in row-detail panel |

Row styling:

- 0.7rem monospace, line-height 1.28, padding 0.12rem 0.7rem
- Alternating zebra (`#0d1218` on even rows)
- `warn` rows: amber tint `rgba(243,195,127,.06)`
- `err` rows: red tint `rgba(243,140,140,.08)`

**File crumb** (top of right pane): `<device> / <session-time> / <filepath>` with metadata on the right (entry count, parse errors, file size).

**Filter bar** below the crumb: severity pills (Info/Warn/Error toggleable, defaults all on; Error is highlighted with the red pill variant), search input, component selector, match counter (`500 / 1.3M`).

**Status bar** (bottom of right pane — right-pane-local, NOT full shell width; see §1 ASCII): `rows 28/500 · 1.3M total · 2 warn · 2 err · 24h window · ⌘↑↓ next file · J/K row · / find`.

**Keyboard**:

- `J` / `K` — row down / up
- `/` — focus in-pane search
- `⌘↑` / `⌘↓` — previous / next file in the current session
- `Enter` on a row — open row-detail panel (slides out from the right; shows full message + extras JSON)

### 7. Migration from current ViewerShell

- `ViewerShell` → `CommandBridge` (new component, similar imports). Delete the `mode` state machine (`"local" | "api" | "devices" | "diff"`).
- `DeviceLogViewer` → deleted after its rail/session/file logic migrates into the new shell's middle pane. Shares `dtoToEntry` and `EntryList` (already extracted).
- `ApiMode` entry-fetching logic → moves into the right-pane controller.
- `LocalMode` → stays. Invoked via a global drag-and-drop handler on the shell root (dropping a `.log`/`.cmtlog`/`.txt` file anywhere on the shell opens LocalMode as a full-screen overlay) OR via `⌘O` which shows the File Picker from `src/lib/file-pickers.ts`. The overlay covers the shell; `Esc` dismisses back to the previous shell state. Not redesigned; just reachable without a mode switch.
- `FilterBar` → inlined into the right pane's filter bar (it's already the same chips + severity toggle; rename and hard-code to right-pane ownership).
- `FilesPanel` → deleted. Middle pane owns the files list.
- `Toolbar` → deleted. Keyboard shortcuts + the KQL bar replace it. Theme-picker (if present) moves to a shell-level menu behind `⌘,`.

The existing WASM integration (local file parsing) stays untouched. The shared `dto-to-entry.ts` and `log-types.ts` remain the contract between wire and UI.

### 8. Theme tokens

Extract to `src/lib/theme.ts` (new). Single source of truth; all new components read from here:

```ts
export const theme = {
  bg: "#0b0f14",
  bgDeep: "#070a0e",
  surface: "#11161d",
  surfaceAlt: "#0d1218",
  border: "#1f2a36",
  textPrimary: "#f3f7fb",
  text: "#c7d1dd",
  textDim: "#7da2c3",
  textFainter: "#3d4a5a",
  accent: "#5ee3c5",      // teal
  accentBg: "#0e2d22",
  pill: {
    ok: { fg: "#5ee3c5", bg: "#0e2d22", dot: "#5ee3c5" },
    okFallbacks: { fg: "#f3c37f", bg: "#3d2e12", dot: "#f3c37f" },
    partial: { fg: "#e08a45", bg: "#3d2516", dot: "#e08a45" },
    failed: { fg: "#f38c8c", bg: "#3d1414", dot: "#f38c8c" },
    pending: { fg: "#7da2c3", bg: "#1f2a36", dot: "#5a6878" },
    stale: { fg: "#7da2c3", bg: "#1f2a36", dot: "#5a6878" },
  },
  font: {
    mono: "ui-monospace, Menlo, Consolas, monospace",
    ui: "ui-sans-serif, -apple-system, BlinkMacSystemFont, Segoe UI, sans-serif",
  },
  pattern: {
    dots: "radial-gradient(#1c2735 1px, transparent 1px) 0 0 / 12px 12px",
  },
};
```

Dark mode is the default (and only) mode for v1 — the DeviceLogViewer aesthetic doesn't have a light variant and adding one is out of scope. If someone complains, that's a v3 problem.

Fluent UI tokens are dropped from the new shell components. Existing viewer components (LocalMode, DiffView) continue to use Fluent until migrated.

### 9. Error handling

- **Device list fetch failure**: rail shows "fleet unreachable" banner at the top with a retry button. Does not crash the shell.
- **Session fetch failure** (specific device): middle pane shows an inline error band in the session list area, still allows device switching.
- **Entry fetch failure** (specific file): right pane shows the error as a monospace banner above the grid, preserves the filter bar and status bar.
- **KQL parse error** (v1, even stubbed): tokens highlight in red, result strip shows `syntax error at position N: <message>`. Does not clear recent queries.
- **Browser incompatibility**: Safari / Firefox fallbacks for File System Access API (already handled in `src/lib/file-pickers.ts`) — no new work.

No loading spinners in the main chrome. Skeleton rows (3 rail slots, 4 session slots, 10 grid rows) render during initial fetch. Subsequent fetches just swap content without a chrome reset.

### 10. Testing

- **Component tests** (vitest + testing-library): every new component gets a `<Component>.test.tsx`. Target: smoke render, keyboard shortcuts, and one error path.
- **Integration**: `CommandBridge.test.tsx` mounts the whole shell with a mocked api-client, confirms device select → session expand → file open → entries render flow.
- **Visual regression**: out of scope for v1 — no Percy/Chromatic. Flagged as a separate-spec concern in the Open Questions list.
- **Accessibility**: all interactive elements (pills, rows, tabs) get aria-label + role. Keyboard navigation asserts: `⌘B`, `⌘K`, `⌘/`, `J`/`K`, `⌘↑/↓`, `Esc`. The dense grid's scroll follows focus.
- **Manual**: GELL-01AA310 session on local dev (has the SecureBoot + ccmexec data I already verified).

## Build order

Incrementally shippable. Each step merges to main and deploys.

1. **Theme tokens + palette extraction**. New `src/lib/theme.ts`. No UI change yet. Existing components don't consume it until step 2.
2. **CommandBridge shell skeleton**: the 3-pane grid with hardcoded content, plus banner + kbd strip. Wires no data. Gated behind `URLSearchParams.get("v") === "next"` (no router dependency; the existing viewer is already a no-router SPA) so the old ViewerShell stays reachable until cutover.
3. **Left rail (collapsed + expanded)** with live device fetches and health dots.
4. **Middle pane — device mode**: sessions + files, matching the existing DeviceLogViewer data flow.
5. **Right pane — log grid**: port EntryList into the dense 6-column layout, status bar.
6. **KQL bar (UI + stubbed executor)**: tokens, autocomplete with static schema, recent/saved in localStorage, canned result strip.
7. **Middle pane — fleet mode**: consumes the stubbed executor's result shape, renders the flat match list.
8. **Row-detail panel** (`Enter` on a log row): slide-out with extras JSON.
9. **Keyboard shortcut pass**: all shortcuts from the spec, with a help overlay behind `?`.
10. **Cutover**: delete old ViewerShell, DeviceLogViewer, FilesPanel, Toolbar. `/?v=next` becomes `/`.
11. **Cleanup**: migrate LocalMode and DiffView styling to the theme tokens so they stop looking out-of-place.

mTLS hardening + real KQL executor follow in separate specs after step 10 lands.

## Open questions (call out, don't block)

- **Saved view storage**: localStorage for v1 works for single-operator dev. A team would want server-side persistence. Not in scope; will fall out of a later api-server spec that adds a `/v1/saved_views` surface.
- **KQL executor boundary**: the real executor could live client-side (translating to existing REST filters) or server-side (new `/v1/query` endpoint compiling to SQL). No decision this spec. UI is identical either way.
- **Device-health bulk endpoint**: N+1 per-device session fetch for health dots works for <100-device fleets. A bulk `/v1/devices/health` (returning `(device_id, most_recent_parse_state, most_recent_ingested_utc)` tuples) would collapse that to a single call. Defer until a fleet has enough devices to matter.
- **Visual regression testing**: no Percy / Chromatic / Storybook-snapshot wiring today. Worth its own spec once the new shell stabilizes — catches unintended layout drift on dense-grid changes, which this spec is particularly vulnerable to.
- **Theme light mode**: if someone wants it, pair with tokens redesign. Deferred.
- **Viewer on `:5173` vs `:8083`**: containerized viewer stays canonical. Dev mode on `:5173` continues to work; theme tokens are shared.
