//! `GET /` — a minimal HTML status page for local-dev / on-device debugging.
//!
//! This is **not** production-safe as-is — it surfaces uptime + request-count
//! metrics without any authentication. In a real deployment this route is
//! expected to be firewalled off or gated behind an auth layer. For now it's
//! strictly a developer convenience to confirm the service is alive + see
//! basic counters at a glance.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{MatchedPath, State},
    http::{header, HeaderValue, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};

use crate::state::{AppState, UNMATCHED_ROUTE_KEY};
use crate::storage::{PoolStats, SessionRow};

/// Axum middleware that bumps the per-route request counter on every request.
///
/// Pulls the `MatchedPath` extension out of the request (set by Axum's
/// router when a route matched) and increments that route's bucket. Requests
/// that didn't match any route — true 404s — go into the `unmatched` bucket
/// so the status page can surface probe traffic without inflating real
/// route counts.
///
/// Uses `Relaxed` ordering because the counter is a display-only metric — no
/// other code branches on its value, so stricter ordering would be overkill.
pub async fn request_counter_middleware(
    State(state): State<Arc<AppState>>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let key = req
        .extensions()
        .get::<MatchedPath>()
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| UNMATCHED_ROUTE_KEY.to_string());

    // entry().or_insert_with(...) keeps the insert fast-path lock-free per
    // shard. The returned guard derefs to the AtomicU64 we want to bump.
    state
        .request_counts
        .entry(key)
        .or_insert_with(|| AtomicU64::new(0))
        .fetch_add(1, Ordering::Relaxed);

    next.run(req).await
}

/// Render the status page. Returns HTML with an explicit UTF-8 content type.
async fn status_page(State(state): State<Arc<AppState>>) -> Response {
    let uptime = state.started_at.elapsed();
    // Snapshot the per-route counter map into a sorted (route, count) vec
    // so the renderer doesn't iterate the live DashMap (and hold shard
    // guards) while building the response string.
    let mut routes: Vec<(String, u64)> = state
        .request_counts
        .iter()
        .map(|kv| (kv.key().clone(), kv.value().load(Ordering::Relaxed)))
        .collect();
    // Sort by count DESC, then route name ASC as a stable tiebreaker so two
    // routes with the same count render in a consistent order across reloads.
    routes.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let total: u64 = routes.iter().map(|(_, c)| *c).sum();

    // Snapshot the metadata pool via the trait method so this handler stays
    // backend-agnostic (see `MetadataStore::pool_stats`).
    let pool = state.meta.pool_stats();

    // Best-effort fetch of recent sessions. A storage failure here shouldn't
    // 500 the dev status page — degrade to an empty list and keep rendering.
    let recent = state
        .meta
        .recent_sessions(10)
        .await
        .unwrap_or_default();

    let body = render_html(&state, uptime, total, &routes, pool, &recent);

    let mut resp = (StatusCode::OK, body).into_response();
    resp.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    resp
}

/// Humanize a `Duration` to a compact "3h 12m" / "45s" style string.
///
/// Kept in-crate because `humantime::format_duration` emits full precision
/// ("3h 12m 5s 123ms 456us") which is too noisy for a dashboard; this impl
/// keeps only the two most-significant units and avoids pulling a dep for
/// a ~15-line helper.
fn humanize(uptime: Duration) -> String {
    let total = uptime.as_secs();
    if total == 0 {
        return "0s".to_string();
    }
    let days = total / 86_400;
    let hours = (total % 86_400) / 3_600;
    let mins = (total % 3_600) / 60;
    let secs = total % 60;

    let mut parts: Vec<String> = Vec::new();
    if days > 0 {
        parts.push(format!("{days}d"));
    }
    if hours > 0 {
        parts.push(format!("{hours}h"));
    }
    if mins > 0 {
        parts.push(format!("{mins}m"));
    }
    if secs > 0 && parts.len() < 2 {
        parts.push(format!("{secs}s"));
    }
    // Keep only the two most-significant units for a compact display.
    parts.truncate(2);
    parts.join(" ")
}

