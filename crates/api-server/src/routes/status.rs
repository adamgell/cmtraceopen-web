//! `GET /` — a minimal HTML status page for local-dev / on-device debugging.
//!
//! This is **not** production-safe as-is — it surfaces uptime + request-count
//! metrics without any authentication. In a real deployment this route is
//! expected to be firewalled off or gated behind an auth layer. For now it's
//! strictly a developer convenience to confirm the service is alive + see
//! basic counters at a glance.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    extract::State,
    http::{header, HeaderValue, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};

/// Shared process-wide state surfaced on the status page.
///
/// Cheap to `Arc::clone` into both the Axum router state and the request-
/// counter middleware.
#[derive(Debug)]
pub struct AppState {
    /// Monotonic start time; used for uptime math.
    pub started_at: Instant,
    /// Total HTTP requests served since process start, all routes + methods.
    /// Incremented once per request by `request_counter_middleware`.
    pub request_count: AtomicU64,
    /// Listen address copied from Config at startup — cheap to stringify.
    pub listen_addr: String,
    /// Hostname reported by the kernel at startup (best-effort; falls back to
    /// `"unknown"` if the OS lookup fails).
    pub hostname: String,
}

impl AppState {
    pub fn new(listen_addr: String) -> Arc<Self> {
        Arc::new(Self {
            started_at: Instant::now(),
            request_count: AtomicU64::new(0),
            listen_addr,
            hostname: detect_hostname(),
        })
    }
}

/// Best-effort hostname lookup. Uses `HOSTNAME` / `COMPUTERNAME` env vars to
/// avoid pulling in a platform-specific crate for a debug-only field.
fn detect_hostname() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

/// Axum middleware that bumps the global request counter on every request.
///
/// Uses `Relaxed` ordering because the counter is a display-only metric — no
/// other code branches on its value, so stricter ordering would be overkill.
pub async fn request_counter_middleware(
    State(state): State<Arc<AppState>>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    state.request_count.fetch_add(1, Ordering::Relaxed);
    next.run(req).await
}

/// Render the status page. Returns HTML with an explicit UTF-8 content type.
async fn status_page(State(state): State<Arc<AppState>>) -> Response {
    let uptime = state.started_at.elapsed();
    let count = state.request_count.load(Ordering::Relaxed);

    let body = render_html(&state, uptime, count);

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

fn render_html(state: &AppState, uptime: Duration, count: u64) -> String {
    let service = env!("CARGO_PKG_NAME");
    let version = env!("CARGO_PKG_VERSION");
    // Rust version captured at build time (see build.rs).
    let rustc = env!("RUSTC_VERSION");
    let uptime_h = humanize(uptime);

    // TODO: DB pool stats surface once PR #7 lands (sqlx on main).

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
    <h2>Endpoints</h2>
    <ul class="links">
      <li><a href="/healthz">/healthz</a></li>
      <li><a href="/readyz">/readyz</a></li>
      <li><a href="/v1/devices">/v1/devices</a> <span style="color:var(--muted)">(404 until PR #7 lands)</span></li>
    </ul>
  </section>

  <section class="card">
    <h2>Companion tools</h2>
    <ul class="links">
      <li><a href="http://{host_only}:8081">Adminer (Postgres UI)</a></li>
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
        count = count,
        listen = state.listen_addr,
        host = state.hostname,
        host_only = hostname_for_link(&state.hostname),
        rustc = rustc,
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
    use super::*;

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

    #[tokio::test]
    async fn render_html_contains_expected_fields() {
        let state = AppState::new("0.0.0.0:8080".to_string());
        let html = render_html(&state, Duration::from_secs(65), 42);
        assert!(html.contains("<!doctype html>"));
        assert!(html.contains("api-server"));
        assert!(html.contains("1m 5s"));
        assert!(html.contains(">42<"));
        assert!(html.contains("0.0.0.0:8080"));
        assert!(html.contains("/healthz"));
        assert!(html.contains(":8081"));
    }
}
