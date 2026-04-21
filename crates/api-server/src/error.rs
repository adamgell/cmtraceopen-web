//! HTTP-facing error type. All handler results funnel through `AppError` so
//! we get consistent JSON bodies + status codes.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use common_wire::ErrorBody;
use tracing::error;

use crate::storage::StorageError;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("{0}")]
    BadRequest(String),

    #[error("{0}")]
    NotFound(String),

    #[error("{0}")]
    Conflict(String),

    #[error("storage error: {0}")]
    Storage(#[from] StorageError),

    #[error("internal error: {0}")]
    Internal(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code) = match &self {
            AppError::BadRequest(_) => (StatusCode::BAD_REQUEST, "bad_request"),
            AppError::NotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
            AppError::Conflict(_) => (StatusCode::CONFLICT, "conflict"),
            AppError::Storage(err) => match err {
                StorageError::UploadNotFound(_) => (StatusCode::NOT_FOUND, "not_found"),
                StorageError::OffsetMismatch { .. } => {
                    (StatusCode::CONFLICT, "offset_mismatch")
                }
                StorageError::SizeOverflow { .. } => {
                    (StatusCode::BAD_REQUEST, "size_overflow")
                }
                StorageError::Sha256Mismatch { .. } => {
                    (StatusCode::BAD_REQUEST, "sha256_mismatch")
                }
                StorageError::AlreadyFinalized(_) => {
                    (StatusCode::CONFLICT, "already_finalized")
                }
                StorageError::SessionConflict { .. } => (StatusCode::CONFLICT, "conflict"),
                _ => {
                    error!(error = %err, "storage error");
                    (StatusCode::INTERNAL_SERVER_ERROR, "internal")
                }
            },
            AppError::Internal(msg) => {
                error!(%msg, "internal error");
                (StatusCode::INTERNAL_SERVER_ERROR, "internal")
            }
        };

        let body = ErrorBody {
            error: code.to_string(),
            message: self.to_string(),
        };
        (status, Json(body)).into_response()
    }
}
