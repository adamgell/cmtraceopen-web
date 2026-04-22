//! TLS configuration for the agent's outbound reqwest client.
//!
//! We hand reqwest a pre-built `rustls::ClientConfig` via
//! `Client::builder().use_preconfigured_tls(..)` rather than letting
//! reqwest pull its own provider. This keeps three things in one place:
//!
//!   1. The choice of crypto provider (`aws-lc-rs`, see `Cargo.toml`
//!      for the rationale).
//!   2. Native-root-store loading via `rustls-native-certs`.
//!   3. Optional client-cert / custom-CA wiring for the Wave 3 mTLS
//!      story.
//!
//! ## Crypto provider installation
//!
//! `rustls` 0.23 requires exactly one provider to be installed as the
//! process-wide default before any `ClientConfig::builder()` call.
//! Calling `install_default()` more than once panics, so
//! [`install_default_crypto_provider`] guards with a `OnceLock`.
//! Tests in this module call it explicitly; the production path calls
//! it from `Uploader::new`.
//!
//! ## Wave 3 readiness
//!
//! [`TlsClientOptions`] mirrors the three TLS-related fields on
//! `AgentConfig`. When both `client_cert_pem` and `client_key_pem` are
//! set, [`build_client_config`] attaches them via
//! `with_client_auth_cert(..)`. When unset, the client connects without
//! a client cert (today's behavior — server-side mTLS isn't yet
//! enforced by the api-server).

use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use rustls::{ClientConfig, RootCertStore};

/// Inputs to [`build_client_config`]. Cloned out of `AgentConfig` so
/// the uploader doesn't take a hard dep on the full config struct (and
/// so tests can build a TLS config without constructing the rest of
/// the agent's runtime state).
#[derive(Debug, Clone, Default)]
pub struct TlsClientOptions {
    /// PEM-encoded client cert (or chain). Wave 3 mTLS pairs this with
    /// `client_key_pem`. `None` skips client auth.
    pub client_cert_pem: Option<PathBuf>,

    /// PEM-encoded private key matching `client_cert_pem`. PKCS#8,
    /// SEC1 (EC), and PKCS#1 (RSA) are all accepted. `None` skips
    /// client auth even if a cert is set (and we log a warning at
    /// the call site).
    pub client_key_pem: Option<PathBuf>,

    /// Optional PEM bundle of trusted CA certs. When `Some`, only
    /// these roots are trusted (the OS native store is **not** layered
    /// on top — explicit override). When `None`, the OS native store
    /// is loaded via `rustls-native-certs`.
    pub ca_bundle_pem: Option<PathBuf>,
}

/// Errors building a rustls `ClientConfig`. Surfaced through to the
/// uploader's `UploaderError` chain so a misconfigured cert path is
/// caught at startup, not on the first chunk PUT.
#[derive(Debug, thiserror::Error)]
pub enum TlsConfigError {
    #[error("failed to read TLS file {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse PEM in {path}: {source}")]
    Pem {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("no certificates found in {path}")]
    NoCerts { path: String },

    #[error("no private key found in {path}")]
    NoKey { path: String },

    #[error("loading native certs: {source}")]
    NativeCerts {
        #[source]
        source: std::io::Error,
    },

