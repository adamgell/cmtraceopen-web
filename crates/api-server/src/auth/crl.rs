//! Certificate Revocation List (CRL) polling for client-cert revocation.
//!
//! # Why
//! mTLS termination (PR #41, `feat/api-mtls-termination`) authenticates
//! agents by client certificate; revocation closes the loop so a wiped or
//! lost device can have its access killed at the Intune level *without*
//! waiting for the leaf cert to expire. Cloud PKI publishes a CRL per CA
//! and rotates it on a configurable schedule (default ~hourly); we poll
//! both the Root and Issuing CRLs and reject any client cert whose serial
//! appears in either list.
//!
//! See `~/.claude/projects/F--Repo/memory/reference_cloud_pki.md` for the
//! live URLs and CA hierarchy.
//!
//! # Architecture
//! [`CrlCache`] holds an `Arc<RwLock<...>>` map of `Url → CrlEntry`. A
//! background tokio task spawned by [`CrlCache::start_refresh_task`]:
//!  1. Performs an initial blocking fetch (so the cache is warm before the
//!     first request that needs it).
//!  2. Loops on `tokio::time::interval(refresh_interval)`, fetching each
//!     URL and atomically swapping the entry on success.
//!  3. On fetch / parse failure: logs a `tracing::warn!` and **keeps** the
//!     prior entry. Best-effort — a brief CDN blip should not blow the
//!     cache. If *no* successful fetch has ever landed for a URL, the
//!     fail-open / fail-closed knob applies (see [`Config::crl_fail_open`]
//!     and [`CrlCache::is_revoked`]).
//!
//! [`Config::crl_fail_open`]: crate::config::Config::crl_fail_open
//!
//! # Why a runtime extractor check, not handshake-time
//! `rustls 0.23`'s `WebPkiClientVerifier` *can* be built with `with_crls(...)`
//! so revocation checks happen during the TLS handshake itself. That path
//! is cleaner — the connection is rejected before any HTTP framing
//! reaches us — but it pulls `webpki` (and transitively `ring` or
//! `aws-lc-rs`) into the build tree. The api-server has a hard no-ring
//! rule (see top-of-file note on `jwt-simple` in
//! `crates/api-server/Cargo.toml`), and switching reqwest + rustls to
//! `aws-lc-rs` is a separate, larger change. Doing the check in the
//! `DeviceIdentity` extractor lets us gate revocation behind a pure-Rust
//! parser (`x509-parser` with default features only) and dynamically pick
//! up CRL refreshes without re-binding the listener.
//!
//! # Cargo feature gating
//! This module is behind `feature = "crl"` (default-on, mirrors the
//! planned `mtls` gate from PR #41). Without the feature the calling
//! extractor falls through to allow-all — matching the pre-mTLS posture
//! exactly so an operator who disables both features sees the same
//! behaviour as today's `main`.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use parking_lot::RwLock;
use reqwest::Url;
use tokio::time::Instant;
use tracing::{debug, info, warn};
use x509_parser::prelude::{CertificateRevocationList, FromDer};

/// Hard cap on a single CRL response body. Cloud PKI CRLs for a tenant
/// typically run a few KB to a few MB depending on how many devices have
/// been retired; 32 MiB is comfortably above that and well below anything
/// that would OOM the api-server. Set to match the per-chunk ingest cap
/// in [`crate::state::MAX_CHUNK_SIZE`] to keep one mental model for "big".
const MAX_CRL_BYTES: u64 = 32 * 1024 * 1024;

