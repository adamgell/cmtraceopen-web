# CMTrace Open Web — Design System

This folder is the **canonical, human-readable design system** for the web viewer.

## Files

- **`SKILL.md`** — read this first when designing anything new. Covers both design surfaces (Fluent + command-bridge shell), where things live, the seven rules, and web-specific anti-patterns.
- **`tokens.css`** — every token for both surfaces, exposed as CSS variables. Source-of-truth mirror of `src/lib/themes/*.ts` (Fluent, prefix `--cmt-*`) **and** `src/lib/theme.ts` (shell, prefix `--shell-*`).

## Two surfaces, on purpose

The web app has two design surfaces running side-by-side. This is intentional:

| Surface | Used by | Source-of-truth |
|---|---|---|
| **Fluent (A)** — eight themes inherited from desktop | Log content, dialogs, `LocalMode`, `DiffView`, entry grid | `src/lib/themes/*.ts` |
| **Command-bridge shell (B)** — dark-only hand-rolled language | `Banner`, `TopHeader`, `KqlBar`, `ResultStrip`, `UserChip`, rail, middle pane | `src/lib/theme.ts` |

`SKILL.md` § "Two design surfaces" walks through how to choose, the migration path (Task 19), and the boundary rules.

## How this relates to the codebase

- **Fluent themes** in `src/lib/themes/` are **copied as-is from the desktop repo** (`~/repo/cmtraceopen`). The same tokens drive both apps. Update there first, copy here second.
- **The shell** in `src/lib/theme.ts` is **web-only**. It does not exist in the desktop app.
- **`tokens.css`** is a documentation artifact + a way for non-Fluent surfaces (marketing pages, the design-system cards, docs sites) to consume the same values.

If `tokens.css` and the TS files disagree, **the TS files win** — update `tokens.css`. There's a sister copy at `src/styles/tokens.css` so drift is visible in code review.

## How to use the system in new work

1. Read `SKILL.md`. The "Two design surfaces" section tells you which surface to extend.
2. For Fluent surfaces, default to **Light theme + Teal brand**. Everything else is a variation.
3. For shell surfaces, there is only one mode — dark, with the mint-teal accent (`#5ee3c5`).
4. If you reach for a hex code that isn't in `tokens.css`, stop. Either it should be added to the system, or you're solving the wrong problem.
5. Use the live design-system project at https://claude.ai/design/p/019dcf99-e7fa-7599-bd90-3158839d5871 for visual reference (the Fluent surfaces; the shell is web-specific and isn't in the cards yet).

## Updating the system

When you change a Fluent theme: update the desktop repo first, then sync `src/lib/themes/` here, then update **both** copies of `tokens.css`.

When you change the shell: update `src/lib/theme.ts` and the `--shell-*` block in **both** copies of `tokens.css`.

When you change a rule, anti-pattern, or surface boundary: update `SKILL.md` here. Mirror to the desktop SKILL.md only if the rule applies cross-app.

## The eight Fluent themes

`light` · `dark` · `high-contrast` · `classic-cmtrace` · `solarized-dark` · `nord` · `dracula` · `hotdog-stand`

Switch in HTML with `<html data-cmt-theme="dark">`. Switch in the app via `useTheme()` from `src/lib/theme-context.tsx`.

## The shell

Single dark surface. To use the shell tokens in HTML or standalone prototypes:

```html
<div data-cmt-surface="command-bridge">…</div>
```

Then reference `--shell-bg`, `--shell-accent`, `--shell-pill-ok-fg`, etc.
