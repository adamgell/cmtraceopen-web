// useDebounce
//
// Tiny dependency-free debounce hook. Returns a value that lags `value` by
// `delayMs` milliseconds; if `value` changes again before the timer fires,
// the timer is reset. Useful for filter inputs where every keystroke should
// not re-trigger a downstream effect (re-filter, re-query, etc.).
//
// 250 ms is the standard for filter UIs — fast enough to feel responsive,
// slow enough that "rapid typing" coalesces into one downstream evaluation.

import { useEffect, useState } from "react";

export function useDebounce<T>(value: T, delayMs = 250): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => {
    const id = setTimeout(() => setDebounced(value), delayMs);
    return () => clearTimeout(id);
  }, [value, delayMs]);
  return debounced;
}
