// Workspace pinboard state for the web viewer.
//
// Owns the list of "pinned" references the operator has curated across the
// app -- local files opened from disk, remote API sessions, and individual
// files inside those sessions -- plus which of them is currently "active"
// (the one the main view shows, analogous to VS Code's active editor).
//
// This module is intentionally UI-free. The provider and hook exported here
// are what FileSidebar, Diff mode, and various "pin" buttons elsewhere will
// consume once the UI is wired up. A small set of pure helpers is exported
// alongside so the core state transitions can be unit-tested without React.
//
// Persistence: the entire `WorkspaceState` is serialized to localStorage
// under `cmtraceopen-web.workspace`. The payload is tiny so we write on
// every change rather than debouncing. Parse failures fall back to the
// empty state silently.

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import type { ReactElement, ReactNode } from "react";
import type { LogEntry } from "./log-types";

const STORAGE_KEY = "cmtraceopen-web.workspace";

/** Stable id for a pinned workspace item. */
export type PinnedItemId = string;

/** One pinned reference the operator has curated. */
export type PinnedItem =
  | {
      id: PinnedItemId;
      kind: "local-file";
      /** File name, shown in the sidebar. */
      label: string;
      /** Full local file path/name. */
      fileName: string;
      addedAtMs: number;
    }
  | {
      id: PinnedItemId;
      kind: "api-session";
      /** Display label, e.g. "DEV-WINARM64-01 / 019db7b-...-fd". */
      label: string;
      deviceId: string;
      sessionId: string;
      addedAtMs: number;
    }
  | {
      id: PinnedItemId;
      kind: "api-file";
      /** Display label, e.g. "DEV-WINARM64-01 / session-id / ccmexec.log". */
      label: string;
      deviceId: string;
      sessionId: string;
      fileId: string;
      relativePath: string;
      addedAtMs: number;
    };

/** Distribute `Omit` across the union members so each kind retains its
 *  discriminant-specific fields. The plain `Omit<PinnedItem, ...>` would
 *  collapse to the intersection of common keys and strip `fileName`,
 *  `sessionId`, etc. */
type DistributiveOmit<T, K extends keyof T> = T extends unknown ? Omit<T, K> : never;

/** Shape of the input accepted by `pin` / `findExisting` -- the fields the
 *  caller supplies. `id` and `addedAtMs` are filled in by the provider. */
export type PinnedItemInput = DistributiveOmit<PinnedItem, "id" | "addedAtMs">;

/** Persisted workspace state -- the pinned items plus the active pointer. */
export interface WorkspaceState {
  items: PinnedItem[];
  /** Which item (if any) is currently the "active" one -- drives which
   *  pinned item the main view shows. Mirrors VS Code's active editor. */
  activeId: PinnedItemId | null;
}

/** Imperative API exposed to consumers via `useWorkspace`. */
export interface WorkspaceApi {
  /** Pin a new item. Idempotent: if an item with the same natural key is
   *  already pinned, no duplicate is inserted and the existing id is
   *  returned. */
  pin: (item: PinnedItemInput) => PinnedItemId;
  /** Remove a pinned item by id. If it was the active item, activeId is
   *  cleared. */
  unpin: (id: PinnedItemId) => void;
  /** Set (or clear) which pinned item is currently active. */
  setActive: (id: PinnedItemId | null) => void;
  /** Is `item` already pinned? Matches on the natural key (sessionId for
   *  api-session, fileId for api-file, fileName for local-file). Returns
   *  the existing pin's id, or null. */
  findExisting: (item: PinnedItemInput) => PinnedItemId | null;
  /** Remove every pinned item. Doesn't touch activeId tracking (activeId
   *  will no longer match anything, but that's fine -- consumers already
   *  have to tolerate a stale id during removal races). */
  clear: () => void;
  /** Attach parsed entries to a local-file pin so Diff mode can reach
   *  them without re-parsing. Held in memory only (not persisted) because
   *  entries can be huge; a page reload drops the cache and the operator
   *  re-pins the file to refill it. No-op for non-local-file kinds. */
  setLocalEntries: (id: PinnedItemId, entries: readonly LogEntry[]) => void;
  /** Read the in-memory entries attached to a local-file pin. Returns
   *  `null` when the pin doesn't exist, isn't a local-file, or its
   *  entries haven't been cached this session. */
  getLocalEntries: (id: PinnedItemId) => readonly LogEntry[] | null;
}

/** Combined state + API shape returned by `useWorkspace`. */
export interface WorkspaceContextValue extends WorkspaceState, WorkspaceApi {}

// ---------------------------------------------------------------------------
// Pure helpers -- exported for unit testing.
// ---------------------------------------------------------------------------

