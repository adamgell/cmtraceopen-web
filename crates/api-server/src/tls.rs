//! TLS termination + mTLS client-cert verification.
//!
//! This module is gated behind the `mtls` Cargo feature. Disabling the
//! feature drops the entire `axum-server` / `rustls` / `aws-lc-sys` dep
//! tree, which keeps the build path on dev boxes that lack cmake/NASM
//! (the C+asm toolchain `aws-lc-sys` needs at build time).
//!
//! # Bring-up
//!
//! [`serve_tls`] is the public entry point used by `main.rs`:
//!  1. Loads the server's own cert chain + key from PEM (the listener's
//!     identity).
//!  2. Loads the client-CA bundle (Gell - PKI Root + Issuing per the
//!     runbook in `docs/provisioning/03-intune-cloud-pki.md`) into a
//!     [`rustls::RootCertStore`].
//!  3. Builds a [`rustls::server::WebPkiClientVerifier`] over those
//!     anchors. When `require_on_ingest` is set, the verifier is wired in
//!     "required" mode (handshakes without a client cert fail). Otherwise
//!     it's wired with `allow_unauthenticated()` so that the legacy
//!     `X-Device-Id` header path keeps working — the
//!     [`crate::auth::device_identity::DeviceIdentity`] extractor decides
//!     per-route whether to enforce the cert.
//!  4. Wraps the standard [`axum_server::tls_rustls::RustlsAcceptor`] in
//!     [`PeerCertCapturingAcceptor`], which after the handshake snapshots
//!     the peer certificate chain off the rustls `ServerConnection` and
//!     wraps the per-connection service in [`InjectPeerCertService`] so
//!     every request lands at the handler with a [`PeerCertChain`]
//!     extension already attached.
//!
//! # Crypto provider
//!
//! We explicitly install `rustls`'s `aws_lc_rs` provider as the process
//! default. axum-server's `tls-rustls-no-provider` feature deliberately
//! does not pick a provider — without an explicit install rustls panics
//! on the first handshake. The install is idempotent (wrapped in a
//! `OnceCell`) so re-entering [`serve_tls`] in tests is safe.
//!
//! # Why a custom acceptor
//!
//! axum-server's stock `RustlsAcceptor` returns the `TlsStream` but only
//! after handing it to hyper for the HTTP serve loop. By the time a
//! request arrives at an Axum handler the original `ServerConnection`
//! handle (and therefore the peer cert) is buried under hyper's
//! connection state. Wrapping the acceptor lets us peel the peer-cert
//! list off the freshly-completed `ServerConnection` exactly once per
//! connection and stash it where every per-request handler can see it.

use std::io;
use std::net::SocketAddr;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::OnceLock;
use std::task::{Context, Poll};

use axum::Router;
use axum_server::accept::Accept;
use axum_server::tls_rustls::{RustlsAcceptor, RustlsConfig};
use http::Request;
use rustls::server::WebPkiClientVerifier;
use rustls::{RootCertStore, ServerConfig};
use rustls_pki_types::{CertificateDer, PrivateKeyDer};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_rustls::server::TlsStream;
use tower::Service;
use tracing::{debug, info, warn};

use crate::config::TlsConfig;

/// Peer certificate chain captured at TLS handshake time.
///
/// Shared (Arc) so cloning per request is cheap. The inner `Vec` is empty
/// when the client did not present a certificate (only possible when
/// `require_on_ingest` is `false` — i.e. the verifier is in
/// `allow_unauthenticated` mode).
#[derive(Clone, Debug, Default)]
pub struct PeerCertChain(pub Arc<Vec<CertificateDer<'static>>>);

impl PeerCertChain {
    /// Leaf cert (end-entity) the client presented, if any. None when no
    /// cert was sent or rustls didn't surface one (broken handshake path).
    pub fn leaf(&self) -> Option<&CertificateDer<'static>> {
        self.0.first()
    }
}

