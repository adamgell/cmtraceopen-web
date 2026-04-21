//! Bundle-ingest routes. Three endpoints implement a resumable,
//! chunked-upload protocol over plain HTTP:
//!
//!   POST /v1/ingest/bundles                              - init
//!   PUT  /v1/ingest/bundles/{upload_id}/chunks?offset=N  - append chunk
//!   POST /v1/ingest/bundles/{upload_id}/finalize         - verify + commit
//!
//! Device identity is carried in the `X-Device-Id` header for MVP. All three
//! routes require it. TODO(M2): switch to cert-identity middleware.

use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{post, put};
use axum::{Json, Router};
use bytes::Bytes;
use chrono::Utc;
use serde::Deserialize;
use tracing::{info, instrument, warn};
use uuid::Uuid;

use common_wire::ingest::{
    content_kind, BundleFinalizeRequest, BundleFinalizeResponse, BundleInitRequest,
    BundleInitResponse,
};

use crate::error::AppError;
use crate::extract::DeviceId;
use crate::state::{AppState, DEFAULT_CHUNK_SIZE, MAX_CHUNK_SIZE};
use crate::storage::{NewUpload, SessionRow, StorageError};

pub fn router(state: Arc<AppState>) -> Router {
    // Axum's default request-body limit is 2 MiB, which would silently 413
    // every chunk above that ceiling before our MAX_CHUNK_SIZE check had a
    // chance to run. Lift the limit on the ingest sub-router to the same
    // constant the handler enforces, so the two values can never drift.
    // Other routers (devices/sessions/health) keep the tight default.
    let body_limit = MAX_CHUNK_SIZE as usize;
    Router::new()
        .route("/v1/ingest/bundles", post(init))
        .route("/v1/ingest/bundles/{upload_id}/chunks", put(put_chunk))
        .route("/v1/ingest/bundles/{upload_id}/finalize", post(finalize))
        .layer(DefaultBodyLimit::max(body_limit))
        .with_state(state)
}

fn validate_sha256_hex(s: &str) -> Result<(), AppError> {
    if s.len() != 64 || !s.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(AppError::BadRequest(
            "sha256 must be 64 lowercase hex chars".into(),
        ));
    }
    Ok(())
}

fn validate_content_kind(k: &str) -> Result<(), AppError> {
    match k {
        content_kind::EVIDENCE_ZIP | content_kind::NDJSON_ENTRIES | content_kind::RAW_FILE => {
            Ok(())
        }
        _ => Err(AppError::BadRequest(format!(
            "unknown contentKind: {k:?} (expected one of: {}, {}, {})",
            content_kind::EVIDENCE_ZIP,
            content_kind::NDJSON_ENTRIES,
            content_kind::RAW_FILE,
        ))),
    }
}

