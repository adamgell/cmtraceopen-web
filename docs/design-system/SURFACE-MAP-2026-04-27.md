# Component Surface Map — 2026-04-27

Classification of every `.tsx` component by design surface.

- **[A]** — Fluent v9 tokens (`tokens.*` from `@fluentui/react-components`)
- **[B]** — Shell tokens (`theme.*` from `src/lib/theme.ts`)
- **[MIXED]** — both (violation)
- **[NEITHER]** — hardcoded only (violation)

| File | Surface | Notes |
|------|---------|-------|
| `components/DropZone.tsx` | [A] | Fluent tokens + components |
| `components/EntryList.tsx` | [MIXED] | Fluent `tokens` + `useTheme()` from theme-context |
| `components/FilterBar.tsx` | [MIXED] | Fluent `tokens` + `useTheme()` from theme-context |
| `components/FindBar.tsx` | [A] | Fluent tokens, Button, Input, Tooltip |
| `components/LocalMode.tsx` | [MIXED] | Fluent `tokens` + `theme` from src/lib/theme |
| `components/log-view/DiffHeader.tsx` | [B] | Shell theme only |
| `components/log-view/DiffView.tsx` | [B] | Shell theme only |
| `components/log-view/DnsWorkspaceBanner.tsx` | [B] | Shell theme + Fluent `Button` component (not tokens) |
| `components/log-view/MergeLegendBar.tsx` | [B] | Shell theme only |
| `components/middle/FleetList.tsx` | [B] | Shell theme |
| `components/middle/MiddlePane.tsx` | [B] | Shell theme |
| `components/middle/SessionTree.tsx` | [B] | Shell theme |
| `components/overlays/HelpOverlay.tsx` | [B] | Shell theme |
| `components/overlays/LocalOverlay.tsx` | [B] | Shell theme |
| `components/rail/DeviceRail.tsx` | [B] | Shell theme |
| `components/rail/DeviceRow.tsx` | [B] | Shell theme |
| `components/rail/SavedViews.tsx` | [B] | Shell theme |
| `components/right/EntryGrid.tsx` | [B] | Shell theme |
| `components/right/FilterBar.tsx` | [B] | Shell theme |
| `components/right/LogViewer.tsx` | [B] | Shell theme |
| `components/right/RowDetail.tsx` | [B] | Shell theme |
| `components/right/StatusBar.tsx` | [B] | Shell theme |
| `components/shell/Banner.tsx` | [B] | Shell theme |
| `components/shell/CommandBridge.tsx` | [B] | Shell theme |
| `components/shell/KqlBar.tsx` | [B] | Shell theme |
| `components/shell/ResultStrip.tsx` | [B] | Shell theme |
| `components/shell/TopHeader.tsx` | [B] | Shell theme |
| `components/shell/UserChip.tsx` | [B] | Shell theme |

## Summary

- **Surface A (Fluent):** 2 pure + 3 mixed = 5
- **Surface B (Shell):** 20 pure
- **Mixed (violation):** 3 — EntryList, FilterBar (root), LocalMode
- **Neither:** 0