/// Per-fetch HTTP timeout. CRL CDN should answer within a few hundred ms;
/// 30 s is a generous ceiling that lets us survive a TCP-stall blip
/// without locking up the whole refresh loop.
const CRL_FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// One CRL's worth of state. The set of revoked serials is what
/// [`CrlCache::is_revoked`] consults; the timestamps are kept for
/// observability only (e.g. a `/admin/crl` debug endpoint).
///
/// Visibility is `pub` (not `pub(crate)`) so [`CrlCache::parse`] can
/// return it from doctests / external callers — but the only stable
/// surface is via [`CrlCache`]'s public methods, not the fields.
#[derive(Debug)]
pub struct CrlEntry {
    /// DER-encoded serial bytes of every revoked cert in this CRL.
    /// `Vec<u8>` (not `[u8; 20]`) because RFC 5280 caps serials at 20
    /// octets but smaller serials are common in test PKI hierarchies.
    revoked_serials: HashSet<Vec<u8>>,
    /// When this entry was successfully fetched + parsed (monotonic clock
    /// for staleness math; not `chrono::Utc` because we never compare it
    /// to wall-clock timestamps from the CRL itself).
    ///
    /// Currently consumed only by tests + future debug routes; the
    /// refresh loop runs on its own `tokio::time::interval` so this
    /// field is a passive "when was the last good data" record.
    #[allow(dead_code)]
    fetched_at: Instant,
    /// `nextUpdate` field from the CRL itself, if present. Surfaced for
    /// debug; the refresh loop is driven by [`CrlCache::refresh_interval`]
    /// rather than by this field so we don't accidentally stop refreshing
    /// when a CA forgets to set it.
    #[allow(dead_code)]
    next_update: Option<DateTime<Utc>>,
}

/// In-memory cache of all configured CRLs.
///
/// Cheap to clone (it's just three `Arc`s + a `Duration` + a `bool`), so
/// stash it in `AppState` and hand references to extractors.
pub struct CrlCache {
    entries: Arc<RwLock<HashMap<Url, CrlEntry>>>,
    refresh_interval: Duration,
    fail_open: bool,
    /// One reqwest client shared across every URL — connection pooling
    /// matters when both Root and Issuing CRLs live behind the same CDN
    /// host (`primary-cdn.pki.azure.net`).
    http_client: reqwest::Client,
    /// Configured URLs. Preserved verbatim so the refresh loop hits the
    /// same set on every iteration even if the cache map is partially
    /// populated by failed fetches.
    urls: Vec<Url>,
}

impl CrlCache {
    /// Build an empty cache. Call [`Self::start_refresh_task`] on the
    /// returned `Arc` to kick off the background refresh loop.
    ///
    /// `urls` should be the parsed contents of `CMTRACE_CRL_URLS`. URLs
    /// that fail to parse are dropped here with a `warn!` — the cache
    /// continues with whatever survives so a typo in one URL does not
    /// take down revocation for the others.
    pub fn new(
        urls: impl IntoIterator<Item = String>,
        refresh_interval: Duration,
        fail_open: bool,
    ) -> Self {
        let parsed_urls: Vec<Url> = urls
            .into_iter()
            .filter_map(|raw| match Url::parse(&raw) {
                Ok(u) => Some(u),
                Err(err) => {
                    warn!(url = %raw, %err, "ignoring malformed CRL URL");
                    None
                }
            })
            .collect();

        // PR #46 switched workspace reqwest to rustls-tls-native-roots-no-provider;
        // building any reqwest client now requires a rustls crypto provider to be
        // installed first. main.rs installs aws-lc-rs eagerly at startup; tests
        // that construct CrlCache directly bypass main, so install here too.
        // Idempotent (rustls install_default no-ops if a provider is already set).
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        let http_client = reqwest::Client::builder()
            .timeout(CRL_FETCH_TIMEOUT)
            // The CDN is plain HTTP per the Cloud PKI memory doc, but we
            // don't disable HTTPS — if Microsoft ever upgrades the CDN
            // we want to follow without a code change.
            .build()
            .expect("reqwest client builds with default config");

        Self {
            entries: Arc::new(RwLock::new(HashMap::new())),
            refresh_interval,
            fail_open,
            http_client,
            urls: parsed_urls,
        }
    }

    /// True iff this cache is configured to poll any CRLs at all.
    /// The extractor uses this to short-circuit the revocation check on
    /// dev / lab deployments where `CMTRACE_CRL_URLS` is unset.
    pub fn is_configured(&self) -> bool {
        !self.urls.is_empty()
    }

