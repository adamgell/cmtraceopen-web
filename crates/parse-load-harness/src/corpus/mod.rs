//! Corpus = bundle source. Synthetic generator and directory-replay both
//! implement this; the driver doesn't know which it's getting.

pub mod replay;
pub mod shape;
pub mod synthetic;

use crate::bundle::Bundle;

#[async_trait::async_trait]
pub trait Corpus: Send + Sync {
    /// Produce the next bundle. The driver tags `seq` and `device_id`
    /// before sending; the corpus only needs to fill `bundle_id`,
    /// `n_files`, and `zip_bytes`.
    async fn next(&mut self) -> anyhow::Result<Bundle>;
}
