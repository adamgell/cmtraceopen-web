// Ported from desktop (src/components/log-view/DnsWorkspaceBanner.tsx).
//
// Web adaptation:
//   - Zustand store reads (`useLogStore`, `useUiStore`, `useDnsDhcpStore`)
//     and the imperative `useDnsDhcpStore.getState().addSource(...)` call
//     are replaced with explicit props. The caller decides whether the
//     banner is applicable (by not rendering, or by passing a falsy
//     `parserKind`) and owns the "Open in Workspace" action.
//   - `ParserKind` isn't present on the web `LogEntry`; callers typically
//     derive it from `ParseResult.parserSelection.parser`. Typed loosely
//     here so the banner doesn't pull the full desktop enum over.
//   - `@fluentui/react-icons` is NOT a web dependency. The desktop version
//     used `DismissRegular` on the dismiss button; we fall back to a plain
//     text "×" glyph to keep the dependency surface flat.
//
// TODO(web-port): once the web viewer grows a real DNS/DHCP workspace,
// promote parser-kind classification into `lib/log-types.ts` and drop the
// local `DnsParserKind` string-literal alias.

import { useState } from "react";
import { Button } from "@fluentui/react-components";
import { theme } from "../../lib/theme";

/**
 * Parser kinds the banner recognizes. Matches a subset of the desktop
 * `ParserKind` enum — only the DNS/DHCP members the banner offers a
 * workspace jump for. Callers may pass `undefined` (or any other string)
 * to suppress the banner.
 */
export type DnsParserKind = "dnsDebug" | "dnsAudit" | "dhcp";

const PARSER_LABELS: Record<DnsParserKind, string> = {
  dnsDebug: "DNS debug log",
  dnsAudit: "DNS audit log",
  dhcp: "DHCP server log",
};

export interface DnsWorkspaceBannerProps {
  /**
   * Parser identified for the currently open log. If `undefined` (or any
   * value other than a known DNS/DHCP kind), the banner renders nothing.
   */
  parserKind: string | undefined;
  /**
   * Called when the user clicks "Open in Workspace". Caller is responsible
   * for handing the entries to whatever workspace state it maintains and
   * navigating to that workspace. The banner doesn't know about entries.
   */
  onOpenInWorkspace: () => void;
  /**
   * Optional controlled dismissal. When omitted, the banner manages its
   * own dismissed state internally (mirrors desktop behavior).
   */
  dismissed?: boolean;
  onDismiss?: () => void;
}

function isDnsParserKind(v: string | undefined): v is DnsParserKind {
  return v === "dnsDebug" || v === "dnsAudit" || v === "dhcp";
}

export function DnsWorkspaceBanner({
  parserKind,
  onOpenInWorkspace,
  dismissed: dismissedProp,
  onDismiss,
}: DnsWorkspaceBannerProps) {
  const [dismissedLocal, setDismissedLocal] = useState(false);
  const dismissed = dismissedProp ?? dismissedLocal;

  if (!isDnsParserKind(parserKind) || dismissed) {
    return null;
  }

  const label = PARSER_LABELS[parserKind];

  const handleDismiss = () => {
    if (onDismiss) onDismiss();
    else setDismissedLocal(true);
  };

  return (
    <div
      style={{
        display: "flex",
        alignItems: "center",
        gap: 12,
        padding: "6px 12px",
        background: theme.surfaceAlt,
        borderBottom: `1px solid ${theme.border}`,
        fontSize: 13,
        color: theme.text,
        flexShrink: 0,
      }}
    >
      <span>
        This looks like a {label}. Open in the DNS/DHCP workspace for device
        correlation and query analysis?
      </span>
      <Button size="small" appearance="primary" onClick={onOpenInWorkspace}>
        Open in Workspace
      </Button>
      <Button
        size="small"
        appearance="subtle"
        onClick={handleDismiss}
        aria-label="Dismiss"
      >
        {/* Text glyph stand-in for `DismissRegular` — the web repo does
            not depend on @fluentui/react-icons. */}
        ×
      </Button>
    </div>
  );
}