#[instrument(
    skip_all,
    fields(
        device_id = %device_id,
        bundle_id = %req.bundle_id,
        size_bytes = req.size_bytes,
        content_kind = %req.content_kind,
    ),
)]
async fn init(
    State(state): State<Arc<AppState>>,
    DeviceId(device_id): DeviceId,
    Json(req): Json<BundleInitRequest>,
) -> Result<(StatusCode, Json<BundleInitResponse>), AppError> {
    validate_sha256_hex(&req.sha256)?;
    validate_content_kind(&req.content_kind)?;
    if req.size_bytes == 0 {
        return Err(AppError::BadRequest("sizeBytes must be > 0".into()));
    }

    let now = Utc::now();

    // Pre-register the device so the subsequent session insert's FK is
    // satisfied.
    state
        .meta
        .upsert_device(&device_id, req.device_hint.as_deref(), now)
        .await?;

    // If this (device, bundle) already finalized, short-circuit with
    // resume_offset = size_bytes so the agent knows it's done. Keeps retries
    // idempotent.
    if let Some(existing) = state
        .meta
        .find_session_by_bundle(&device_id, req.bundle_id)
        .await?
    {
        info!(
            session_id = %existing.session_id,
            "init short-circuited: bundle already finalized",
        );
        return Ok((
            StatusCode::OK,
            Json(BundleInitResponse {
                // We don't have a meaningful upload_id to hand back; reuse
                // the session_id as a stable echo so retried inits are
                // consistent.
                upload_id: existing.session_id,
                chunk_size: DEFAULT_CHUNK_SIZE,
                resume_offset: existing.size_bytes,
            }),
        ));
    }

    // If a prior upload was interrupted for this (device, bundle), resume it.
    //
    // But only if the init invariants (sha256, size_bytes, content_kind) match
    // what's on disk. A client re-initing the same bundle_id with a *different*
    // sha / size / kind is almost certainly a bundle-id collision or a client
    // bug — resuming would silently mix bytes from two different bundles, so
    // reject it as 409 instead.
    if let Some(prior) = state
        .meta
        .find_resumable_upload(&device_id, req.bundle_id)
        .await?
    {
        let new_sha = req.sha256.to_lowercase();
        if prior.expected_sha256 != new_sha {
            return Err(AppError::Conflict(format!(
                "bundle {} already initialized with a different sha256 \
                 (stored={}, requested={}); refusing to resume",
                req.bundle_id, prior.expected_sha256, new_sha
            )));
        }
        if prior.size_bytes != req.size_bytes {
            return Err(AppError::Conflict(format!(
                "bundle {} already initialized with a different sizeBytes \
                 (stored={}, requested={}); refusing to resume",
                req.bundle_id, prior.size_bytes, req.size_bytes
            )));
        }
        if prior.content_kind != req.content_kind {
            return Err(AppError::Conflict(format!(
                "bundle {} already initialized with a different contentKind \
                 (stored={}, requested={}); refusing to resume",
                req.bundle_id, prior.content_kind, req.content_kind
            )));
        }
        info!(
            upload_id = %prior.upload_id,
            resume_offset = prior.offset_bytes,
            "resuming interrupted upload",
        );
        return Ok((
            StatusCode::OK,
            Json(BundleInitResponse {
                upload_id: prior.upload_id,
                chunk_size: DEFAULT_CHUNK_SIZE,
                resume_offset: prior.offset_bytes,
            }),
        ));
    }

    // Fresh upload.
    let upload_id = Uuid::now_v7();
    state.blobs.create_staging(upload_id).await?;
    let staged_path = state
        .blobs
        .staging_path(upload_id)
        .to_string_lossy()
        .to_string();

    state
        .meta
        .insert_upload(
            NewUpload {
                upload_id,
                bundle_id: req.bundle_id,
                device_id: device_id.clone(),
                size_bytes: req.size_bytes,
                expected_sha256: req.sha256.to_lowercase(),
                content_kind: req.content_kind.clone(),
                staged_path,
            },
            now,
        )
        .await?;

    info!(
        %upload_id,
        %device_id,
        bundle_id = %req.bundle_id,
        size_bytes = req.size_bytes,
        "bundle upload initialized"
    );

    Ok((
        StatusCode::CREATED,
        Json(BundleInitResponse {
            upload_id,
            chunk_size: DEFAULT_CHUNK_SIZE,
            resume_offset: 0,
        }),
    ))
}

#[derive(Debug, Deserialize)]
struct ChunkQuery {
    offset: u64,
}

#[derive(Debug, serde::Serialize)]
struct ChunkResponse {
    #[serde(rename = "nextOffset")]
    next_offset: u64,
}

#[instrument(
    skip_all,
    fields(
        device_id = %device_id,
        upload_id = %upload_id,
        offset = q.offset,
        chunk_len = body.len(),
    ),
)]
async fn put_chunk(
    State(state): State<Arc<AppState>>,
    DeviceId(device_id): DeviceId,
    Path(upload_id): Path<Uuid>,
    Query(q): Query<ChunkQuery>,
    body: Bytes,
) -> Result<Json<ChunkResponse>, AppError> {
    let upload = state.meta.get_upload(upload_id).await?;

    // Device binding: the upload belongs to the device that created it. A
    // different device presenting the same upload_id is rejected as 404 to
    // avoid leaking that the upload exists.
    if upload.device_id != device_id {
        return Err(AppError::NotFound(format!(
            "upload {upload_id} not found"
        )));
    }

    if upload.finalized {
        return Err(AppError::from(StorageError::AlreadyFinalized(upload_id)));
    }

    let body_len = body.len() as u64;
    if body_len == 0 {
        return Err(AppError::BadRequest("empty chunk".into()));
    }
    if body_len > MAX_CHUNK_SIZE {
        return Err(AppError::BadRequest(format!(
            "chunk too large: {body_len} > {MAX_CHUNK_SIZE}"
        )));
    }

    // size_bytes is immutable for a given upload_id so this pre-flight
    // overflow check against the stale snapshot is still correct.
    let new_offset = q.offset.saturating_add(body_len);
    if new_offset > upload.size_bytes {
        return Err(AppError::from(StorageError::SizeOverflow {
            declared: upload.size_bytes,
            attempted: new_offset,
        }));
    }

    // Atomic reservation of the offset slot. The previous read-then-write
    // sequence let two concurrent PUTs at the same offset both pass the
    // check; a single conditional UPDATE closes that race at the DB level.
    let reserved = state
        .meta
        .compare_and_set_upload_offset(upload_id, q.offset, new_offset)
        .await?;
    if !reserved {
        // Someone else advanced the cursor. Re-fetch for an accurate error
        // body so the client can retry at the real cursor.
        let current = state
            .meta
            .get_upload(upload_id)
            .await
            .map(|u| u.offset_bytes)
            // If it disappeared, fall back to the stale snapshot value.
            .unwrap_or(upload.offset_bytes);
        return Err(AppError::from(StorageError::OffsetMismatch {
            expected: current,
            actual: q.offset,
        }));
    }

    // DB slot reserved; append bytes. If the blob write fails after we've
    // already advanced the cursor, the upload can't finalize (sha will
    // mismatch) — the client will re-init. Inverting the order (blob first,
    // then DB) would leave two concurrent PUTs appending at the same byte
    // range, which is worse.
    if let Err(e) = state.blobs.put_chunk(upload_id, q.offset, &body).await {
        warn!(
            %upload_id,
            offset = q.offset,
            error = %e,
            "blob append failed after offset reservation; upload left in broken state"
        );
        return Err(AppError::from(e));
    }

    info!(next_offset = new_offset, "chunk accepted");
    Ok(Json(ChunkResponse { next_offset: new_offset }))
}