/// Minimal HTML escaper for the handful of fields we render verbatim
/// (route names, device ids, parse_state). Avoids pulling a full htmlescape
/// crate for ~5 substitutions; covers the characters that would otherwise
/// break out of an attribute or element body.
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Render the per-route counter section. Picks the top 8 by count; falls
/// back to a friendly placeholder when the map is still empty (e.g. very
/// first request to `/`).
fn render_routes(routes: &[(String, u64)]) -> String {
    if routes.is_empty() {
        return "<p class=\"muted\">No requests served yet.</p>".to_string();
    }
    let mut out = String::from("<dl class=\"routes\">");
    for (route, count) in routes.iter().take(8) {
        out.push_str(&format!(
            "<dt>{route}</dt><dd>{count}</dd>",
            route = esc(route),
            count = count,
        ));
    }
    out.push_str("</dl>");
    out
}

/// Render the recent-bundles table. Empty state surfaces a friendly note so
/// fresh deployments don't show a blank panel.
fn render_recent(recent: &[SessionRow]) -> String {
    if recent.is_empty() {
        return "<p class=\"muted\">No bundles ingested yet.</p>".to_string();
    }
    let mut out = String::from(
        "<table class=\"recent\">\
         <thead><tr>\
         <th>device_id</th>\
         <th>session_id</th>\
         <th>parse_state</th>\
         <th>ingestedUtc</th>\
         </tr></thead><tbody>",
    );
    for s in recent {
        // Short session id: first 8 hex chars + ellipsis. UUIDs render as
        // 36-char strings; trimming keeps the table from sprawling.
        let sid = s.session_id.to_string();
        let short = if sid.len() > 8 { &sid[..8] } else { sid.as_str() };
        out.push_str(&format!(
            "<tr><td>{dev}</td><td>{short}…</td><td>{state}</td><td>{ts}</td></tr>",
            dev = esc(&s.device_id),
            short = esc(short),
            state = esc(&s.parse_state),
            // RFC 3339 always renders as ASCII so esc is overkill, but keep
            // it for symmetry with the other cells in case the formatter
            // ever changes.
            ts = esc(&s.ingested_utc.to_rfc3339()),
        ));
    }
    out.push_str("</tbody></table>");
    out
}

