//! Auth strategies for HTTP targets. The in-process target stamps a
//! `DeviceIdentity` directly and bypasses this trait.

pub mod header;
pub mod mtls;

use reqwest::RequestBuilder;

pub trait Auth: Send + Sync {
    /// Stamp a request with whatever the target needs (header, client cert
    /// already configured on the client, etc.).
    fn apply(&self, req: RequestBuilder, device_id: &str) -> RequestBuilder;
}
