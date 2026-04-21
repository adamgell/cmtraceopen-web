// cmtrace-wasm
//
// Thin wasm-bindgen wrapper around the pure-Rust cmtraceopen-parser crate.
// Exposed JS surface (after wasm-pack):
//
//   init(): Promise<void>                  // call once on load (handled by wasm-bridge.ts)
//   ping(): string                          // version sanity check
//   parseContent(content, filePath, size): ParseResult
//
// The crate itself has no filesystem, Tauri, or native dependencies — all parsing
// is pure functional transformation from String → LogEntry[].

use wasm_bindgen::prelude::*;

#[wasm_bindgen(start)]
pub fn init() {
    #[cfg(feature = "console_error_panic_hook")]
    console_error_panic_hook::set_once();
}

/// Sanity ping — returns the compiled crate version. Useful for confirming the
/// WASM module has loaded before doing any real parsing work.
#[wasm_bindgen]
pub fn ping() -> String {
    format!("cmtrace-wasm v{}", env!("CARGO_PKG_VERSION"))
}

/// Parse log content, auto-detecting the log format. Returns a `ParseResult`-shaped
/// JS object with `entries`, `formatDetected`, `parserSelection`, `totalLines`,
/// `parseErrors`, `filePath`, `fileSize`, `byteOffset`.
///
/// `fileSize` arrives as a JS `number` (f64) for ergonomic interop; values up to
/// 2^53 bytes (~9 PB) round-trip exactly, which is fine for any realistic log.
#[wasm_bindgen(js_name = parseContent)]
pub fn parse_content(
    content: &str,
    file_path: &str,
    file_size: f64,
) -> Result<JsValue, JsValue> {
    let size = if file_size.is_finite() && file_size >= 0.0 {
        file_size as u64
    } else {
        content.len() as u64
    };

    let (result, _selection) = cmtraceopen_parser::parser::parse_content(content, file_path, size);

    serde_wasm_bindgen::to_value(&result).map_err(|e| JsValue::from_str(&e.to_string()))
}