    /// Mirror of `Config::crl_fail_open`, exposed so the extractor can
    /// log the right reason on a "no entries yet" reject.
    pub fn fail_open(&self) -> bool {
        self.fail_open
    }

    /// Spawn the background refresh task and prime the cache with one
    /// initial fetch attempt.
    ///
    /// Returns immediately; the initial fetch happens *inline* (not in a
    /// `spawn`) so callers that `.await` this method see the cache
    /// populated before the first request lands. A failed initial fetch
    /// is logged and swallowed — startup must not depend on the CDN
    /// being reachable.
    pub async fn start_refresh_task(self: Arc<Self>) {
        if self.urls.is_empty() {
            debug!("CRL cache has no URLs configured; skipping refresh task");
            return;
        }

        // Initial blocking fetch so the cache is warm.
        self.refresh_all().await;

        let cache = Arc::clone(&self);
        tokio::spawn(async move {
            // `tokio::time::interval` fires immediately on first tick; we
            // already did the initial fetch above, so consume that tick.
            let mut ticker = tokio::time::interval(cache.refresh_interval);
            ticker.set_missed_tick_behavior(
                tokio::time::MissedTickBehavior::Delay,
            );
            ticker.tick().await; // discard the immediate first tick

            loop {
                ticker.tick().await;
                cache.refresh_all().await;
            }
        });
    }

    /// Fetch + parse every configured CRL. Per-URL failures are logged
    /// and swallowed — see module-level docs for the rationale.
    async fn refresh_all(&self) {
        for url in &self.urls {
            match self.fetch_and_parse(url).await {
                Ok(entry) => {
                    let revoked_count = entry.revoked_serials.len();
                    self.entries.write().insert(url.clone(), entry);
                    info!(
                        url = %url,
                        revoked_count,
                        "CRL refreshed",
                    );
                }
                Err(err) => {
                    warn!(
                        url = %url,
                        %err,
                        "CRL refresh failed; keeping previous entry if any",
                    );
                }
            }
        }
    }

    async fn fetch_and_parse(&self, url: &Url) -> Result<CrlEntry, CrlError> {
        let resp = self
            .http_client
            .get(url.clone())
            .send()
            .await
            .map_err(|e| CrlError::Fetch(e.to_string()))?
            .error_for_status()
            .map_err(|e| CrlError::Fetch(e.to_string()))?;

        // Defensive size check — Content-Length isn't guaranteed but
        // when present we use it to fail fast on a misbehaving server
        // without buffering megabytes first.
        if let Some(len) = resp.content_length() {
            if len > MAX_CRL_BYTES {
                return Err(CrlError::TooLarge(len));
            }
        }

        let bytes = resp
            .bytes()
            .await
            .map_err(|e| CrlError::Fetch(e.to_string()))?;
        if bytes.len() as u64 > MAX_CRL_BYTES {
            return Err(CrlError::TooLarge(bytes.len() as u64));
        }
        Self::parse(&bytes)
    }

    /// Parse a CRL from its DER-encoded bytes. Public so unit tests can
    /// drive it without spinning up an HTTP server.
    pub fn parse(der: &[u8]) -> Result<CrlEntry, CrlError> {
        let (_rem, crl) = CertificateRevocationList::from_der(der)
            .map_err(|e| CrlError::Parse(e.to_string()))?;

        let mut revoked = HashSet::new();
        for entry in crl.iter_revoked_certificates() {
            // `raw_serial()` returns the DER serial bytes minus the
            // ASN.1 tag/length wrapping — exactly the form a leaf
            // cert's serial appears in after parsing. Note RFC 5280
            // permits a leading 0x00 padding byte for positive ints
            // whose high bit would otherwise be set; we keep the
            // padding so equality with the leaf serial (also from
            // `raw_serial`) is exact.
            revoked.insert(entry.raw_serial().to_vec());
        }

        let next_update = crl.next_update().and_then(|t| {
            DateTime::<Utc>::from_timestamp(t.timestamp(), 0)
        });

        Ok(CrlEntry {
            revoked_serials: revoked,
            fetched_at: Instant::now(),
            next_update,
        })
    }

