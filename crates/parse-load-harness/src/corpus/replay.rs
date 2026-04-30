//! Replay = a `Corpus` backed by a directory of real `.zip` files.

use crate::bundle::Bundle;
use crate::corpus::Corpus;
use std::path::PathBuf;

const MAX_EVIDENCE_ZIP_BYTES: u64 = 50 * 1024 * 1024;

pub struct ReplayCorpus {
    paths: Vec<PathBuf>,
    cursor: usize,
}

impl ReplayCorpus {
    pub fn new(dir: &std::path::Path) -> anyhow::Result<Self> {
        let mut paths: Vec<PathBuf> = std::fs::read_dir(dir)?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("zip"))
            .filter(|p| {
                p.metadata()
                    .map(|m| m.len() <= MAX_EVIDENCE_ZIP_BYTES)
                    .unwrap_or(false)
            })
            .collect();
        paths.sort();
        anyhow::ensure!(!paths.is_empty(), "no usable .zip files in {dir:?}");
        Ok(Self { paths, cursor: 0 })
    }
}

#[async_trait::async_trait]
impl Corpus for ReplayCorpus {
    async fn next(&mut self) -> anyhow::Result<Bundle> {
        let path = &self.paths[self.cursor % self.paths.len()];
        self.cursor = self.cursor.wrapping_add(1);
        let zip_bytes = tokio::fs::read(path).await?;
        let n_files = {
            let cursor = std::io::Cursor::new(&zip_bytes);
            zip::ZipArchive::new(cursor)?.len() as u32
        };
        Ok(Bundle {
            seq: 0,
            device_id: String::new(),
            bundle_id: uuid::Uuid::now_v7(),
            n_files,
            zip_bytes,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_min_zip(path: &std::path::Path) {
        let f = std::fs::File::create(path).unwrap();
        let mut zw = zip::ZipWriter::new(f);
        let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default();
        zw.start_file("a.log", opts).unwrap();
        zw.write_all(b"hi\n").unwrap();
        zw.finish().unwrap();
    }

    #[tokio::test]
    async fn round_robins_through_directory() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["a.zip", "b.zip", "c.zip"] {
            write_min_zip(&dir.path().join(name));
        }
        let mut c = ReplayCorpus::new(dir.path()).unwrap();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..6 {
            seen.insert(c.next().await.unwrap().bundle_id);
        }
        assert_eq!(seen.len(), 6, "each call gets a fresh bundle_id even on replay");
    }

    #[tokio::test]
    async fn errors_on_empty_directory() {
        let dir = tempfile::tempdir().unwrap();
        assert!(ReplayCorpus::new(dir.path()).is_err());
    }
}
