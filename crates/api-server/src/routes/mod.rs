pub mod devices;
pub mod health;
pub mod ingest;
pub mod sessions;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;

use crate::error::AppError;

/// Encode a UTF-8 cursor payload as a url-safe, unpadded base64 string.
pub(crate) fn encode_cursor(payload: &str) -> String {
    URL_SAFE_NO_PAD.encode(payload.as_bytes())
}

/// Decode a base64 cursor back into UTF-8. Returns a 400 on garbage input so
/// clients don't have to distinguish "no page" from "corrupt cursor".
pub(crate) fn decode_cursor(cursor: &str) -> Result<String, AppError> {
    let bytes = URL_SAFE_NO_PAD
        .decode(cursor.as_bytes())
        .map_err(|_| AppError::BadRequest("invalid cursor".into()))?;
    String::from_utf8(bytes).map_err(|_| AppError::BadRequest("invalid cursor".into()))
}

/// Clamp a caller-provided `limit` to `[1, max]`, defaulting to `default`.
pub(crate) fn clamp_limit(limit: Option<u32>, default: u32, max: u32) -> u32 {
    limit.unwrap_or(default).clamp(1, max)
}
