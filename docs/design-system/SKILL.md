# SKILL — Designing for CMTrace Open Web

> Read this file first when designing anything new for the web viewer.
> The web viewer is **not** the desktop app with a different runtime — it has its own architecture, its own design surfaces, and its own constraints. Don't assume desktop conventions apply unless this file says they do.

---

## What CMTrace Open Web is

A **browser-based log viewer** for CMTrace Open. Logs are parsed client-side via WebAssembly (`cmtrace-wasm`); a thin React shell presents fleets, sessions, and entries to the user. The repo also ships a "command-bridge" mode for live device telemetry over an authenticated API.

Stack: **React 19 + Fluent v9 + Vite + WASM + MSAL** (Entra ID auth optional).

The product promise mirrors the desktop:
> Drop in a log file and start reading. Errors highlight automatically.

The web app extends that with: a fleet rail, session tree, KQL bar, and a command bridge to live devices.

---

## Two design surfaces

This is the most important thing to internalize before designing anything. The web app has **two design surfaces running side-by-side, on purpose**:

### Surface A — The eight Fluent themes (`src/lib/themes/`)
- Used by: `LocalMode`, `DiffView`, the entry grid, dialogs, anything that touches log content.
- Consumes: Fluent v9 design tokens via `FluentProvider`.
- Files: `src/lib/themes/*` — copied as-is from the desktop repo (`~/repo/cmtraceopen`).
- Documented in: `docs/design-system/SKILL.md` § "Visual foundations" + the live design system.
- **CSS variable prefix in `tokens.css`:** `--cmt-*`
- **HTML attribute switch:** `<html data-cmt-theme="light">`

### Surface B — The command-bridge shell (`src/lib/theme.ts`)
- Used by: `Banner`, `TopHeader`, `KqlBar`, `ResultStrip`, `UserChip`, `CommandBridge`, the rail, the middle pane.
- Consumes: a hand-rolled, **dark-only** design language defined in `src/lib/theme.ts`.
- Deliberately **does not** use Fluent tokens. The comment in source says: *"Single source of truth. Every new shell component reads colors / fonts / background patterns from here. Fluent UI's own `tokens.*` is deliberately NOT used by shell components — we want to fully own the look."*
- **CSS variable prefix in `tokens.css`:** `--shell-*` (under `[data-cmt-surface='command-bridge']`)
- **TypeScript object:** `import { theme } from '../../lib/theme'`

### How to decide which surface to use

| If the component… | Use surface |
|---|---|
| Renders log entries, diffs, or any list of log content | **A — Fluent** |
| Lives in a dialog or settings pane | **A — Fluent** |
| Is part of the command-bridge chrome (banner, header, rail, KQL bar, status strips) | **B — Shell** |
| Is part of `LocalMode` or `DiffView` (the standalone-file flows) | **A — Fluent** (these are pre-Task-19; a future cleanup will reconcile) |
| Is brand-new and you're not sure | **Ask.** Don't invent a third surface. Add a TODO referencing this skill and surface it for review. |

### The migration path
The source comment in `theme-context.tsx` says: *"LocalMode and DiffView keep Fluent until the Task 19 cleanup pass."* — meaning the team intends the shell language to expand and Fluent's footprint to shrink over time. **Don't pre-empt that.** Until Task 19 lands, both surfaces are first-class.

---

## Where things live

| You need… | Look here | Mirrored in |
|---|---|---|
| Eight-theme color tokens | `src/lib/themes/*.ts` | `src/styles/tokens.css` (`--cmt-*`), `docs/design-system/tokens.css` |
| Command-bridge shell tokens | `src/lib/theme.ts` | `src/styles/tokens.css` (`--shell-*`) |
| Active theme state + persistence | `src/lib/theme-context.tsx` | n/a — runtime only |
| KQL grammar | `src/lib/kql-lexer.ts`, `kql-schema.ts` | n/a — language def |
| Workspace + fleet data | `src/lib/workspace-context.tsx`, `bridge-state.tsx` | n/a — state |
| WASM bridge | `src/lib/wasm-bridge.ts` | n/a — runtime |
| Designer-facing rules | This file | This file |

---

## Setup (every new component file)

### Surface A (Fluent)
```tsx
import { tokens, makeStyles } from "@fluentui/react-components";

const useStyles = makeStyles({
  root: {
    backgroundColor: tokens.colorNeutralBackground1,
    color: tokens.colorNeutralForeground1,
    fontFamily: tokens.fontFamilyBase,
  },
});
```