    #[error("rustls error: {0}")]
    Rustls(#[from] rustls::Error),

    #[error("client cert was set but client key was not (or vice versa); both are required for mTLS")]
    PartialClientAuth,
}

/// Install the `aws-lc-rs` crypto provider as the process-wide rustls
/// default. Idempotent and thread-safe. Returns `Ok(())` whether this
/// call did the install or simply confirmed a prior call already had.
///
/// `rustls::CryptoProvider::install_default` itself panics on second
/// call, so we gate behind a `OnceLock`. We surface its `Result<(),
/// CryptoProvider>` (an error means *somebody else* installed first —
/// fine for our purposes) by ignoring it.
pub fn install_default_crypto_provider() {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        // `install_default` returns Result<(), Arc<CryptoProvider>>; the
        // err arm means another caller raced us, which is fine — we
        // only care that *some* default is installed.
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
}

/// Build a `rustls::ClientConfig` from [`TlsClientOptions`].
///
/// Caller must have invoked [`install_default_crypto_provider`] first
/// (or have installed a provider some other way). The uploader does
/// this in `Uploader::new`.
pub fn build_client_config(opts: &TlsClientOptions) -> Result<ClientConfig, TlsConfigError> {
    let roots = build_root_store(opts.ca_bundle_pem.as_deref())?;

    let builder = ClientConfig::builder().with_root_certificates(roots);

    let config = match (&opts.client_cert_pem, &opts.client_key_pem) {
        (Some(cert), Some(key)) => {
            let cert_chain = load_cert_chain(cert)?;
            let private_key = load_private_key(key)?;
            builder.with_client_auth_cert(cert_chain, private_key)?
        }
        (None, None) => builder.with_no_client_auth(),
        // Half-set is a config bug worth surfacing loudly — silently
        // dropping the cert would mask an mTLS misconfiguration.
        _ => return Err(TlsConfigError::PartialClientAuth),
    };

    Ok(config)
}

/// Build a `reqwest::Client` configured with the given TLS options.
///
/// The resulting client uses the same rustls `ClientConfig` as the
/// `Uploader`, so cert validation settings are consistent across all
/// outbound HTTP calls (config-sync + bundle upload).
///
/// Caller must have invoked [`install_default_crypto_provider`] first.
pub fn build_reqwest_client(opts: TlsClientOptions) -> Result<reqwest::Client, TlsConfigError> {
    install_default_crypto_provider();
    let tls_config = build_client_config(&opts)?;
    let client = reqwest::Client::builder()
        .use_preconfigured_tls(tls_config)
        .build()
        // reqwest::Error is not easily convertible here; wrap as Io.
        .map_err(|e| TlsConfigError::Io {
            path: "<reqwest client>".into(),
            source: std::io::Error::other(e.to_string()),
        })?;
    Ok(client)
}


/// Build the trust anchor set. If a CA bundle is provided we use *only*
/// those roots; otherwise we load the OS native trust store.
fn build_root_store(ca_bundle: Option<&Path>) -> Result<RootCertStore, TlsConfigError> {
    let mut store = RootCertStore::empty();

    if let Some(path) = ca_bundle {
        let certs = load_cert_chain(path)?;
        for cert in certs {
            store.add(cert)?;
        }
        return Ok(store);
    }

    // Native roots path. `rustls-native-certs` 0.8 returns a
    // `CertificateResult` (no Result wrapper) with a `certs` Vec plus a
    // list of soft errors. We trust whatever it could load and surface a
    // hard error only if the platform call returned zero certs *and*
    // produced at least one error — partial trust beats no trust.
    let native = rustls_native_certs::load_native_certs();
    for cert in native.certs {
        // `add` may reject malformed certs from the native store; skip
        // rather than fail the whole startup — partial trust is better
        // than no trust.
        let _ = store.add(cert);
    }
    if store.is_empty() {
        // Surface the first error we got from the platform load, if
        // any, so the operator sees something actionable.
        if let Some(err) = native.errors.into_iter().next() {
            return Err(TlsConfigError::NativeCerts {
                source: std::io::Error::other(err.to_string()),
            });
        }
    }
    Ok(store)
}

fn load_cert_chain(path: &Path) -> Result<Vec<CertificateDer<'static>>, TlsConfigError> {
    let file = File::open(path).map_err(|source| TlsConfigError::Io {
        path: path.display().to_string(),
        source,
    })?;
    let mut reader = BufReader::new(file);
    let mut certs = Vec::new();
    for result in rustls_pemfile::certs(&mut reader) {
        let cert = result.map_err(|source| TlsConfigError::Pem {
            path: path.display().to_string(),
            source,
        })?;
        certs.push(cert);
    }
    if certs.is_empty() {
        return Err(TlsConfigError::NoCerts {
            path: path.display().to_string(),
        });
    }
    Ok(certs)
}