/** The empty workspace state. */
export const EMPTY_WORKSPACE_STATE: WorkspaceState = {
  items: [],
  activeId: null,
};

/** Match a candidate against existing pins using the natural key for its
 *  kind. Returns the existing pin's id, or null when no match. */
export function matchExisting(
  items: readonly PinnedItem[],
  candidate: PinnedItemInput,
): PinnedItemId | null {
  for (const existing of items) {
    if (existing.kind !== candidate.kind) continue;
    switch (candidate.kind) {
      case "local-file":
        if (existing.kind === "local-file" && existing.fileName === candidate.fileName) {
          return existing.id;
        }
        break;
      case "api-session":
        if (existing.kind === "api-session" && existing.sessionId === candidate.sessionId) {
          return existing.id;
        }
        break;
      case "api-file":
        if (existing.kind === "api-file" && existing.fileId === candidate.fileId) {
          return existing.id;
        }
        break;
    }
  }
  return null;
}

/** Generate a new pin id. Prefers `crypto.randomUUID` and falls back to a
 *  Math.random-based id for environments where it's unavailable (very old
 *  browsers, certain non-secure contexts). */
function newPinId(): PinnedItemId {
  const c: Crypto | undefined =
    typeof globalThis !== "undefined" ? globalThis.crypto : undefined;
  if (c && typeof c.randomUUID === "function") {
    return c.randomUUID();
  }
  // Fallback: not cryptographically strong, but good enough to disambiguate
  // pins in a single browser profile.
  return `pin-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 10)}`;
}

/** Return a new state with `item` added. Idempotent: if an item matching
 *  `item`'s natural key already exists, the state is returned unchanged and
 *  the existing pin's id is reported. */
export function addPin(
  state: WorkspaceState,
  item: PinnedItemInput,
  now: number = Date.now(),
  idFactory: () => PinnedItemId = newPinId,
): { state: WorkspaceState; id: PinnedItemId } {
  const existingId = matchExisting(state.items, item);
  if (existingId !== null) {
    return { state, id: existingId };
  }
  const id = idFactory();
  const newItem = { ...item, id, addedAtMs: now } as PinnedItem;
  return {
    state: { ...state, items: [...state.items, newItem] },
    id,
  };
}

/** Return a new state with the pin matching `id` removed. If the removed
 *  pin was active, `activeId` is cleared. */
export function removePin(state: WorkspaceState, id: PinnedItemId): WorkspaceState {
  const nextItems = state.items.filter((it) => it.id !== id);
  if (nextItems.length === state.items.length) {
    // Nothing removed -- return the original reference so React can bail out.
    return state;
  }
  return {
    items: nextItems,
    activeId: state.activeId === id ? null : state.activeId,
  };
}

// ---------------------------------------------------------------------------
// Persistence.
// ---------------------------------------------------------------------------

/** Narrow an unknown value to a valid `PinnedItem`, or return null. Used
 *  only when rehydrating from localStorage so a malformed entry for one
 *  kind doesn't poison the rest of the list. */
function parsePinnedItem(raw: unknown): PinnedItem | null {
  if (!raw || typeof raw !== "object") return null;
  const r = raw as Record<string, unknown>;
  if (
    typeof r.id !== "string" ||
    typeof r.label !== "string" ||
    typeof r.addedAtMs !== "number"
  ) {
    return null;
  }
  switch (r.kind) {
    case "local-file":
      if (typeof r.fileName !== "string") return null;
      return {
        id: r.id,
        kind: "local-file",
        label: r.label,
        fileName: r.fileName,
        addedAtMs: r.addedAtMs,
      };
    case "api-session":
      if (typeof r.deviceId !== "string" || typeof r.sessionId !== "string") return null;
      return {
        id: r.id,
        kind: "api-session",
        label: r.label,
        deviceId: r.deviceId,
        sessionId: r.sessionId,
        addedAtMs: r.addedAtMs,
      };
    case "api-file":
      if (
        typeof r.deviceId !== "string" ||
        typeof r.sessionId !== "string" ||
        typeof r.fileId !== "string" ||
        typeof r.relativePath !== "string"
      ) {
        return null;
      }
      return {
        id: r.id,
        kind: "api-file",
        label: r.label,
        deviceId: r.deviceId,
        sessionId: r.sessionId,
        fileId: r.fileId,
        relativePath: r.relativePath,
        addedAtMs: r.addedAtMs,
      };
    default:
      return null;
  }
}

/** Load persisted workspace state from localStorage. Returns the empty
 *  state when running outside the browser, when nothing is stored, or
 *  when the stored value fails to parse. */
