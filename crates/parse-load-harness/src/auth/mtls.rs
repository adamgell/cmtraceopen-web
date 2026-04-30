//! In-memory test CA + per-virtual-device leaf certs. Local-http target only.
//! Wraps `rcgen` so we don't depend on `openssl`.

use crate::auth::Auth;
use rcgen::{CertificateParams, DnType, KeyPair};
use reqwest::RequestBuilder;
use std::collections::HashMap;
use std::sync::Mutex;

pub struct MtlsAuth {
    ca_pem: String,
    leaf_cache: Mutex<HashMap<String, (String, String)>>, // device_id -> (cert, key)
    ca_cert: rcgen::Certificate,
    ca_key: KeyPair,
}

impl MtlsAuth {
    pub fn new() -> anyhow::Result<Self> {
        let mut params = CertificateParams::new(vec!["cmtrace-load-test-ca".into()])?;
        params.distinguished_name.push(DnType::CommonName, "cmtrace-load-test-ca");
        let key = KeyPair::generate()?;
        let ca_cert = params.self_signed(&key)?;
        let ca_pem = ca_cert.pem();
        Ok(Self {
            ca_pem,
            leaf_cache: Mutex::new(HashMap::new()),
            ca_cert,
            ca_key: key,
        })
    }

    pub fn ca_pem(&self) -> &str {
        &self.ca_pem
    }

    /// Mint (or fetch from cache) a leaf cert + key for a given device_id.
    /// Returned strings are PEM-encoded.
    pub fn leaf_for(&self, device_id: &str) -> anyhow::Result<(String, String)> {
        if let Some(v) = self.leaf_cache.lock().unwrap().get(device_id).cloned() {
            return Ok(v);
        }
        let mut params = CertificateParams::new(vec![device_id.to_string()])?;
        params.distinguished_name.push(DnType::CommonName, device_id);
        let leaf_key = KeyPair::generate()?;
        let leaf = params.signed_by(&leaf_key, &self.ca_cert, &self.ca_key)?;
        let cert_pem = leaf.pem();
        let key_pem = leaf_key.serialize_pem();
        let entry = (cert_pem, key_pem);
        self.leaf_cache
            .lock()
            .unwrap()
            .insert(device_id.into(), entry.clone());
        Ok(entry)
    }
}

impl Auth for MtlsAuth {
    /// mTLS auth attaches via the reqwest *client* (set up at target startup),
    /// not per-request. This impl is a no-op so the trait stays single-shape;
    /// the local-http target swaps the http client when --auth=mtls is set.
    fn apply(&self, req: RequestBuilder, _device_id: &str) -> RequestBuilder {
        req
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mints_ca_and_leaf_certs() {
        let a = MtlsAuth::new().unwrap();
        assert!(a.ca_pem().contains("BEGIN CERTIFICATE"));
        let (cert, key) = a.leaf_for("LOAD-TEST-A-0001").unwrap();
        assert!(cert.contains("BEGIN CERTIFICATE"));
        assert!(key.contains("PRIVATE KEY"));
    }

    #[test]
    fn leaf_cache_returns_same_pair() {
        let a = MtlsAuth::new().unwrap();
        let p1 = a.leaf_for("dev-1").unwrap();
        let p2 = a.leaf_for("dev-1").unwrap();
        assert_eq!(p1, p2);
    }
}