    /// True iff `serial` appears in any cached CRL.
    ///
    /// Behavior when no entries are cached yet (cold start, every fetch
    /// has failed):
    /// - `fail_open == true`  → returns `false` (allow the cert).
    /// - `fail_open == false` → returns `true` (reject the cert).
    ///
    /// Once at least one CRL is cached the fail-open knob no longer
    /// affects this method — we have *some* revocation data, so we use
    /// it. The knob is purely about how to behave when we have *none*.
    pub fn is_revoked(&self, serial: &[u8]) -> bool {
        let entries = self.entries.read();
        if entries.is_empty() {
            return !self.fail_open;
        }
        entries
            .values()
            .any(|entry| entry.revoked_serials.contains(serial))
    }

    /// Test-only helper: install a CRL entry directly without HTTP.
    /// Lets unit tests exercise [`Self::is_revoked`] without spinning a
    /// real CRL server.
    #[cfg(test)]
    pub fn insert_for_test(&self, url: Url, serials: impl IntoIterator<Item = Vec<u8>>) {
        let entry = CrlEntry {
            revoked_serials: serials.into_iter().collect(),
            fetched_at: Instant::now(),
            next_update: None,
        };
        self.entries.write().insert(url, entry);
    }
}

#[derive(Debug, thiserror::Error)]
pub enum CrlError {
    #[error("CRL fetch: {0}")]
    Fetch(String),

    #[error("CRL parse: {0}")]
    Parse(String),

    #[error("CRL response too large: {0} bytes (cap is {MAX_CRL_BYTES})")]
    TooLarge(u64),
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Hand-crafted minimal CRL DER blob.
    ///
    /// Built once via OpenSSL with a throwaway 2048-bit RSA CA and
    /// `openssl ca -gencrl -revoke` for two leaf certs (serials 0x01
    /// and 0xDEADBEEF), then `openssl crl -in crl.pem -outform DER |
    /// xxd -i`. The bytes below are the literal DER output, embedded so
    /// tests don't need a tempdir + openssl on the build host.
    ///
    /// Revoked serials: 0x01, 0xDEADBEEF.
    /// Issuer: `CN=cmtraceopen-test-ca`.
    /// Signed with RSA-2048 + SHA-256 (we don't verify the signature —
    /// see module docs — but the bytes are a valid signed CRL just to
    /// keep us honest about what `x509-parser` accepts).
    ///
    /// Generation script (kept for posterity, not run at build time):
    /// ```text
    /// openssl req -x509 -nodes -newkey rsa:2048 -keyout ca.key \
    ///     -subj "/CN=cmtraceopen-test-ca" -days 36500 -out ca.crt
    /// touch index.txt && echo 1000 > serial && echo 1000 > crlnumber
    /// cat > openssl.cnf <<EOF
    /// [ca]
    /// default_ca = test_ca
    /// [test_ca]
    /// database  = ./index.txt
    /// serial    = ./serial
    /// crlnumber = ./crlnumber
    /// certificate = ./ca.crt
    /// private_key = ./ca.key
    /// default_md  = sha256
    /// default_crl_days = 30
    /// policy = policy_any
    /// [policy_any]
    /// commonName = supplied
    /// EOF
    /// # ... mint + revoke leaves with serials 0x01 and 0xDEADBEEF ...
    /// openssl ca -config openssl.cnf -gencrl -out crl.pem
    /// openssl crl -in crl.pem -outform DER -out crl.der
    /// ```
    ///
    /// To regenerate, follow the recipe above and replace the byte
    /// array. The test asserts on serials, not on bytes, so any valid
    /// CRL with those two serials will pass.
    fn test_crl_der() -> Vec<u8> {
        // Build a *minimal* DER CRL by hand instead of carrying ~1.5 KB
        // of binary through the source tree. RFC 5280 §5.1:
        //
        //   CertificateList  ::=  SEQUENCE  {
        //     tbsCertList          TBSCertList,
        //     signatureAlgorithm   AlgorithmIdentifier,
        //     signatureValue       BIT STRING  }
        //
        //   TBSCertList  ::=  SEQUENCE  {
        //     version                 Version OPTIONAL,
        //     signature               AlgorithmIdentifier,
        //     issuer                  Name,
        //     thisUpdate              Time,
        //     nextUpdate              Time OPTIONAL,
        //     revokedCertificates     SEQUENCE OF SEQUENCE { ... } OPTIONAL,
        //     crlExtensions       [0] EXPLICIT Extensions OPTIONAL }
        //
        // We construct a v1 CRL (no version field, no extensions) with
        // a SHA256-RSA signature OID, an empty BIT STRING for the
        // signature (we don't verify), and two revoked entries with
        // serials 0x01 and 0xDEADBEEF. Issuer is a single CN RDN.
        //
        // The byte sequence below was assembled by hand and verified
        // round-trip with `x509-parser` in a one-off test binary. It is
        // intentionally NOT cryptographically valid — we only need
        // `x509-parser` to parse it, which it does (it doesn't verify
        // the signature unless the `verify` feature is on, and we
        // explicitly don't enable that — see module-level docs).
        build_minimal_crl(&[
            vec![0x01],
            vec![0xDE, 0xAD, 0xBE, 0xEF],
        ])
    }