function loadInitialState(): WorkspaceState {
  if (typeof window === "undefined") return EMPTY_WORKSPACE_STATE;
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return EMPTY_WORKSPACE_STATE;
    const parsed = JSON.parse(raw) as unknown;
    if (!parsed || typeof parsed !== "object") return EMPTY_WORKSPACE_STATE;
    const p = parsed as Record<string, unknown>;
    const itemsRaw = Array.isArray(p.items) ? p.items : [];
    const items: PinnedItem[] = [];
    for (const candidate of itemsRaw) {
      const item = parsePinnedItem(candidate);
      if (item) items.push(item);
    }
    const activeId =
      typeof p.activeId === "string" && items.some((it) => it.id === p.activeId)
        ? p.activeId
        : null;
    return { items, activeId };
  } catch {
    return EMPTY_WORKSPACE_STATE;
  }
}

/** Write `state` to localStorage. Silently ignores any write failure --
 *  quota exceeded, disabled storage, SSR, etc. */
function persistState(state: WorkspaceState): void {
  if (typeof window === "undefined") return;
  try {
    window.localStorage.setItem(STORAGE_KEY, JSON.stringify(state));
  } catch {
    // Storage is best-effort; the in-memory state remains authoritative.
  }
}

// ---------------------------------------------------------------------------
// Context + provider.
// ---------------------------------------------------------------------------

const WorkspaceContext = createContext<WorkspaceContextValue | null>(null);

/** Provider -- wrap the app at the same level as `ThemeProvider`. */
export function WorkspaceProvider({ children }: { children: ReactNode }): ReactElement {
  const [state, setState] = useState<WorkspaceState>(loadInitialState);

  // Persist on every change. The payload is tiny (a handful of items at
  // most), so we skip debouncing.
  useEffect(() => {
    persistState(state);
  }, [state]);

  const pin = useCallback((item: PinnedItemInput): PinnedItemId => {
    let assignedId: PinnedItemId = "";
    setState((prev) => {
      const result = addPin(prev, item);
      assignedId = result.id;
      return result.state;
    });
    return assignedId;
  }, []);

  // Ephemeral, in-memory only: entries for local-file pins so Diff
  // mode can reach them. Kept in a ref so updates don't trigger a
  // re-render of every workspace consumer. Dropped on page reload by
  // design -- the operator re-pins to refill.
  const localEntriesCache = useRef<Map<PinnedItemId, readonly LogEntry[]>>(
    new Map(),
  );

  const unpin = useCallback((id: PinnedItemId) => {
    localEntriesCache.current.delete(id);
    setState((prev) => removePin(prev, id));
  }, []);

  const setLocalEntries = useCallback(
    (id: PinnedItemId, entries: readonly LogEntry[]) => {
      localEntriesCache.current.set(id, entries);
    },
    [],
  );

  const getLocalEntries = useCallback(
    (id: PinnedItemId): readonly LogEntry[] | null =>
      localEntriesCache.current.get(id) ?? null,
    [],
  );

  const setActive = useCallback((id: PinnedItemId | null) => {
    setState((prev) => {
      if (prev.activeId === id) return prev;
      // Don't allow pointing activeId at a missing id -- but do allow null.
      if (id !== null && !prev.items.some((it) => it.id === id)) {
        return prev;
      }
      return { ...prev, activeId: id };
    });
  }, []);

  const findExisting = useCallback(
    (item: PinnedItemInput): PinnedItemId | null => matchExisting(state.items, item),
    [state.items],
  );

  const clear = useCallback(() => {
    localEntriesCache.current.clear();
    setState((prev) => (prev.items.length === 0 ? prev : { ...prev, items: [] }));
  }, []);

  const value = useMemo<WorkspaceContextValue>(
    () => ({
      items: state.items,
      activeId: state.activeId,
      pin,
      unpin,
      setActive,
      findExisting,
      clear,
      setLocalEntries,
      getLocalEntries,
    }),
    [
      state.items,
      state.activeId,
      pin,
      unpin,
      setActive,
      findExisting,
      clear,
      setLocalEntries,
      getLocalEntries,
    ],
  );

  return (
    <WorkspaceContext.Provider value={value}>{children}</WorkspaceContext.Provider>
  );
}

/** Access the workspace context. Throws when used outside the provider. */
export function useWorkspace(): WorkspaceContextValue {
  const ctx = useContext(WorkspaceContext);
  if (!ctx) {
    throw new Error("useWorkspace must be used inside <WorkspaceProvider>");
  }
  return ctx;
}

/** Selector hook -- the current active pinned item, or null if there is
 *  no active id or it no longer resolves to an item. */
export function useActivePinnedItem(): PinnedItem | null {
  const { items, activeId } = useWorkspace();
  return useMemo(() => {
    if (activeId === null) return null;
    return items.find((it) => it.id === activeId) ?? null;
  }, [items, activeId]);
}
