//! Generic [`BlobStore`] adapter on top of any [`object_store::ObjectStore`].
//!
//! This is the production seam. Pick a backend (LocalFileSystem,
//! MicrosoftAzure, AmazonS3, GoogleCloudStorage) once at startup, hand the
//! resulting `Arc<dyn ObjectStore>` to [`ObjectStoreBlobStore::new`], and
//! every ingest / parse code path threads through the same trait without
//! caring where the bytes physically live.
//!
//! ## Why staging is still local-disk
//!
//! `ObjectStore::put_multipart` only supports *append-ordered* parts —
//! there is no random-offset write API. The ingest wire protocol
//! (`PUT /v1/ingest/uploads/<id>?offset=N`) is sequential by contract, but
//! the local seek-and-write model is what the resume / retry semantics on
//! the agent side were designed against, and switching the staging path to
//! per-chunk multipart-parts would change failure modes (a missed chunk in
//! the middle becomes unrecoverable instead of "PUT it again at the same
//! offset").
//!
//! So: staging files live on local disk under `<staging_root>/<upload_id>`
//! regardless of backend, and only the *finalized* blob is shipped to the
//! configured object_store. For local-FS the upload step is just a rename
//! (object_store::local copies + deletes under the hood); for Azure it's a
//! single multipart upload of the assembled bundle. Bundle size is bounded
//! per-host by available disk during the ingest window — finalized blobs
//! are what need to scale beyond a single host's disk, and those land in
//! the object store.
//!
//! ## URI scheme contract
//!
//! `BlobHandle::uri` is opaque to clients but the parse worker round-trips
//! it back through [`BlobStore::read_blob`]. To keep callers from having to
//! parse the URI, the implementation just stores the `object_store::path::Path`
//! verbatim (prefixed with the configured scheme + host for human readability
//! on the status page and in logs). Anything written by `finalize` is
//! readable by `read_blob` — that's the only invariant route handlers care
//! about.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use bytes::Bytes;
use futures::StreamExt;
use object_store::path::Path as ObjPath;
use object_store::{ObjectStore, PutPayload};
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use uuid::Uuid;

use super::{BlobHandle, BlobStore, StorageError};

/// Buffer size for streamed sha256 hashing over the staged file.
const HASH_READ_BUF: usize = 1024 * 1024; // 1 MiB

/// Path prefix inside the object store where finalized blobs live, keyed by
/// session_id. A single literal so URI generation, lookup, and any future
/// admin tooling stay in agreement.
const BLOBS_PREFIX: &str = "blobs";

/// Generic [`BlobStore`] backed by an [`object_store::ObjectStore`].
///
/// `staging_root` is a local directory used to assemble in-progress uploads
/// before they're shipped to the object store at finalize. See the module
/// docs for why staging is local-only.
///
/// `uri_scheme` + `uri_host` build the human-readable `BlobHandle::uri` we
/// hand to the metadata store: `<scheme>://<host>/<obj_path>`. For local-FS
/// the constructor builds these to match the legacy `file://<root>/blobs/...`
/// form so on-disk layout + URIs are byte-for-byte compatible with the
/// pre-refactor `LocalFsBlobStore`. For Azure they're `azure://<container>`.
pub struct ObjectStoreBlobStore {
    inner: Arc<dyn ObjectStore>,
    staging_root: PathBuf,
    uri_scheme: String,
    uri_host: String,
}

impl ObjectStoreBlobStore {
    /// Build an adapter. Creates `<staging_root>` eagerly so the first
    /// `create_staging` call doesn't pay for the directory walk.
    ///
    /// `uri_scheme` and `uri_host` are baked into the [`BlobHandle::uri`]
    /// returned by [`Self::finalize`]. The host part is purely cosmetic for
    /// non-local backends — the lookup path inside `read_blob` parses the
    /// trailing `obj_path` and ignores the host.
    pub async fn new(
        inner: Arc<dyn ObjectStore>,
        staging_root: PathBuf,
        uri_scheme: impl Into<String>,
        uri_host: impl Into<String>,
    ) -> Result<Self, StorageError> {
        fs::create_dir_all(&staging_root).await?;
        Ok(Self {
            inner,
            staging_root,
            uri_scheme: uri_scheme.into(),
            uri_host: uri_host.into(),
        })
    }

    /// Object-store key for a finalized session blob.
    fn blob_obj_path(session_id: Uuid) -> ObjPath {
        ObjPath::from(format!("{BLOBS_PREFIX}/{session_id}"))
    }

    /// Build the public URI we surface to callers.
    fn build_uri(&self, obj_path: &ObjPath) -> String {
        // For local-FS we want exactly `file://<root>/blobs/<id>` (matches
        // the legacy hand-rolled string so existing rows in `sessions.blob_uri`
        // and any operator tooling don't see a churn). For non-local schemes
        // the scheme://host/<key> shape is what S3 / Azure tooling already
        // produces.
        format!("{}://{}/{}", self.uri_scheme, self.uri_host, obj_path.as_ref())
    }