/// Errors that can stop TLS bring-up at startup. Surfaced to `main.rs`
/// where they print and exit non-zero.
#[derive(Debug, thiserror::Error)]
pub enum TlsBringupError {
    #[error("io error reading {what}: {err}")]
    Io {
        what: &'static str,
        #[source]
        err: io::Error,
    },
    #[error("no certificates found in {0}")]
    NoCertsInPem(String),
    #[error("no private key found in {0}")]
    NoKeyInPem(String),
    #[error("rustls config: {0}")]
    Rustls(#[from] rustls::Error),
    #[error("client verifier build: {0}")]
    Verifier(#[from] rustls::server::VerifierBuilderError),
    #[error("invalid CA cert in bundle: {0}")]
    InvalidCa(String),
    #[error("server hyper: {0}")]
    Server(#[source] io::Error),
    #[error("crypto provider already installed (different provider)")]
    ProviderAlreadyInstalled,
}

/// Idempotently install the aws-lc-rs crypto provider as rustls' process
/// default. Safe to call from concurrent test starts.
fn install_default_crypto_provider() -> Result<(), TlsBringupError> {
    static INSTALLED: OnceLock<()> = OnceLock::new();
    INSTALLED.get_or_init(|| {
        // The result is only an error if a *different* provider was
        // already installed. In our build that can only happen if a
        // dependency snuck in `ring`; ignore here and let the actual
        // handshake fail loudly downstream.
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    });
    Ok(())
}

/// Test-only re-export of [`install_default_crypto_provider`]. Lets the
/// gated `mtls_integration` test set the rustls process provider before
/// it constructs its own `ClientConfig` (which would otherwise panic).
pub fn install_default_crypto_provider_for_tests() {
    let _ = install_default_crypto_provider();
}

/// Public entry point. Build a TLS-terminating server, attach our
/// peer-cert-capturing acceptor on top of axum-server's rustls acceptor,
/// and serve.
pub async fn serve_tls(
    addr: SocketAddr,
    app: Router,
    cfg: &TlsConfig,
) -> Result<(), TlsBringupError> {
    serve_tls_inner(addr, app, cfg, None).await
}

/// Same as [`serve_tls`] but lets the caller pass an axum-server
/// [`axum_server::Handle`] in for graceful-shutdown plumbing AND to learn
/// the bound port (handy for ephemeral-port integration tests).
pub async fn serve_tls_with_handle(
    addr: SocketAddr,
    app: Router,
    cfg: &TlsConfig,
    handle: axum_server::Handle,
) -> Result<(), TlsBringupError> {
    serve_tls_inner(addr, app, cfg, Some(handle)).await
}

async fn serve_tls_inner(
    addr: SocketAddr,
    app: Router,
    cfg: &TlsConfig,
    handle: Option<axum_server::Handle>,
) -> Result<(), TlsBringupError> {
    install_default_crypto_provider()?;

    let cert_path = cfg
        .server_cert_pem
        .as_ref()
        .expect("Config::from_env validates this when tls.enabled");
    let key_path = cfg
        .server_key_pem
        .as_ref()
        .expect("Config::from_env validates this when tls.enabled");
    let ca_path = cfg
        .client_ca_bundle
        .as_ref()
        .expect("Config::from_env validates this when tls.enabled");

    let server_certs = load_certs_from_pem(cert_path)?;
    let server_key = load_key_from_pem(key_path)?;
    let roots = load_roots_from_pem(ca_path)?;

    let mut verifier_builder = WebPkiClientVerifier::builder(Arc::new(roots));
    if !cfg.require_on_ingest {
        // Transitional mode — the handshake won't fail when no cert is
        // sent; the per-route extractor decides whether to 401. This
        // lets fleets cut over device by device.
        verifier_builder = verifier_builder.allow_unauthenticated();
    }
    let verifier = verifier_builder.build()?;

    let mut server_config = ServerConfig::builder()
        .with_client_cert_verifier(verifier)
        .with_single_cert(server_certs, server_key)?;
    // h2 + http/1.1, matching axum-server's defaults.
    server_config.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];

    let rustls_config = RustlsConfig::from_config(Arc::new(server_config));
    let inner_acceptor = RustlsAcceptor::new(rustls_config);
    let acceptor = PeerCertCapturingAcceptor { inner: inner_acceptor };

    info!(
        require_client_cert = cfg.require_on_ingest,
        san_uri_scheme = %cfg.expected_san_uri_scheme,
        cert_path = %cert_path.display(),
        ca_path = %ca_path.display(),
        "TLS termination ready",
    );

    // axum-server's `Server::handle` consumes self and returns Self.
    // Branch once on the optional handle so the surrounding async block
    // sees a single concrete future type.
    let server = axum_server::bind(addr).acceptor(acceptor);
    let result = if let Some(h) = handle {
        server.handle(h).serve(app.into_make_service()).await
    } else {
        server.serve(app.into_make_service()).await
    };
    result.map_err(TlsBringupError::Server)
}

// ---------------------------------------------------------------------------
// PEM loading
// ---------------------------------------------------------------------------

fn load_certs_from_pem(path: &Path) -> Result<Vec<CertificateDer<'static>>, TlsBringupError> {
    let bytes = std::fs::read(path).map_err(|err| TlsBringupError::Io {
        what: "server cert pem",
        err,
    })?;
    let mut reader = bytes.as_slice();
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut reader)
        .filter_map(Result::ok)
        .collect();
    if certs.is_empty() {
        return Err(TlsBringupError::NoCertsInPem(path.display().to_string()));
    }
    Ok(certs)
}

