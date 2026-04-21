//! Axum extractors.
//!
//! # DeviceId (MVP)
//! Until mTLS termination lands in M2, the agent identifies itself via the
//! `X-Device-Id` request header. This is explicitly a placeholder — in
//! production the device id will be derived from the client certificate
//! fingerprint by a middleware layer, and the header will be ignored.
//!
//! TODO(M2): replace with cert-identity middleware.

use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use common_wire::ErrorBody;

pub const DEVICE_ID_HEADER: &str = "x-device-id";

/// Extracted device identity. Just a newtype over the header value for now.
#[derive(Debug, Clone)]
pub struct DeviceId(pub String);

impl<S> FromRequestParts<S> for DeviceId
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let hv = parts
            .headers
            .get(DEVICE_ID_HEADER)
            .ok_or_else(missing_header_response)?;

        let s = hv.to_str().map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorBody {
                    error: "bad_request".into(),
                    message: "X-Device-Id must be ASCII".into(),
                }),
            )
                .into_response()
        })?;

        let trimmed = s.trim();
        if trimmed.is_empty() || trimmed.len() > 256 {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorBody {
                    error: "bad_request".into(),
                    message: "X-Device-Id must be 1..=256 chars".into(),
                }),
            )
                .into_response());
        }

        Ok(DeviceId(trimmed.to_string()))
    }
}

fn missing_header_response() -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(ErrorBody {
            error: "bad_request".into(),
            message: "missing X-Device-Id header (MVP device identity until mTLS lands)".into(),
        }),
    )
        .into_response()
}