### Surface B (Shell)
```tsx
import { theme } from "../../lib/theme";

export function Thing() {
  return (
    <div style={{
      background: theme.bg,
      color: theme.text,
      fontFamily: theme.font.ui,
      borderBottom: `1px solid ${theme.border}`,
    }}>…</div>
  );
}
```

For HTML/standalone prototypes (marketing pages, docs, the design-system cards), link `src/styles/tokens.css` and use the `--cmt-*` or `--shell-*` variables directly.

---

## The seven rules

The desktop app's six rules apply (see `docs/design-system/SKILL.md` if you've never read them — they're inherited verbatim). The web adds one:

### 7. Surface boundaries are real
A component is **either** Fluent **or** shell. Never both. If a component bridges the two surfaces (e.g. a Fluent dialog opened from a shell button), the dialog itself is Fluent and lives inside its own `FluentProvider` subtree. The button stays shell. The seam is intentional.

The remaining six (recap):
1. **Severity colors are non-negotiable defaults** — errors red, warnings amber, info default. Zero-config.
2. **Tabular numbers, always** — `var(--cmt-font-numeric)` for counts, sizes, timestamps.
3. **Density over comfort** — log rows are 23px at 13px font.
4. **Strokes do the work, not shadows** — reserve shadow-8 for popovers, shadow-16 for dialogs.
5. **Brand teal is for accent, not decoration** — only on the canonical surfaces (status bar, primary, active tab, selected sidebar, logo).
6. **Don't soften technical errors** — direct, operational copy. No emoji in product UI.

---

## Web-specific patterns

### KQL bar
The KQL bar is the most opinionated piece of UI in the app. Token rules:
- Identifiers: `theme.text` (shell) — they're highlighted by the lexer, not the styling layer.
- Operators / keywords: `theme.accent`.
- Strings: keep distinct from identifiers and accent — pick a warm neutral and stick with it across the lexer + completion popup.
- Errors in the bar itself are **inline red dots**, never row-coloring — the bar isn't a log grid.

### Pills (parse / health state)
Six states: `ok`, `okFallbacks`, `partial`, `failed`, `pending`, `stale`. Every parse-state surface uses these colors. **Do not invent new states or remap existing ones** — if a new condition arises, extend the enum in `src/lib/theme.ts` and document it here.

### Banner texture
The dotted texture (`theme.pattern.dots`) is reserved for the Banner. Don't apply it elsewhere; it's a wayfinding signal that you're in the command-bridge shell.

### Anonymous vs. authenticated mode
The app boots in two modes (`entraConfig.status === "configured"` or `"anonymous"`). Visually they should be **identical** except:
- The `UserChip` shows "Sign in" (anonymous) or the user's identity + tenant (configured).
- Auth-gated affordances (e.g. "Pin file") are present but disabled in anonymous mode, with a tooltip pointing to sign-in.

Don't fork the design between modes. The mode is a state, not a theme.

### LocalMode + DiffView
These two are the **pre-Task-19 Fluent surfaces**. They use surface A, the eight Fluent themes. When you design within them, follow the desktop SKILL rules verbatim (see `docs/design-system/SKILL.md`). When the Task 19 cleanup happens, they'll move to the shell surface — but until then, treat them as canonical Fluent.

---

## Anti-patterns (hard no)

All the desktop anti-patterns apply, plus these web-specific ones:

- **Mixing surfaces in one component.** A shell component that imports `tokens.colorBrandForeground1` is broken on its face.
- **Adding a third color system.** If `theme.ts` doesn't have what you need, add it to `theme.ts` (and the `--shell-*` block in `tokens.css`) — don't inline a new palette.
- **Light-mode shell.** The shell is dark-only by design. Don't try to "make it work in light." That's a Fluent surface decision.
- **Fluent dialogs styled to match the shell.** Dialogs live in Fluent. If the visual mismatch bothers you, that's a sign Task 19 should be prioritized — surface that to the team, don't paper over it.
- **Loading the WASM bundle from a `useEffect` for design preview purposes.** The design system cards never need real log data; use fixtures from `tests/`.
- **Reading `process.env`** — Vite uses `import.meta.env`. Don't muscle-memory your way into a build break.

---

## When in doubt

- **Surface A?** Look at `src/components/right/EntryGrid.tsx` — densest Fluent surface in the codebase.
- **Surface B?** Look at `src/components/shell/Banner.tsx` — the canonical shell composition.
- **Token drift?** `src/styles/tokens.css` is the human-readable mirror of both surfaces. `src/lib/themes/*.ts` and `src/lib/theme.ts` are the runtime sources of truth — they win on conflict.
- **Default to Light + Teal** for Fluent surfaces; the shell has only one mode.
- **The product is for people who read logs for a living.** Respect their time.