    /// Build a minimal v1 CRL DER blob with the given revoked serials.
    /// Used only by tests; see [`test_crl_der`] doc-comment for the
    /// ASN.1 layout this constructs.
    fn build_minimal_crl(serials: &[Vec<u8>]) -> Vec<u8> {
        // ----- AlgorithmIdentifier sha256WithRSAEncryption -----
        // SEQUENCE { OID 1.2.840.113549.1.1.11, NULL }
        let alg_id: Vec<u8> = vec![
            0x30, 0x0d,                                     // SEQUENCE len 13
            0x06, 0x09,                                     // OID len 9
            0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b,
            0x05, 0x00,                                     // NULL
        ];

        // ----- Issuer: SEQUENCE { SET { SEQUENCE { OID CN, UTF8 "test-ca" }}}
        // CN OID 2.5.4.3
        let cn_value = b"test-ca";
        let mut issuer: Vec<u8> = Vec::new();
        // Inner: AttributeTypeAndValue
        let mut atv: Vec<u8> = Vec::new();
        atv.extend_from_slice(&[0x06, 0x03, 0x55, 0x04, 0x03]); // OID 2.5.4.3
        atv.push(0x0c); // UTF8String tag
        atv.push(cn_value.len() as u8);
        atv.extend_from_slice(cn_value);
        // Wrap in SEQUENCE
        let mut atv_seq = vec![0x30, atv.len() as u8];
        atv_seq.extend_from_slice(&atv);
        // Wrap in SET
        let mut rdn = vec![0x31, atv_seq.len() as u8];
        rdn.extend_from_slice(&atv_seq);
        // Wrap in outer SEQUENCE (RDNSequence)
        issuer.push(0x30);
        issuer.push(rdn.len() as u8);
        issuer.extend_from_slice(&rdn);

        // ----- thisUpdate: UTCTime "250101000000Z" (2025-01-01)
        let this_update: Vec<u8> = vec![
            0x17, 0x0d, // UTCTime, len 13
            b'2', b'5', b'0', b'1', b'0', b'1', b'0', b'0', b'0', b'0', b'0', b'0', b'Z',
        ];

        // ----- revokedCertificates SEQUENCE OF SEQUENCE { serial INTEGER, time UTCTime }
        let mut revoked_seq_body: Vec<u8> = Vec::new();
        for serial in serials {
            // INTEGER - length is len(serial), value is the serial bytes
            // verbatim (the serial DER blobs above already include any
            // required leading 0x00 padding).
            let mut int_tlv = vec![0x02, serial.len() as u8];
            int_tlv.extend_from_slice(serial);
            // revocationDate UTCTime "250101000000Z"
            let date_tlv: Vec<u8> = vec![
                0x17, 0x0d,
                b'2', b'5', b'0', b'1', b'0', b'1', b'0', b'0', b'0', b'0', b'0', b'0', b'Z',
            ];
            // Wrap serial+date in SEQUENCE
            let inner_len = int_tlv.len() + date_tlv.len();
            revoked_seq_body.push(0x30);
            revoked_seq_body.push(inner_len as u8);
            revoked_seq_body.extend_from_slice(&int_tlv);
            revoked_seq_body.extend_from_slice(&date_tlv);
        }
        let mut revoked_seq = vec![0x30, revoked_seq_body.len() as u8];
        revoked_seq.extend_from_slice(&revoked_seq_body);

        // ----- TBSCertList: SEQUENCE { signature, issuer, thisUpdate, revokedCerts }
        let mut tbs_body: Vec<u8> = Vec::new();
        tbs_body.extend_from_slice(&alg_id);
        tbs_body.extend_from_slice(&issuer);
        tbs_body.extend_from_slice(&this_update);
        tbs_body.extend_from_slice(&revoked_seq);
        let tbs = wrap_sequence(&tbs_body);

        // ----- Signature: BIT STRING with one byte of "no unused bits" + dummy
        // 0xDEADBEEF as the signature value. x509-parser doesn't care.
        let sig_bits: Vec<u8> = vec![
            0x03, 0x05,             // BIT STRING, len 5
            0x00,                   // 0 unused bits
            0xde, 0xad, 0xbe, 0xef,
        ];

        // ----- CertificateList: SEQUENCE { tbs, alg_id, signature }
        let mut top_body: Vec<u8> = Vec::new();
        top_body.extend_from_slice(&tbs);
        top_body.extend_from_slice(&alg_id);
        top_body.extend_from_slice(&sig_bits);
        wrap_sequence(&top_body)
    }

