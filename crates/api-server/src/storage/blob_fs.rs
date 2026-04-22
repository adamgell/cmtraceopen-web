//! Local-filesystem [`BlobStore`]. Backed by two directories under `<root>`:
//!   - `<root>/staging/<upload_id>`  - in-progress uploads
//!   - `<root>/blobs/<session_id>`   - finalized content-addressed blobs
//!
//! ## Implementation note
//!
//! Historically this module hand-rolled `tokio::fs` calls. It now delegates
//! to the shared [`ObjectStoreBlobStore`] adapter configured with
//! `object_store::local::LocalFileSystem`, so the local-dev path and the
//! cloud-backed paths share one production-tested code path. The
//! hand-rolled layer is gone; `LocalFsBlobStore` is a thin constructor that
//! wires the adapter up with the on-disk URI scheme (`file://<root>/blobs/…`)
//! the rest of the server already persists in `sessions.blob_uri`.
//!
//! On-disk layout is preserved exactly so existing data directories migrate
//! in-place — no schema or filesystem changes.

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use object_store::local::LocalFileSystem;
use object_store::ObjectStore;
use tokio::fs;
use uuid::Uuid;

use super::blob_object_store::ObjectStoreBlobStore;
use super::{BlobHandle, BlobStore, StorageError};

/// Local-filesystem blob store. Delegates to [`ObjectStoreBlobStore`] so the
/// only real difference between `LocalFsBlobStore` and the Azure-backed
/// constructor is which `object_store::ObjectStore` gets wrapped.
pub struct LocalFsBlobStore {
    inner: ObjectStoreBlobStore,
}

impl LocalFsBlobStore {
    /// Build a store rooted at `root`. Creates `root/staging` and
    /// `root/blobs` eagerly so first-request latency is predictable and so
    /// `LocalFileSystem::new_with_prefix` doesn't trip on a missing prefix.
    pub async fn new(root: impl Into<PathBuf>) -> Result<Self, StorageError> {
        let root: PathBuf = root.into();
        fs::create_dir_all(root.join("staging")).await?;
        fs::create_dir_all(root.join("blobs")).await?;

        let local = LocalFileSystem::new_with_prefix(&root)
            .map_err(|e| StorageError::ObjectStore(e.to_string()))?;
        let inner_store: Arc<dyn ObjectStore> = Arc::new(local);

        // Emit `file://<root>/blobs/<session_id>` to stay wire-compatible
        // with any `sessions.blob_uri` rows already committed by the
        // pre-refactor impl. The old code wrote `file://<abs path>` on unix
        // and `file:///<drive>:/…` on Windows; we normalize to forward
        // slashes and strip a leading slash so the final string matches
        // the historical format on both platforms.
        let host = root
            .to_string_lossy()
            .replace('\\', "/")
            .trim_start_matches('/')
            .to_string();

        let inner =
            ObjectStoreBlobStore::new(inner_store, root.join("staging"), "file", host).await?;
        Ok(Self { inner })
    }
}

#[async_trait]
impl BlobStore for LocalFsBlobStore {
    fn staging_path(&self, upload_id: Uuid) -> PathBuf {
        self.inner.staging_path(upload_id)
    }

    async fn create_staging(&self, upload_id: Uuid) -> Result<(), StorageError> {
        self.inner.create_staging(upload_id).await
    }

    async fn put_chunk(
        &self,
        upload_id: Uuid,
        offset: u64,
        bytes: &[u8],
    ) -> Result<(), StorageError> {
        self.inner.put_chunk(upload_id, offset, bytes).await
    }

    async fn hash(&self, upload_id: Uuid) -> Result<String, StorageError> {
        self.inner.hash(upload_id).await
    }

    async fn finalize(
        &self,
        upload_id: Uuid,
        session_id: Uuid,
    ) -> Result<BlobHandle, StorageError> {
        self.inner.finalize(upload_id, session_id).await
    }

    async fn discard_staging(&self, upload_id: Uuid) -> Result<(), StorageError> {
        self.inner.discard_staging(upload_id).await
    }

    async fn head_blob(&self, uri: &str) -> Result<u64, StorageError> {
        self.inner.head_blob(uri).await
    }

    async fn read_blob(&self, uri: &str) -> Result<Vec<u8>, StorageError> {
        self.inner.read_blob(uri).await
    }

    async fn delete_blob(&self, uri: &str) -> Result<(), StorageError> {
        self.inner.delete_blob(uri).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn put_hash_finalize_round_trip() {
        let tmp = TempDir::new().unwrap();
        let store = LocalFsBlobStore::new(tmp.path()).await.unwrap();

        let upload_id = Uuid::now_v7();
        let session_id = Uuid::now_v7();
        store.create_staging(upload_id).await.unwrap();
        store.put_chunk(upload_id, 0, b"hello ").await.unwrap();
        store.put_chunk(upload_id, 6, b"world").await.unwrap();

        let h = store.hash(upload_id).await.unwrap();
        // Precomputed sha256("hello world")
        assert_eq!(
            h,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );

        let handle = store.finalize(upload_id, session_id).await.unwrap();
        assert_eq!(handle.size_bytes, 11);
        assert!(handle.uri.starts_with("file://"));

        // Ensure the URI we emit is readable back through the trait — this
        // is the invariant the parse worker relies on.
        let bytes = store.read_blob(&handle.uri).await.unwrap();
        assert_eq!(bytes, b"hello world");
    }

    #[tokio::test]
    async fn discard_missing_is_ok() {
        let tmp = TempDir::new().unwrap();
        let store = LocalFsBlobStore::new(tmp.path()).await.unwrap();
        store.discard_staging(Uuid::now_v7()).await.unwrap();
    }
}
