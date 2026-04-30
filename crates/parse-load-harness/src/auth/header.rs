//! `X-Device-Id` header — pilot-compatible. Default for HTTP targets.

use crate::auth::Auth;
use reqwest::RequestBuilder;

pub struct HeaderAuth;

impl Auth for HeaderAuth {
    fn apply(&self, req: RequestBuilder, device_id: &str) -> RequestBuilder {
        req.header("X-Device-Id", device_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_auth_constructs_without_panicking() {
        let _ = HeaderAuth;
    }
}