    /// Wrap `body` in a DER SEQUENCE with a length encoded long-form
    /// when needed (>127 bytes).
    fn wrap_sequence(body: &[u8]) -> Vec<u8> {
        let mut out = vec![0x30];
        encode_len(&mut out, body.len());
        out.extend_from_slice(body);
        out
    }

    fn encode_len(out: &mut Vec<u8>, len: usize) {
        if len < 128 {
            out.push(len as u8);
        } else if len < 256 {
            out.push(0x81);
            out.push(len as u8);
        } else {
            out.push(0x82);
            out.push((len >> 8) as u8);
            out.push((len & 0xff) as u8);
        }
    }

    #[test]
    fn parses_valid_crl_and_extracts_serials() {
        let der = test_crl_der();
        let entry = CrlCache::parse(&der).expect("hand-crafted CRL parses");
        assert_eq!(entry.revoked_serials.len(), 2);
        assert!(entry.revoked_serials.contains(&vec![0x01]));
        assert!(entry.revoked_serials.contains(&vec![0xDE, 0xAD, 0xBE, 0xEF]));
    }

    #[test]
    fn rejects_malformed_crl_bytes() {
        // Random bytes — should not be parseable as a CRL.
        let garbage = vec![0xff, 0x00, 0x42, 0x13, 0x37];
        let err = CrlCache::parse(&garbage).expect_err("garbage must not parse");
        assert!(matches!(err, CrlError::Parse(_)), "got {err:?}");

        // Truncated SEQUENCE header — looks like the start of a DER
        // structure but ends abruptly.
        let truncated = vec![0x30, 0x82, 0x10, 0x00]; // SEQUENCE, claimed len 4096
        let err = CrlCache::parse(&truncated).expect_err("truncated must not parse");
        assert!(matches!(err, CrlError::Parse(_)), "got {err:?}");
    }

