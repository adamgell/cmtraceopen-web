// Tiny global keyboard shortcut hook.
//
// Subscribes a handler to window `keydown` and fires it when the key + modifier
// combination in `spec` matches. Meta on Mac maps to `metaKey` (⌘) but we
// accept `ctrlKey` as an equivalent so non-Mac users bind Ctrl automatically.
//
// Callers that want to swallow the default browser action (e.g. ⌘/ would open
// Quick Find in some browsers) should call `e.preventDefault()` inside the
// handler — the hook itself never preventDefaults on your behalf.

import { useEffect } from "react";

export interface ShortcutSpec {
  key: string;
  meta?: boolean;
  shift?: boolean;
  alt?: boolean;
}

export function useShortcut(spec: ShortcutSpec, handler: (e: KeyboardEvent) => void) {
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key.toLowerCase() !== spec.key.toLowerCase()) return;
      if (!!spec.meta !== (e.metaKey || e.ctrlKey)) return;
      if (!!spec.shift !== e.shiftKey) return;
      if (!!spec.alt !== e.altKey) return;
      handler(e);
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [spec.key, spec.meta, spec.shift, spec.alt, handler]);
}