#[instrument(
    skip_all,
    fields(
        device_id = %device_id,
        upload_id = %upload_id,
    ),
)]
async fn finalize(
    State(state): State<Arc<AppState>>,
    DeviceId(device_id): DeviceId,
    Path(upload_id): Path<Uuid>,
    Json(req): Json<BundleFinalizeRequest>,
) -> Result<(StatusCode, Json<BundleFinalizeResponse>), AppError> {
    validate_sha256_hex(&req.final_sha256)?;

    let upload = state.meta.get_upload(upload_id).await?;
    if upload.device_id != device_id {
        return Err(AppError::NotFound(format!(
            "upload {upload_id} not found"
        )));
    }

    // Idempotent finalize: if a prior call already committed the session,
    // return it.
    if upload.finalized {
        if let Some(existing) = state
            .meta
            .find_session_by_bundle(&device_id, upload.bundle_id)
            .await?
        {
            return Ok((
                StatusCode::OK,
                Json(BundleFinalizeResponse {
                    session_id: existing.session_id,
                    parse_state: existing.parse_state,
                }),
            ));
        }
        return Err(AppError::from(StorageError::AlreadyFinalized(upload_id)));
    }

    if upload.offset_bytes != upload.size_bytes {
        return Err(AppError::BadRequest(format!(
            "incomplete upload: {} of {} bytes received",
            upload.offset_bytes, upload.size_bytes
        )));
    }

    // Verify client-claimed sha256 matches staged bytes, and that both match
    // what we were told at init. Case-insensitive compare to tolerate mixed
    // casing on the client side.
    let actual = state.blobs.hash(upload_id).await?;
    let expected_init = upload.expected_sha256.to_lowercase();
    let claimed_final = req.final_sha256.to_lowercase();
    if actual != expected_init || actual != claimed_final {
        warn!(
            %upload_id,
            %device_id,
            %actual,
            %expected_init,
            %claimed_final,
            "sha256 mismatch; discarding staging"
        );
        // Fire-and-forget discard; log the error but still return the primary
        // one.
        if let Err(e) = state.blobs.discard_staging(upload_id).await {
            warn!(%upload_id, error = %e, "failed to discard staging after mismatch");
        }
        return Err(AppError::from(StorageError::Sha256Mismatch {
            expected: expected_init,
            actual,
        }));
    }

    let session_id = Uuid::now_v7();
    let handle = state.blobs.finalize(upload_id, session_id).await?;
    state.meta.mark_upload_finalized(upload_id).await?;

    let now = Utc::now();
    let row = SessionRow {
        session_id,
        device_id: device_id.clone(),
        bundle_id: upload.bundle_id,
        blob_uri: handle.uri,
        content_kind: upload.content_kind.clone(),
        size_bytes: handle.size_bytes,
        sha256: handle.sha256,
        collected_utc: None, // TODO(M2): extract from bundle manifest when parsing.
        ingested_utc: now,
        parse_state: "pending".to_string(),
    };

    match state.meta.insert_session(row.clone()).await {
        Ok(()) => {}
        Err(StorageError::SessionConflict { .. }) => {
            // Another concurrent finalize won. Return the winning session
            // instead of erroring.
            if let Some(existing) = state
                .meta
                .find_session_by_bundle(&device_id, upload.bundle_id)
                .await?
            {
                return Ok((
                    StatusCode::OK,
                    Json(BundleFinalizeResponse {
                        session_id: existing.session_id,
                        parse_state: existing.parse_state,
                    }),
                ));
            }
            return Err(AppError::Conflict(
                "session conflict but no existing session found".into(),
            ));
        }
        Err(e) => return Err(AppError::from(e)),
    }

    // TODO(M2): enqueue a background parse job using cmtraceopen-parser from
    // the sibling submodule. For MVP parse_state stays "pending".

    info!(
        %session_id,
        %upload_id,
        %device_id,
        bundle_id = %upload.bundle_id,
        size_bytes = row.size_bytes,
        "bundle finalized"
    );

    Ok((
        StatusCode::CREATED,
        Json(BundleFinalizeResponse {
            session_id: row.session_id,
            parse_state: row.parse_state,
        }),
    ))
}