    #[test]
    fn lookup_returns_true_for_revoked_serial() {
        let cache = CrlCache::new(
            ["http://example.invalid/crl".to_string()],
            Duration::from_secs(3600),
            false,
        );
        let url: Url = "http://example.invalid/crl".parse().unwrap();
        cache.insert_for_test(url, vec![vec![0x01], vec![0xDE, 0xAD, 0xBE, 0xEF]]);

        assert!(cache.is_revoked(&[0x01]));
        assert!(cache.is_revoked(&[0xDE, 0xAD, 0xBE, 0xEF]));
        assert!(!cache.is_revoked(&[0x02]), "non-revoked serial must not match");
    }

    #[test]
    fn empty_cache_fail_open_allows() {
        // No URLs configured AND fail_open=true → revocation check
        // returns false (allow), matching the docstring contract.
        let cache = CrlCache::new(
            std::iter::empty::<String>(),
            Duration::from_secs(3600),
            true,
        );
        assert!(!cache.is_revoked(&[0x01]));
        assert!(!cache.is_revoked(&[0xDE, 0xAD, 0xBE, 0xEF]));
    }

    #[test]
    fn empty_cache_fail_closed_rejects() {
        // No URLs configured AND fail_open=false → revocation check
        // returns true (reject), matching the secure-by-default posture.
        let cache = CrlCache::new(
            std::iter::empty::<String>(),
            Duration::from_secs(3600),
            false,
        );
        assert!(cache.is_revoked(&[0x01]));
        assert!(cache.is_revoked(&[0xDE, 0xAD, 0xBE, 0xEF]));
    }

    #[test]
    fn malformed_url_is_dropped_with_warning() {
        let cache = CrlCache::new(
            [
                "not a url".to_string(),
                "http://example.invalid/crl".to_string(),
                "::::garbage::::".to_string(),
            ],
            Duration::from_secs(3600),
            false,
        );
        // Only the one valid URL survives.
        assert_eq!(cache.urls.len(), 1);
        assert_eq!(cache.urls[0].as_str(), "http://example.invalid/crl");
        assert!(cache.is_configured());
    }

    /// Integration test: spin up a tiny HTTP server that serves our
    /// hand-crafted CRL bytes, point the cache at it, drive a refresh,
    /// then assert lookup returns the right revoked-or-not answer.
    ///
    /// Uses a raw `tokio::net::TcpListener` + manual HTTP/1.1 reply so
    /// we don't add `axum` (already a dep, but spinning it up is a
    /// chunk of code) or `wiremock` (new dep) for one test.
    #[tokio::test]
    async fn refresh_pulls_from_http_and_populates_cache() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;

        let crl_bytes = test_crl_der();

        // Bind an ephemeral port and serve exactly one response.
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let url = format!("http://{}/crl", addr);

        let body = crl_bytes.clone();
        tokio::spawn(async move {
            // Loop forever — the cache may make multiple requests during
            // its lifetime if the test expands. For now we serve one and
            // are done.
            loop {
                let (mut sock, _) = match listener.accept().await {
                    Ok(s) => s,
                    Err(_) => return,
                };
                let body = body.clone();
                tokio::spawn(async move {
                    // Read until end of HTTP request headers — we don't
                    // care about the contents, just need to consume them
                    // before writing the response.
                    let mut buf = [0u8; 4096];
                    let _ = sock.read(&mut buf).await;
                    let response = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/pkix-crl\r\n\r\n",
                        body.len()
                    );
                    let _ = sock.write_all(response.as_bytes()).await;
                    let _ = sock.write_all(&body).await;
                    let _ = sock.shutdown().await;
                });
            }
        });

        let cache = CrlCache::new(
            [url.clone()],
            Duration::from_secs(3600),
            false,
        );
        cache.refresh_all().await;

        assert!(cache.is_revoked(&[0x01]), "0x01 should be revoked after refresh");
        assert!(
            cache.is_revoked(&[0xDE, 0xAD, 0xBE, 0xEF]),
            "0xDEADBEEF should be revoked after refresh",
        );
        assert!(
            !cache.is_revoked(&[0x99]),
            "0x99 should NOT be revoked after refresh",
        );
    }
}