    /// Inverse of [`Self::build_uri`]: pull out the object-store key from a
    /// URI we previously emitted. Returns `None` if the URI doesn't carry our
    /// scheme — used by `read_blob` / `head_blob` to short-circuit unknown
    /// URIs into a clean error.
    fn extract_obj_path(&self, uri: &str) -> Option<ObjPath> {
        let prefix = format!("{}://{}/", self.uri_scheme, self.uri_host);
        let rest = uri.strip_prefix(&prefix)?;
        Some(ObjPath::from(rest))
    }
}

#[async_trait]
impl BlobStore for ObjectStoreBlobStore {
    fn staging_path(&self, upload_id: Uuid) -> PathBuf {
        self.staging_root.join(upload_id.to_string())
    }

    async fn create_staging(&self, upload_id: Uuid) -> Result<(), StorageError> {
        let path = self.staging_path(upload_id);
        // Same overwrite-on-collide semantics as the legacy impl: the
        // metadata store is the source of truth for what's in flight, so a
        // stale file from a crashed prior attempt should be wiped.
        let mut f = fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&path)
            .await?;
        f.flush().await?;
        Ok(())
    }

    async fn put_chunk(
        &self,
        upload_id: Uuid,
        offset: u64,
        bytes: &[u8],
    ) -> Result<(), StorageError> {
        let path = self.staging_path(upload_id);
        let mut f = fs::OpenOptions::new().write(true).open(&path).await?;
        f.seek(std::io::SeekFrom::Start(offset)).await?;
        f.write_all(bytes).await?;
        f.flush().await?;
        Ok(())
    }

    async fn hash(&self, upload_id: Uuid) -> Result<String, StorageError> {
        let path = self.staging_path(upload_id);
        stream_sha256(&path).await
    }

    async fn finalize(
        &self,
        upload_id: Uuid,
        session_id: Uuid,
    ) -> Result<BlobHandle, StorageError> {
        let src = self.staging_path(upload_id);

        // Re-hash + size under the same FD so the reported values reflect
        // the bytes we're about to ship, not a racing writer.
        let metadata = fs::metadata(&src).await?;
        let size_bytes = metadata.len();
        let sha256 = stream_sha256(&src).await?;

        let dst = Self::blob_obj_path(session_id);

        // Stream the staged file to the object store. For local-FS this is
        // a copy + delete; for Azure it's a single block-blob upload (or
        // multipart for large bundles — object_store picks the right path).
        // We use put_multipart so payloads larger than one Azure block-blob
        // commit (~256 MiB) work without us having to size-branch.
        upload_file_to_store(self.inner.as_ref(), &src, &dst).await?;

        // Staged copy is no longer needed; best-effort cleanup. A failure
        // here doesn't invalidate the finalized blob, so we don't bubble it.
        if let Err(err) = fs::remove_file(&src).await {
            tracing::warn!(
                upload_id = %upload_id,
                staging = %src.display(),
                error = %err,
                "failed to remove staging file after finalize; will linger until process restart"
            );
        }

        let uri = self.build_uri(&dst);
        Ok(BlobHandle {
            uri,
            size_bytes,
            sha256,
        })
    }

    async fn discard_staging(&self, upload_id: Uuid) -> Result<(), StorageError> {
        let path = self.staging_path(upload_id);
        match fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(err.into()),
        }
    }

    async fn head_blob(&self, uri: &str) -> Result<u64, StorageError> {
        let obj_path = self
            .extract_obj_path(uri)
            .ok_or_else(|| StorageError::BadBlobUri(uri.to_string()))?;
        let meta = self
            .inner
            .head(&obj_path)
            .await
            .map_err(|e| StorageError::ObjectStore(e.to_string()))?;
        Ok(meta.size as u64)
    }

    async fn read_blob(&self, uri: &str) -> Result<Vec<u8>, StorageError> {
        let obj_path = self
            .extract_obj_path(uri)
            .ok_or_else(|| StorageError::BadBlobUri(uri.to_string()))?;
        let get_result = self
            .inner
            .get(&obj_path)
            .await
            .map_err(|e| StorageError::ObjectStore(e.to_string()))?;

        // Stream the body into a single Vec<u8>. Worth noting: the parse
        // worker already enforces a size cap (`MAX_EVIDENCE_ZIP_BYTES`) via
        // `head_blob` before this is called, so we know the buffer can't
        // run away — but we still iterate the stream rather than calling
        // `bytes()` in one shot so memory pressure shows up as backpressure
        // rather than a single allocation spike on a large bundle.
        let mut stream = get_result.into_stream();
        let mut out = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk: Bytes = chunk.map_err(|e| StorageError::ObjectStore(e.to_string()))?;
            out.extend_from_slice(&chunk);
        }
        Ok(out)
    }
}