fn render_html(
    state: &AppState,
    uptime: Duration,
    total_requests: u64,
    routes: &[(String, u64)],
    pool: PoolStats,
    recent: &[SessionRow],
) -> String {
    let service = env!("CARGO_PKG_NAME");
    let version = env!("CARGO_PKG_VERSION");
    // Rust version captured at build time (see build.rs).
    let rustc = env!("RUSTC_VERSION");
    let uptime_h = humanize(uptime);
    let routes_html = render_routes(routes);
    let recent_html = render_recent(recent);

    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>{service} status</title>
<meta name="viewport" content="width=device-width, initial-scale=1">
<style>
  :root {{
    color-scheme: light dark;
    --bg: #fafafa;
    --fg: #1a1a1a;
    --muted: #6b7280;
    --card: #ffffff;
    --border: #e5e7eb;
    --accent: #2563eb;
  }}
  @media (prefers-color-scheme: dark) {{
    :root {{
      --bg: #0f1115;
      --fg: #e5e7eb;
      --muted: #9ca3af;
      --card: #171a21;
      --border: #2a2f3a;
      --accent: #60a5fa;
    }}
  }}
  body {{
    margin: 0;
    padding: 2rem 1rem;
    font: 14px/1.5 ui-sans-serif, system-ui, -apple-system, Segoe UI, Roboto, sans-serif;
    background: var(--bg);
    color: var(--fg);
  }}
  main {{ max-width: 760px; margin: 0 auto; }}
  h1 {{ font-size: 1.4rem; margin: 0 0 0.25rem; }}
  h1 .version {{ font-weight: 400; color: var(--muted); font-size: 0.9rem; margin-left: 0.5rem; }}
  p.subtitle {{ margin: 0 0 1.5rem; color: var(--muted); }}
  p.muted {{ color: var(--muted); margin: 0; font-style: italic; }}
  section.card {{
    background: var(--card);
    border: 1px solid var(--border);
    border-radius: 8px;
    padding: 1rem 1.25rem;
    margin-bottom: 1rem;
  }}
  section.card h2 {{
    font-size: 0.85rem;
    text-transform: uppercase;
    letter-spacing: 0.05em;
    color: var(--muted);
    margin: 0 0 0.75rem;
  }}
  dl {{ display: grid; grid-template-columns: max-content 1fr; gap: 0.35rem 1rem; margin: 0; }}
  dt {{ color: var(--muted); }}
  dd {{ margin: 0; font-family: ui-monospace, SFMono-Regular, Menlo, monospace; }}
  dl.routes {{ grid-template-columns: 1fr max-content; gap: 0.25rem 1rem; }}
  dl.routes dt {{
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    color: var(--fg);
    overflow-wrap: anywhere;
  }}
  dl.routes dd {{ text-align: right; color: var(--muted); }}
  table.recent {{
    width: 100%;
    border-collapse: collapse;
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.85rem;
  }}
  table.recent th, table.recent td {{
    text-align: left;
    padding: 0.35rem 0.5rem;
    border-bottom: 1px solid var(--border);
    white-space: nowrap;
  }}
  table.recent th {{
    color: var(--muted);
    font-weight: 500;
    font-size: 0.75rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }}
  table.recent tr:last-child td {{ border-bottom: none; }}
  ul.links {{ list-style: none; padding: 0; margin: 0; display: flex; flex-wrap: wrap; gap: 0.5rem 1rem; }}
  ul.links a {{ color: var(--accent); text-decoration: none; }}
  ul.links a:hover {{ text-decoration: underline; }}
  footer {{ color: var(--muted); font-size: 0.8rem; margin-top: 1.5rem; }}
</style>
</head>
<body>
<main>
  <h1>{service}<span class="version">v{version}</span></h1>
  <p class="subtitle">Dev-debugging status page. Not production-safe; firewall off in real deployments.</p>

  <section class="card">
    <h2>Process</h2>
    <dl>
      <dt>Uptime</dt><dd>{uptime_h}</dd>
      <dt>Requests served</dt><dd>{count}</dd>
      <dt>Listen addr</dt><dd>{listen}</dd>
      <dt>Hostname</dt><dd>{host}</dd>
      <dt>Built with</dt><dd>{rustc}</dd>
    </dl>
  </section>

  <section class="card">
    <h2>Storage</h2>
    <dl>
      <dt>SQLite pool</dt>
      <dd>size: {pool_size} / idle: {pool_idle} / max_size: {pool_max}</dd>
    </dl>
  </section>

  <section class="card">
    <h2>Recent bundles</h2>
    {recent_html}
  </section>

  <section class="card">
    <h2>Top routes</h2>
    {routes_html}
  </section>

  <section class="card">
    <h2>Endpoints</h2>
    <ul class="links">
      <li><a href="/healthz">/healthz</a></li>
      <li><a href="/readyz">/readyz</a></li>
      <li><a href="/v1/devices">/v1/devices</a></li>
    </ul>
  </section>

  <section class="card">
    <h2>Companion tools</h2>
    <ul class="links">
      <li><a href="http://{host_only}:8082">Adminer (Postgres UI)</a></li>
    </ul>
  </section>

  <footer>cmtraceopen-api &middot; v{version}</footer>
</main>
</body>
</html>
"#,
        service = service,
        version = version,
        uptime_h = uptime_h,
        count = total_requests,
        listen = state.listen_addr,
        host = state.hostname,
        host_only = hostname_for_link(&state.hostname),
        rustc = rustc,
        pool_size = pool.size,
        pool_idle = pool.idle,
        pool_max = pool.max_size,
        recent_html = recent_html,
        routes_html = routes_html,
    )
}

/// Pick a usable hostname for the Adminer link. Containers often report
/// opaque IDs like `a1b2c3d4e5` that won't resolve from the browser; fall
/// back to `localhost` when the hostname looks container-ish or is unknown.
fn hostname_for_link(hostname: &str) -> &str {
    if hostname == "unknown" || hostname.len() == 12 && hostname.chars().all(|c| c.is_ascii_hexdigit()) {
        "localhost"
    } else {
        hostname
    }
}