fn load_key_from_pem(path: &Path) -> Result<PrivateKeyDer<'static>, TlsBringupError> {
    let bytes = std::fs::read(path).map_err(|err| TlsBringupError::Io {
        what: "server key pem",
        err,
    })?;
    let mut reader = bytes.as_slice();
    // Tolerate any of the standard PKCS#1 / PKCS#8 / SEC1 layouts. We pick
    // the first key that parses; if the file has multiple, that's almost
    // always operator error — log a warn and use the first.
    let mut keys = rustls_pemfile::read_all(&mut reader).filter_map(Result::ok);
    while let Some(item) = keys.next() {
        match item {
            rustls_pemfile::Item::Pkcs1Key(k) => return Ok(PrivateKeyDer::Pkcs1(k)),
            rustls_pemfile::Item::Pkcs8Key(k) => return Ok(PrivateKeyDer::Pkcs8(k)),
            rustls_pemfile::Item::Sec1Key(k) => return Ok(PrivateKeyDer::Sec1(k)),
            _ => continue,
        }
    }
    Err(TlsBringupError::NoKeyInPem(path.display().to_string()))
}

fn load_roots_from_pem(path: &Path) -> Result<RootCertStore, TlsBringupError> {
    let bytes = std::fs::read(path).map_err(|err| TlsBringupError::Io {
        what: "client CA bundle",
        err,
    })?;
    let mut reader = bytes.as_slice();
    let mut roots = RootCertStore::empty();
    let mut added = 0usize;
    let mut rejected = 0usize;
    for cert in rustls_pemfile::certs(&mut reader).filter_map(Result::ok) {
        match roots.add(cert.clone()) {
            Ok(()) => added += 1,
            Err(err) => {
                warn!(error = %err, "skipping invalid CA cert in client bundle");
                rejected += 1;
            }
        }
    }
    if added == 0 {
        return Err(TlsBringupError::InvalidCa(format!(
            "no usable CAs in {} (rejected {})",
            path.display(),
            rejected
        )));
    }
    debug!(added, rejected, "client CA bundle loaded");
    Ok(roots)
}

// ---------------------------------------------------------------------------
// Custom acceptor: peer-cert capture + per-request extension injection
// ---------------------------------------------------------------------------

/// Wraps axum-server's [`RustlsAcceptor`]. After the handshake completes,
/// snapshots the peer cert chain off the rustls `ServerConnection` and
/// attaches it to every request handled on this connection by wrapping
/// the per-connection service.
#[derive(Clone)]
pub struct PeerCertCapturingAcceptor {
    inner: RustlsAcceptor,
}

impl<I, S> Accept<I, S> for PeerCertCapturingAcceptor
where
    I: AsyncRead + AsyncWrite + Unpin + Send + 'static,
    S: Send + 'static,
{
    type Stream = TlsStream<I>;
    type Service = InjectPeerCertService<S>;
    type Future = Pin<
        Box<
            dyn std::future::Future<
                    Output = io::Result<(Self::Stream, Self::Service)>,
                > + Send,
        >,
    >;

    fn accept(&self, stream: I, service: S) -> Self::Future {
        let inner_future = <RustlsAcceptor as Accept<I, S>>::accept(&self.inner, stream, service);
        Box::pin(async move {
            let (tls_stream, inner_service) = inner_future.await?;
            // Pull peer-cert chain off the handshake we just finished.
            // `peer_certificates` returns Some(slice) when the client
            // presented one, None otherwise (only possible under
            // `allow_unauthenticated`).
            let chain: Vec<CertificateDer<'static>> = {
                let (_, conn) = tls_stream.get_ref();
                conn.peer_certificates()
                    .map(|certs| certs.iter().map(|c| c.clone().into_owned()).collect())
                    .unwrap_or_default()
            };
            let chain = PeerCertChain(Arc::new(chain));
            Ok((tls_stream, InjectPeerCertService { inner: inner_service, chain }))
        })
    }
}

/// Per-connection tower service wrapper. Inserts the captured
/// [`PeerCertChain`] into each request's extensions before dispatching to
/// the inner service.
#[derive(Clone)]
pub struct InjectPeerCertService<S> {
    inner: S,
    chain: PeerCertChain,
}

impl<S, B> Service<Request<B>> for InjectPeerCertService<S>
where
    S: Service<Request<B>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = S::Future;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<B>) -> Self::Future {
        req.extensions_mut().insert(self.chain.clone());
        self.inner.call(req)
    }
}

// axum-server's `SendService` is a sealed trait with a blanket impl over
// any `Service<Request<B>> + Send + Clone + 'static` whose response is
// `http::Response<B>`. `InjectPeerCertService<S>` satisfies those bounds
// automatically when `S` does, so there is nothing to implement here.