async fn stream_sha256(path: &Path) -> Result<String, StorageError> {
    let mut f = fs::File::open(path).await?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; HASH_READ_BUF];
    loop {
        let n = f.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Stream a local file up to the configured object store, using multipart
/// so very large bundles don't require buffering the whole thing in memory.
async fn upload_file_to_store(
    store: &dyn ObjectStore,
    src: &Path,
    dst: &ObjPath,
) -> Result<(), StorageError> {
    // Read the staged file in chunks and feed them into a multipart upload.
    // For LocalFileSystem this is internally just rename/copy; for Azure
    // each chunk becomes a staged block which is committed by `complete()`.
    let mut upload = store
        .put_multipart(dst)
        .await
        .map_err(|e| StorageError::ObjectStore(e.to_string()))?;

    let mut f = fs::File::open(src).await?;
    // 8 MiB matches `state::DEFAULT_CHUNK_SIZE` so behavior is uniform with
    // the wire-protocol chunk size — operators reasoning about throughput
    // see the same number on both sides of finalize.
    let mut buf = vec![0u8; 8 * 1024 * 1024];
    loop {
        let n = f.read(&mut buf).await?;
        if n == 0 {
            break;
        }
        let part = PutPayload::from(Bytes::copy_from_slice(&buf[..n]));
        if let Err(e) = upload.put_part(part).await {
            // Best-effort abort so we don't leak orphan parts on the
            // backend. The finalize call already failed; we surface the
            // original put_part error, not the abort error.
            let _ = upload.abort().await;
            return Err(StorageError::ObjectStore(e.to_string()));
        }
    }

    upload
        .complete()
        .await
        .map_err(|e| StorageError::ObjectStore(e.to_string()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use object_store::local::LocalFileSystem;
    use tempfile::TempDir;

    /// Build a LocalFileSystem-backed ObjectStoreBlobStore for tests.
    /// Uses `<tmp>/staging` for in-progress uploads and `<tmp>/blobs/...`
    /// for finalized objects (LocalFileSystem rooted at `<tmp>` puts every
    /// `Path` under that prefix, so `blobs/<id>` lands at the right place).
    async fn build_store(tmp: &TempDir) -> ObjectStoreBlobStore {
        // Create the `blobs` dir up front so LocalFileSystem doesn't try to
        // walk a nonexistent prefix on first put. (object_store::local is
        // fine creating files but expects the parent dir to exist for some
        // operations.)
        fs::create_dir_all(tmp.path().join(BLOBS_PREFIX)).await.unwrap();
        let inner: Arc<dyn ObjectStore> =
            Arc::new(LocalFileSystem::new_with_prefix(tmp.path()).unwrap());
        let staging_root = tmp.path().join("staging");
        let uri_host = tmp.path().to_string_lossy().replace('\\', "/");
        // Strip leading slash on unix so the local-fs URI form ends up as
        // `file://<host>/blobs/...` — matches the legacy hand-rolled writer.
        let uri_host = uri_host.trim_start_matches('/').to_string();
        ObjectStoreBlobStore::new(inner, staging_root, "file", uri_host)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn put_hash_finalize_round_trip() {
        let tmp = TempDir::new().unwrap();
        let store = build_store(&tmp).await;

        let upload_id = Uuid::now_v7();
        let session_id = Uuid::now_v7();
        store.create_staging(upload_id).await.unwrap();
        store.put_chunk(upload_id, 0, b"hello ").await.unwrap();
        store.put_chunk(upload_id, 6, b"world").await.unwrap();

        let h = store.hash(upload_id).await.unwrap();
        assert_eq!(
            h,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );

        let handle = store.finalize(upload_id, session_id).await.unwrap();
        assert_eq!(handle.size_bytes, 11);
        assert!(handle.uri.starts_with("file://"));

        // Round-trip: read_blob must return the same bytes finalize wrote.
        let got = store.read_blob(&handle.uri).await.unwrap();
        assert_eq!(got, b"hello world");

        // head_blob agrees with finalize's reported size.
        let size = store.head_blob(&handle.uri).await.unwrap();
        assert_eq!(size, 11);
    }

    #[tokio::test]
    async fn discard_missing_is_ok() {
        let tmp = TempDir::new().unwrap();
        let store = build_store(&tmp).await;
        store.discard_staging(Uuid::now_v7()).await.unwrap();
    }

    #[tokio::test]
    async fn read_blob_unknown_uri_errors_cleanly() {
        let tmp = TempDir::new().unwrap();
        let store = build_store(&tmp).await;
        let err = store
            .read_blob("s3://some-bucket/blobs/nope")
            .await
            .unwrap_err();
        assert!(matches!(err, StorageError::BadBlobUri(_)));
    }
}
