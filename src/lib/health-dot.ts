// Given a device's most-recent parse_state + last_seen_utc, decide which
// health-dot color to render on the left rail.
//
// Rule: a device is `stale` if it hasn't shipped a bundle in > 24h, even
// if the most recent bundle was clean. Freshness dominates state because
// a "green dot · last seen 2 days ago" is a misleading signal for ops.

import type { PillState } from "./theme";

const STALE_MS = 24 * 3600 * 1000;

export interface HealthInput {
  parseState: string;
  lastSeenMs: number | null;
}

export function deriveHealth(input: HealthInput, nowMs: number): PillState {
  if (input.lastSeenMs == null) return "stale";
  if (nowMs - input.lastSeenMs > STALE_MS) return "stale";
  switch (input.parseState) {
    case "ok": return "ok";
    case "ok-with-fallbacks": return "okFallbacks";
    case "partial": return "partial";
    case "failed": return "failed";
    case "pending": return "pending";
    default: return "pending";
  }
}
