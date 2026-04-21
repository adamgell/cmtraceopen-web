// Thin typed wrapper over the wasm-pack generated bindings. Lazily initializes
// the WASM module on first use so callers can `await` a stable Promise without
// worrying about double-init races.
//
// The generated bindings live in ../../pkg/ after `pnpm wasm:build`. They are
// gitignored; a dev sets them up with `pnpm install && pnpm wasm:build`.

import init, { ping, parseContent } from "../../pkg/cmtrace_wasm";
import type { ParseResult } from "./log-types";

let readyPromise: Promise<void> | null = null;

export function initWasm(): Promise<void> {
  if (!readyPromise) {
    readyPromise = init().then(() => undefined);
  }
  return readyPromise;
}

/** Sanity check — returns the compiled cmtrace-wasm crate version. */
export async function wasmPing(): Promise<string> {
  await initWasm();
  return ping();
}

/**
 * Parse log content, auto-detecting the format. The returned shape mirrors
 * `cmtraceopen_parser::models::log_entry::ParseResult` (camelCased by serde):
 * `entries`, `formatDetected`, `parserSelection`, `totalLines`, `parseErrors`,
 * `filePath`, `fileSize`, `byteOffset`.
 *
 * `fileSize` MUST be the UTF-8 byte size of the file — the Rust side uses it
 * as `byte_offset` for tailing and for parity with native file reads. It is
 * NOT equivalent to `content.length` (which is UTF-16 code units and is
 * wrong for any file containing non-ASCII characters).
 *
 * Callers that have a `File` (drag-drop, file picker) should pass
 * `file.size`. Callers that only have a string (e.g., pasted content) can
 * omit `fileSize` and we'll compute the true UTF-8 byte length via
 * `TextEncoder`.
 *
 * The wasm-pack binding returns `any` — we assert to `ParseResult` here so
 * callers get a typed view without sprinkling casts at use sites. The shape
 * is guaranteed by the Rust `#[serde(rename_all = "camelCase")]` contract
 * and mirrored in `src/lib/log-types.ts`.
 */
export async function parse(
  content: string,
  filePath: string,
  fileSize?: number,
): Promise<ParseResult> {
  await initWasm();
  const size =
    fileSize ?? new TextEncoder().encode(content).byteLength;
  return parseContent(content, filePath, size) as ParseResult;
}