pub fn router(state: Arc<AppState>) -> Router {
    Router::new().route("/", get(status_page)).with_state(state)
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use chrono::Utc;
    use dashmap::DashMap;

    use super::*;

    /// Build a bare-bones `AppState` for rendering tests. `render_html` only
    /// reads the process-level display fields (`started_at`, `request_counts`,
    /// `listen_addr`, `hostname`), so we stub the two storage trait objects
    /// with live in-memory + tempdir-backed implementations to satisfy the
    /// type without any mocking scaffolding.
    async fn fake_state(listen: &str) -> AppState {
        use crate::storage::{LocalFsBlobStore, SqliteMetadataStore};

        // Tempdir leaks for the duration of the process — unit tests are
        // short-lived and we never exercise the blob store from these tests.
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let blobs = Arc::new(
            LocalFsBlobStore::new(tmp.path())
                .await
                .expect("blob store"),
        );
        // Intentionally leak the TempDir so the path outlives this helper —
        // we never actually touch the blob store, but the blob store holds
        // paths referencing this dir.
        std::mem::forget(tmp);
        let meta = Arc::new(
            SqliteMetadataStore::connect(":memory:")
                .await
                .expect("sqlite"),
        );
        use crate::auth::{AuthMode, AuthState, JwksCache};
        AppState {
            meta,
            blobs,
            started_at: Instant::now(),
            request_counts: Arc::new(DashMap::new()),
            listen_addr: listen.to_string(),
            hostname: "unknown".to_string(),
            auth: AuthState {
                mode: AuthMode::Disabled,
                entra: None,
                jwks: Arc::new(JwksCache::new("http://127.0.0.1:1/unused".to_string())),
            },
            cors: crate::state::CorsConfig::default(),
            mtls: crate::state::MtlsRuntimeConfig::default(),
        }
    }

    #[test]
    fn humanize_zero() {
        assert_eq!(humanize(Duration::from_secs(0)), "0s");
    }

    #[test]
    fn humanize_seconds_only() {
        assert_eq!(humanize(Duration::from_secs(45)), "45s");
    }

    #[test]
    fn humanize_minutes_and_seconds() {
        assert_eq!(humanize(Duration::from_secs(125)), "2m 5s");
    }

    #[test]
    fn humanize_hours_and_minutes_truncates() {
        // 3h 12m 5s -> keep the two most-significant units.
        assert_eq!(humanize(Duration::from_secs(3 * 3600 + 12 * 60 + 5)), "3h 12m");
    }

    #[test]
    fn humanize_days_and_hours() {
        assert_eq!(humanize(Duration::from_secs(2 * 86400 + 3 * 3600)), "2d 3h");
    }

    #[test]
    fn hostname_for_link_maps_container_id_to_localhost() {
        assert_eq!(hostname_for_link("a1b2c3d4e5f6"), "localhost");
        assert_eq!(hostname_for_link("unknown"), "localhost");
        assert_eq!(hostname_for_link("bigmac"), "bigmac");
    }

    #[test]
    fn esc_handles_html_metacharacters() {
        assert_eq!(esc("a & b < c > d \" e ' f"), "a &amp; b &lt; c &gt; d &quot; e &#39; f");
        assert_eq!(esc("plain"), "plain");
    }

    #[test]
    fn render_routes_top_eight_sorted_desc() {
        // Build a 10-route input; render_routes should keep only the top 8 by
        // count and order them DESC. Ties (count=5) break alphabetically.
        let routes: Vec<(String, u64)> = vec![
            ("/a".into(), 100),
            ("/b".into(), 50),
            ("/c".into(), 25),
            ("/d".into(), 10),
            ("/e".into(), 9),
            ("/f".into(), 5),
            ("/g".into(), 5),
            ("/h".into(), 4),
            ("/i".into(), 3),
            ("/j".into(), 2),
        ];
        let html = render_routes(&routes);
        // Top-8 boundary: /h (count 4) is in, /i (count 3) is out.
        assert!(html.contains("<dt>/a</dt><dd>100</dd>"));
        assert!(html.contains("<dt>/h</dt><dd>4</dd>"));
        assert!(!html.contains("/i"));
        assert!(!html.contains("/j"));
        // Ordering: /a appears before /b appears before /c.
        let pos_a = html.find("/a").unwrap();
        let pos_b = html.find("/b").unwrap();
        assert!(pos_a < pos_b);
    }

    #[test]
    fn render_routes_empty_state() {
        assert!(render_routes(&[]).contains("No requests served yet"));
    }

    #[test]
    fn render_recent_empty_state() {
        assert!(render_recent(&[]).contains("No bundles ingested yet"));
    }

    #[test]
    fn render_recent_renders_table() {
        let row = SessionRow {
            session_id: uuid::Uuid::parse_str("019db170-1111-7000-8000-000000000001").unwrap(),
            device_id: "WIN-X".into(),
            bundle_id: uuid::Uuid::nil(),
            blob_uri: "file:///tmp/x".into(),
            content_kind: "evidence-zip".into(),
            size_bytes: 0,
            sha256: "0".repeat(64),
            collected_utc: None,
            ingested_utc: Utc::now(),
            parse_state: "ok".into(),
        };
        let html = render_recent(&[row]);
        assert!(html.contains("<table"));
        assert!(html.contains("WIN-X"));
        // Short session id is the first 8 chars + ellipsis.
        assert!(html.contains("019db170…"));
        assert!(html.contains("ok"));
    }

    #[tokio::test]
    async fn render_html_contains_expected_fields() {
        let state = fake_state("0.0.0.0:8080").await;
        let pool = PoolStats { size: 3, idle: 2, max_size: 8 };
        let routes = vec![("/healthz".to_string(), 42)];
        let html = render_html(&state, Duration::from_secs(65), 42, &routes, pool, &[]);
        assert!(html.contains("<!doctype html>"));
        assert!(html.contains("api-server"));
        assert!(html.contains("1m 5s"));
        assert!(html.contains(">42<"));
        assert!(html.contains("0.0.0.0:8080"));
        assert!(html.contains("/healthz"));
        assert!(html.contains(":8082"));
        // Storage section populated with the supplied PoolStats.
        assert!(html.contains("Storage"));
        assert!(html.contains("SQLite pool"));
        assert!(html.contains("size: 3 / idle: 2 / max_size: 8"));
        // Recent bundles section renders the empty-state copy.
        assert!(html.contains("Recent bundles"));
        assert!(html.contains("No bundles ingested yet"));
        // Top routes section shows the supplied route + count.
        assert!(html.contains("Top routes"));
        assert!(html.contains("<dt>/healthz</dt><dd>42</dd>"));
    }

    #[tokio::test]
    async fn middleware_increments_per_route_buckets() {
        // Driver: build a tiny router with two routes + the counter
        // middleware, fire requests at each, then assert the per-route
        // buckets have the right counts. Uses tower::ServiceExt::oneshot
        // to drive the router in-process.
        use axum::body::Body;
        use axum::http::Request;
        use tower::ServiceExt;

        let state = Arc::new(fake_state("test").await);
        let app = Router::new()
            .route("/a", get(|| async { "a" }))
            .route("/b/{id}", get(|| async { "b" }))
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                request_counter_middleware,
            ))
            .with_state(());

        // Hit /a twice and /b/{id} three times (with two different ids, so
        // we'd see two buckets if the middleware keyed on raw path — proving
        // it keys on MatchedPath instead).
        for path in ["/a", "/a", "/b/1", "/b/2", "/b/3"] {
            let req = Request::builder().uri(path).body(Body::empty()).unwrap();
            let resp = app.clone().oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
        }

        let a = state
            .request_counts
            .get("/a")
            .expect("/a bucket")
            .load(Ordering::Relaxed);
        let b = state
            .request_counts
            .get("/b/{id}")
            .expect("/b/{{id}} bucket (route template, not raw path)")
            .load(Ordering::Relaxed);
        assert_eq!(a, 2);
        assert_eq!(b, 3);

        // Unmatched bucket: hitting an unknown path should land in `unmatched`.
        let req = Request::builder().uri("/no-such-route").body(Body::empty()).unwrap();
        let _ = app.clone().oneshot(req).await.unwrap();
        let unmatched = state
            .request_counts
            .get(UNMATCHED_ROUTE_KEY)
            .expect("unmatched bucket")
            .load(Ordering::Relaxed);
        assert_eq!(unmatched, 1);
    }
}
