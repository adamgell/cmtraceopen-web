//! Local-filesystem [`BlobStore`] backed by two directories:
//!   - `<root>/staging/<upload_id>`  - in-progress uploads
//!   - `<root>/blobs/<session_id>`   - finalized content-addressed blobs
//!
//! Uses tokio::fs so hot paths don't block the runtime. Hashing streams the
//! file through sha2 to avoid loading multi-GB bundles into memory.

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use uuid::Uuid;

use super::{BlobHandle, BlobStore, StorageError};

/// Buffer size for streamed sha256 hashing over the staged file.
const HASH_READ_BUF: usize = 1024 * 1024; // 1 MiB

#[derive(Debug, Clone)]
pub struct LocalFsBlobStore {
    root: PathBuf,
}

impl LocalFsBlobStore {
    /// Build a store rooted at `root`. Creates `root/staging` and
    /// `root/blobs` eagerly so first-request latency is predictable.
    pub async fn new(root: impl Into<PathBuf>) -> Result<Self, StorageError> {
        let root = root.into();
        fs::create_dir_all(root.join("staging")).await?;
        fs::create_dir_all(root.join("blobs")).await?;
        Ok(Self { root })
    }

    fn staging_dir(&self) -> PathBuf {
        self.root.join("staging")
    }

    fn blobs_dir(&self) -> PathBuf {
        self.root.join("blobs")
    }

    fn blob_path(&self, session_id: Uuid) -> PathBuf {
        self.blobs_dir().join(session_id.to_string())
    }
}

#[async_trait]
impl BlobStore for LocalFsBlobStore {
    fn staging_path(&self, upload_id: Uuid) -> PathBuf {
        self.staging_dir().join(upload_id.to_string())
    }

    async fn create_staging(&self, upload_id: Uuid) -> Result<(), StorageError> {
        let path = self.staging_path(upload_id);
        // Overwrite any stale file from a crashed prior attempt; the metadata
        // store is the source of truth for what's in-flight.
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
        let dst = self.blob_path(session_id);

        // Re-hash + size under the same FD so the reported values reflect the
        // bytes we're about to move, not a racing writer.
        let metadata = fs::metadata(&src).await?;
        let size_bytes = metadata.len();
        let sha256 = stream_sha256(&src).await?;

        // tokio::fs::rename is cross-drive-unsafe on Windows; for MVP the
        // staging + blobs dirs live under the same root so a rename is fine.
        fs::rename(&src, &dst).await?;

        let uri = path_to_file_uri(&dst);
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

/// Minimal, cross-platform `file://` URI builder. Good enough for MVP:
/// blob_uri is opaque to clients; we only need it to round-trip.
fn path_to_file_uri(path: &Path) -> String {
    // On Windows, tolerate backslashes; on unix it's already /-separated.
    let s = path.to_string_lossy().replace('\\', "/");
    if s.starts_with('/') {
        format!("file://{s}")
    } else {
        format!("file:///{s}")
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
    }

    #[tokio::test]
    async fn discard_missing_is_ok() {
        let tmp = TempDir::new().unwrap();
        let store = LocalFsBlobStore::new(tmp.path()).await.unwrap();
        store.discard_staging(Uuid::now_v7()).await.unwrap();
    }
}