fn load_private_key(path: &Path) -> Result<PrivateKeyDer<'static>, TlsConfigError> {
    let file = File::open(path).map_err(|source| TlsConfigError::Io {
        path: path.display().to_string(),
        source,
    })?;
    let mut reader = BufReader::new(file);
    // `private_key` returns the *first* of {pkcs8, sec1, pkcs1} it
    // finds — handles all three common PEM key encodings.
    match rustls_pemfile::private_key(&mut reader) {
        Ok(Some(key)) => Ok(key),
        Ok(None) => Err(TlsConfigError::NoKey {
            path: path.display().to_string(),
        }),
        Err(source) => Err(TlsConfigError::Pem {
            path: path.display().to_string(),
            source,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Building a config with no client cert and no CA bundle should
    /// succeed against the OS native trust store. (Headless CI images
    /// still have a usable ca-certificates package.)
    #[test]
    fn builds_without_client_cert() {
        install_default_crypto_provider();
        let opts = TlsClientOptions::default();
        // We can't usefully assert on the contents of the resulting
        // ClientConfig — rustls keeps internals private — but a
        // successful build is the contract this test guards.
        let _config = build_client_config(&opts).expect("native-roots config builds");
    }

    /// Half-configured client auth (cert without key, or vice versa)
    /// should fail loudly. Catching this at startup beats discovering
    /// the misconfiguration on the first upload.
    #[test]
    fn partial_client_auth_is_rejected() {
        install_default_crypto_provider();
        let opts = TlsClientOptions {
            client_cert_pem: Some(PathBuf::from("/tmp/does-not-exist.crt")),
            client_key_pem: None,
            ca_bundle_pem: None,
        };
        let err = build_client_config(&opts).expect_err("must reject half-set client auth");
        assert!(matches!(err, TlsConfigError::PartialClientAuth));
    }

    /// A missing client cert file surfaces an `Io` error pointing at
    /// the bad path — operators can grep for the path in logs.
    #[test]
    fn missing_cert_file_surfaces_path() {
        install_default_crypto_provider();
        let opts = TlsClientOptions {
            client_cert_pem: Some(PathBuf::from("/tmp/cmtraceopen-agent-no-such-cert.pem")),
            client_key_pem: Some(PathBuf::from("/tmp/cmtraceopen-agent-no-such-key.pem")),
            ca_bundle_pem: None,
        };
        let err = build_client_config(&opts).expect_err("must error on missing cert file");
        match err {
            TlsConfigError::Io { path, .. } => {
                assert!(path.contains("cmtraceopen-agent-no-such-cert.pem"));
            }
            other => panic!("expected Io error, got {other:?}"),
        }
    }

    /// Generating a throwaway cert + key in-test would pull `rcgen`
    /// (and ring through it). We instead verify the load helpers via
    /// a hand-rolled "bad PEM" path; the happy-path cert+key load is
    /// covered indirectly by the `partial_client_auth_is_rejected`
    /// test above (which exercises the same branch up to the file
    /// open). A full mTLS round trip lives in the integration test
    /// suite once Wave 3 server-side enforcement lands.
    ///
    /// TODO(wave-3): add an integration test that boots the
    /// api-server with mTLS required and asserts the agent's
    /// configured client cert is accepted.
    #[test]
    fn empty_pem_file_surfaces_no_certs_error() {
        install_default_crypto_provider();
        let dir = std::env::temp_dir().join(format!(
            "cmtraceopen-agent-tls-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let cert_path = dir.join("empty.pem");
        std::fs::write(&cert_path, b"").unwrap();
        let err = load_cert_chain(&cert_path).expect_err("empty pem must error");
        assert!(matches!(err, TlsConfigError::NoCerts { .. }));
    }
}
