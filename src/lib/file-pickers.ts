// Browser file-picker helpers.
//
// The File System Access API (`window.showOpenFilePicker`,
// `window.showDirectoryPicker`) gives a real OS-native picker when available
// — currently Chromium-only. Firefox and Safari still need the classic
// `<input type="file">` fallback, which we keep behind the same Promise
// shape so callers don't have to branch. Desktop parity is deliberately
// partial: the Tauri app uses `open()` with path metadata we can't expose
// from a browser, so folder/file handles are flattened to plain `File`
// objects here.
//
// These helpers are UI-free on purpose. Sidebars, toolbars, and empty-state
// cards can call into them without importing any picker-shaped component.

/** Minimal typings for the subset of the File System Access API we use.
 *  Kept local because the standard TS DOM lib still marks these as optional
 *  and we want to avoid a `@types/wicg-file-system-access`-style dep. */
interface FileSystemFileHandleLike {
  getFile(): Promise<File>;
}
interface FileSystemDirectoryHandleLike {
  // Async iterator over [name, handle] entries. Handles nest via `kind`.
  entries(): AsyncIterableIterator<
    [string, FileSystemFileHandleLike | FileSystemDirectoryHandleLike]
  >;
  readonly kind: "directory";
}

interface WindowWithFsAccess extends Window {
  showOpenFilePicker?: (options?: {
    multiple?: boolean;
    excludeAcceptAllOption?: boolean;
    types?: Array<{
      description?: string;
      accept: Record<string, string[]>;
    }>;
  }) => Promise<FileSystemFileHandleLike[]>;
  showDirectoryPicker?: (options?: {
    mode?: "read" | "readwrite";
  }) => Promise<FileSystemDirectoryHandleLike>;
}

/** True when the browser exposes the Chromium-era File System Access API
 *  for single files. Call-sites use this to decide between the native
 *  picker and the `<input type="file">` fallback. */
export function hasNativeFilePicker(): boolean {
  return (
    typeof window !== "undefined" &&
    typeof (window as WindowWithFsAccess).showOpenFilePicker === "function"
  );
}

/** True when the browser exposes `showDirectoryPicker`. Non-Chromium
 *  browsers fall back to `<input webkitdirectory>`, which returns a flat
 *  `FileList` rather than a traversable handle. */
export function hasNativeDirectoryPicker(): boolean {
  return (
    typeof window !== "undefined" &&
    typeof (window as WindowWithFsAccess).showDirectoryPicker === "function"
  );
}

/**
 * Prompt the user for a single log file. Resolves to `null` if the user
 * cancels. Never rejects for user-cancellation — the AbortError from the
 * native picker is swallowed so callers can write a linear flow.
 *
 * Uses `showOpenFilePicker` when available (keeps the last-used directory
 * between calls and gives the user a native title-bar). Falls back to a
 * one-shot `<input type="file">` otherwise.
 */
export async function pickLogFile(): Promise<File | null> {
  if (hasNativeFilePicker()) {
    const w = window as WindowWithFsAccess;
    try {
      const [handle] = await w.showOpenFilePicker!({
        multiple: false,
        // CMTrace logs come in many flavors (.log, .txt, .cmtlog, none at
        // all), so we expose the "all files" option too. Kept consistent
        // with DropZone's "anything CMTrace-shaped" posture.
        excludeAcceptAllOption: false,
        types: [
          {
            description: "CMTrace logs",
            accept: {
              "text/plain": [".log", ".txt", ".cmtlog"],
            },
          },
        ],
      });
      if (!handle) return null;
      return await handle.getFile();
    } catch (err) {
      // User cancelled — every Chromium version raises DOMException
      // AbortError. Treat any other error as cancellation too so the
      // caller can keep its UI stable; the user can retry.
      if (isAbortError(err)) return null;
      return null;
    }
  }
  const picked = await pickViaInput({ directory: false });
  // `directory: false` always resolves to a single File or null;
  // narrow explicitly so the public signature stays `File | null`.
  if (picked == null) return null;
  return Array.isArray(picked) ? (picked[0] ?? null) : picked;
}

/**
 * Prompt the user to choose a folder and return every file inside it
 * (recursively). Resolves to `null` if the user cancels.
 *
 * Web-only caveat vs. the desktop Tauri dialog: paths are not exposed, so
 * callers get `File` objects keyed by `File.name`. The native `File`
 * shape only carries `webkitRelativePath` when we're on the input
 * fallback; for `showDirectoryPicker`, we flatten to names only and the
 * caller is responsible for deduplication.
 *
 * TODO(web-port): downstream routing for a folder drop is not wired yet —
 * this helper exists so a future "open a folder of logs" flow has a
 * single place to plug in.
 */
export async function pickLogFolder(): Promise<File[] | null> {
  if (hasNativeDirectoryPicker()) {
    const w = window as WindowWithFsAccess;
    try {
      const dir = await w.showDirectoryPicker!({ mode: "read" });
      const out: File[] = [];
      await collectFilesFromDirectory(dir, out);
      return out;
    } catch (err) {
      if (isAbortError(err)) return null;
      return null;
    }
  }
  const result = await pickViaInput({ directory: true });
  if (result == null) return null;
  return Array.isArray(result) ? result : [result];
}

// ---------------------------------------------------------------------------
// Internals

async function collectFilesFromDirectory(
  dir: FileSystemDirectoryHandleLike,
  out: File[],
): Promise<void> {
  for await (const [, child] of dir.entries()) {
    if ("getFile" in child) {
      out.push(await child.getFile());
    } else if (child.kind === "directory") {
      await collectFilesFromDirectory(child, out);
    }
  }
}

function isAbortError(err: unknown): boolean {
  return (
    typeof err === "object" &&
    err !== null &&
    "name" in err &&
    (err as { name: unknown }).name === "AbortError"
  );
}

/**
 * One-shot `<input type="file">` wrapper. The input is created off-DOM,
 * clicked immediately, and discarded once it fires `change` or `cancel`.
 * Safari / Firefox follow this path exclusively.
 *
 * When `directory` is true we set the non-standard `webkitdirectory`
 * attribute; browsers that support it return the folder's files, the
 * rest silently fall through to a single-file dialog.
 */
function pickViaInput(opts: {
  directory: boolean;
}): Promise<File | File[] | null> {
  return new Promise((resolve) => {
    if (typeof document === "undefined") {
      resolve(null);
      return;
    }
    const input = document.createElement("input");
    input.type = "file";
    if (opts.directory) {
      // Non-standard but widely supported folder-upload attributes. Using
      // `setAttribute` avoids the TS type-narrowing complaint for
      // `webkitdirectory`, which isn't in lib.dom.d.ts.
      input.setAttribute("webkitdirectory", "");
      input.setAttribute("directory", "");
      input.multiple = true;
    } else {
      input.multiple = false;
    }
    input.style.display = "none";

    let settled = false;
    const done = (value: File | File[] | null) => {
      if (settled) return;
      settled = true;
      // `cancel` sometimes fires after `change` on very fast repeated
      // interactions; the guard above prevents double-resolve.
      input.remove();
      resolve(value);
    };

    input.addEventListener("change", () => {
      const files = input.files;
      if (!files || files.length === 0) {
        done(null);
        return;
      }
      if (opts.directory) {
        done(Array.from(files));
      } else {
        done(files[0] ?? null);
      }
    });
    // `cancel` is Chrome 113+ / Firefox 91+ — older browsers just leave
    // the input orphaned until GC, which is harmless since we `remove()`
    // on settle.
    input.addEventListener("cancel", () => done(null));

    document.body.appendChild(input);
    input.click();
  });
}
