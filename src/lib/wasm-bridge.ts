// Thin typed wrapper over the wasm-pack generated bindings. Lazily initializes
// the WASM module on first use so callers can `await` a stable Promise without
// worrying about double-init races.
//
// The generated bindings live in ../../pkg/ after `pnpm wasm:build`. They are
// gitignored; a dev sets them up with `pnpm install && pnpm wasm:build`.

import init, { ping, parseContent } from "../../pkg/cmtrace_wasm";

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
 * Typed as `unknown` until we introduce a shared TS types package mirroring
 * the parser crate's serde shapes — callers should narrow at the use site.
 */
export async function parse(content: string, filePath: string): Promise<unknown> {
  await initWasm();
  return parseContent(content, filePath, content.length);
}
