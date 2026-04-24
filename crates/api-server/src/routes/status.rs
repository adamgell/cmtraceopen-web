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
    extract::{MatchedPath, State},
    http::{header, HeaderValue, Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    routing::get,
    Router,
};

use crate::auth::AuthMode;
use crate::state::{AppState, UNMATCHED_ROUTE_KEY};
use crate::storage::{DeviceRow, MetadataStore, PoolStats, SessionRow};

/// Axum middleware that bumps both the per-route request counter (used by the
/// dev status page) and the Prometheus per-route counter / latency histogram
/// on every request.
///
/// Pulls the `MatchedPath` extension out of the request (set by Axum's
/// router when a route matched) and increments that route's bucket. Requests
/// that didn't match any route — true 404s — go into the `unmatched` bucket
/// so the status page can surface probe traffic without inflating real
/// route counts.
///
/// Uses `Relaxed` ordering on the per-route DashMap counter because it's a
/// display-only metric — no other code branches on its value, so stricter
/// ordering would be overkill. The Prometheus counter uses the same
/// `MatchedPath` template (e.g. `/v1/devices/{device_id}/sessions`) rather
/// than the raw URI so label cardinality stays bounded by the route table.
///
/// Also pulls the SQLite pool stats once per request and refreshes the
/// `cmtrace_db_connections_in_use` gauge — sampling on the hot path keeps the
/// gauge fresh without spawning a background task. The pool snapshot is a
/// non-blocking read of two atomics, so the cost is negligible.
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
        .entry(key.clone())
        .or_insert_with(|| AtomicU64::new(0))
        .fetch_add(1, Ordering::Relaxed);

    let start = Instant::now();
    let response = next.run(req).await;
    let elapsed = start.elapsed().as_secs_f64();

    metrics::counter!("cmtrace_http_requests_total", "path" => key.clone()).increment(1);
    metrics::histogram!("cmtrace_http_request_duration_seconds", "path" => key).record(elapsed);

    // Refresh the pool gauge from the snapshot. (size - idle) gives us the
    // connections currently in-use, which is what the operator cares about
    // for spotting saturation.
    let pool = state.meta.pool_stats();
    let in_use = pool.size.saturating_sub(pool.idle);
    metrics::gauge!("cmtrace_db_connections_in_use").set(f64::from(in_use));

    response
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

    // Parse-state distribution across ALL sessions (not just recent N).
    // Backends without a real impl (PG today) return an empty vec via the
    // trait default; the renderer handles that as a muted placeholder so
    // the card still appears with an honest "no data" message rather than
    // being silently omitted.
    let state_dist = fetch_state_distribution(&state.meta).await;

    // Device roster for the summary + top-devices panel. Cap at 1000 — this
    // is a dev dashboard, not a paginated API surface. Degrade to empty on
    // storage errors for the same reason as `recent`.
    const DEVICE_CAP: u32 = 1000;
    let mut devices = state
        .meta
        .list_devices(DEVICE_CAP, None)
        .await
        .unwrap_or_default();
    devices.sort_by(|a, b| {
        b.session_count
            .cmp(&a.session_count)
            .then_with(|| b.last_seen_utc.cmp(&a.last_seen_utc))
    });

    let body = render_html(
        &state, uptime, total, &routes, pool, &recent, &devices, &state_dist,
    );

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

/// Map a `parse_state` value to the CSS class suffix used on the
/// `.state-<slug>` pill. Keep this list closed: any future state the
/// parser starts emitting (e.g. `timeout`) falls through to `unknown` and
/// picks up the neutral styling rather than crashing the renderer or
/// inheriting the wrong color. See `STATE_OK` et al in
/// `pipeline::parse_worker` for the canonical emitter side.
fn state_slug(state: &str) -> &'static str {
    match state {
        "ok" => "ok",
        "ok-with-fallbacks" => "ok-fallbacks",
        "partial" => "partial",
        "failed" => "failed",
        "pending" => "pending",
        _ => "unknown",
    }
}

/// Render a parse_state value wrapped in a color-coded pill span. Shared
/// between the Recent-bundles table and the Parse-state-distribution
/// card so the colors line up visually.
fn render_state_pill(state: &str) -> String {
    format!(
        "<span class=\"state state-{slug}\">{label}</span>",
        slug = state_slug(state),
        label = esc(state),
    )
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
            state = render_state_pill(&s.parse_state),
            // RFC 3339 always renders as ASCII so esc is overkill, but keep
            // it for symmetry with the other cells in case the formatter
            // ever changes.
            ts = esc(&s.ingested_utc.to_rfc3339()),
        ));
    }
    out.push_str("</tbody></table>");
    out
}

/// Best-effort pull of the per-state session count. Returns an empty vec on
/// any storage error so the dev status page keeps rendering even if the
/// metadata store is flaky — matches how `recent_sessions` is handled in
/// the parent handler.
async fn fetch_state_distribution(meta: &Arc<dyn MetadataStore>) -> Vec<(String, u64)> {
    meta.count_sessions_by_state().await.unwrap_or_default()
}

/// Render the parse-state distribution strip as a `<ul class="pill-list">`
/// so the counts render inline and wrap gracefully on narrow viewports.
/// Each item uses the same pill styling as the Recent-bundles table cell
/// so the colors line up visually between the two cards.
fn render_state_distribution(dist: &[(String, u64)]) -> String {
    if dist.is_empty() {
        return "<p class=\"muted\">No sessions yet.</p>".to_string();
    }
    let mut out = String::from("<ul class=\"pill-list\">");
    for (state, count) in dist {
        out.push_str(&format!(
            "<li>{pill} <span class=\"pill-count\">{count}</span></li>",
            pill = render_state_pill(state),
            count = count,
        ));
    }
    out.push_str("</ul>");
    out
}

/// Render the "top devices" table (by session count). Shows up to 5 rows so
/// the dashboard stays glanceable. Falls back to the same muted empty-state
/// copy pattern as the other panels.
fn render_top_devices(devices: &[DeviceRow]) -> String {
    if devices.is_empty() {
        return "<p class=\"muted\">No devices registered yet.</p>".to_string();
    }
    let mut out = String::from(
        "<table class=\"recent\">\
         <thead><tr>\
         <th>device_id</th>\
         <th>hostname</th>\
         <th>sessions</th>\
         <th>last_seen</th>\
         </tr></thead><tbody>",
    );
    for d in devices.iter().take(5) {
        out.push_str(&format!(
            "<tr><td>{dev}</td><td>{host}</td><td>{count}</td><td>{ts}</td></tr>",
            dev = esc(&d.device_id),
            host = esc(d.hostname.as_deref().unwrap_or("—")),
            count = d.session_count,
            ts = esc(&d.last_seen_utc.to_rfc3339()),
        ));
    }
    out.push_str("</tbody></table>");
    out
}

/// Summarize the auth + transport-security posture. Honest "disabled" vs
/// "enabled" strings so it's obvious at a glance if the server is running
/// wide-open (e.g. local dev with CMTRACE_AUTH_MODE=disabled).
fn render_auth_panel(state: &AppState) -> String {
    let mut rows: Vec<(&str, String)> = Vec::new();

    let (mode_label, mode_detail) = match state.auth.mode {
        AuthMode::Enabled => ("enabled", state.auth.entra.is_some()),
        AuthMode::Disabled => ("disabled", false),
    };
    rows.push(("Auth mode", mode_label.to_string()));

    if mode_detail {
        if let Some(entra) = &state.auth.entra {
            rows.push(("Tenant", entra.tenant_id.clone()));
            rows.push(("Audience", entra.audience.clone()));
        }
    }

    rows.push((
        "mTLS",
        if state.mtls.require_on_ingest {
            "required on ingest".to_string()
        } else {
            "disabled".to_string()
        },
    ));

    let rl = &state.rate_limit;
    let rl_bits = [
        rl.device_ingest.is_some().then_some("device-ingest"),
        rl.ip_ingest.is_some().then_some("ip-ingest"),
        rl.ip_query.is_some().then_some("ip-query"),
    ];
    let rl_active: Vec<&str> = rl_bits.into_iter().flatten().collect();
    rows.push((
        "Rate limit",
        if rl_active.is_empty() {
            "disabled".to_string()
        } else {
            rl_active.join(", ")
        },
    ));

    let cors = if state.cors.allowed_origins.is_empty() {
        "(none)".to_string()
    } else {
        state.cors.allowed_origins.join(", ")
    };
    rows.push(("CORS origins", cors));

    let mut out = String::from("<dl>");
    for (k, v) in rows {
        out.push_str(&format!("<dt>{}</dt><dd>{}</dd>", esc(k), esc(&v)));
    }
    out.push_str("</dl>");
    out
}

// This helper wires together every dashboard section with its rendered
// fragment; the flat parameter list keeps each section composable and
// testable in isolation. Hiding these behind a struct just for the sake
// of the lint would obscure the call site without adding real cohesion.
#[allow(clippy::too_many_arguments)]
fn render_html(
    state: &AppState,
    uptime: Duration,
    total_requests: u64,
    routes: &[(String, u64)],
    pool: PoolStats,
    recent: &[SessionRow],
    devices: &[DeviceRow],
    state_dist: &[(String, u64)],
) -> String {
    let service = env!("CARGO_PKG_NAME");
    let version = env!("CARGO_PKG_VERSION");
    // Rust version captured at build time (see build.rs).
    let rustc = env!("RUSTC_VERSION");
    let uptime_h = humanize(uptime);
    let routes_html = render_routes(routes);
    let recent_html = render_recent(recent);
    let top_devices_html = render_top_devices(devices);
    let state_dist_html = render_state_distribution(state_dist);
    let auth_html = render_auth_panel(state);
    let total_devices = devices.len();
    let total_sessions: i64 = devices.iter().map(|d| d.session_count).sum();

    format!(
        r#"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<title>{service} status</title>
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta http-equiv="refresh" content="10">
<style>
  :root {{
    color-scheme: light dark;
    --bg: #fafafa;
    --fg: #1a1a1a;
    --muted: #6b7280;
    --card: #ffffff;
    --border: #e5e7eb;
    --accent: #2563eb;
    /* Parse-state pill palette (light). Chosen to stay readable against
       a white --card background: low-saturation tints for the fill and a
       darker shade of the same hue for the text, so 4.5:1 contrast is
       comfortably met without needing a heavy border. */
    --pill-ok-bg:           #d1fadf;
    --pill-ok-fg:           #0f5132;
    --pill-ok-fb-bg:        #fef3c7;
    --pill-ok-fb-fg:        #7c4a03;
    --pill-partial-bg:      #ffedd5;
    --pill-partial-fg:      #9a3412;
    --pill-failed-bg:       #fee2e2;
    --pill-failed-fg:       #991b1b;
    --pill-pending-bg:      #e5e7eb;
    --pill-pending-fg:      #374151;
    --pill-unknown-bg:      #e5e7eb;
    --pill-unknown-fg:      #374151;
  }}
  @media (prefers-color-scheme: dark) {{
    :root {{
      --bg: #0f1115;
      --fg: #e5e7eb;
      --muted: #9ca3af;
      --card: #171a21;
      --border: #2a2f3a;
      --accent: #60a5fa;
      /* Dark-mode pill palette. Darker fills (derived from the same hue
         as the light values) keep the card-background contrast honest,
         and the fg stays bright so the label reads at a glance. */
      --pill-ok-bg:         #14532d;
      --pill-ok-fg:         #bbf7d0;
      --pill-ok-fb-bg:      #713f12;
      --pill-ok-fb-fg:      #fde68a;
      --pill-partial-bg:    #7c2d12;
      --pill-partial-fg:    #fed7aa;
      --pill-failed-bg:     #7f1d1d;
      --pill-failed-fg:     #fecaca;
      --pill-pending-bg:    #374151;
      --pill-pending-fg:    #d1d5db;
      --pill-unknown-bg:    #374151;
      --pill-unknown-fg:    #d1d5db;
    }}
  }}
  body {{
    margin: 0;
    padding: 2rem 1rem;
    font: 14px/1.5 ui-sans-serif, system-ui, -apple-system, Segoe UI, Roboto, sans-serif;
    background: var(--bg);
    color: var(--fg);
  }}
  main {{ max-width: 960px; margin: 0 auto; }}
  .grid-2 {{ display: grid; grid-template-columns: 1fr 1fr; gap: 1rem; margin-bottom: 1rem; }}
  .grid-2 > section.card {{ margin-bottom: 0; }}
  @media (max-width: 720px) {{ .grid-2 {{ grid-template-columns: 1fr; }} }}
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
  /* Parse-state pill. Inline-block so it sits cleanly inside a table cell
     next to surrounding monospace text; tight line-height + small padding
     so the pill doesn't enlarge the row height. */
  .state {{
    display: inline-block;
    padding: 0.08rem 0.5rem;
    border-radius: 999px;
    font-size: 0.75rem;
    font-weight: 600;
    line-height: 1.4;
    letter-spacing: 0.01em;
    font-family: ui-sans-serif, system-ui, -apple-system, Segoe UI, Roboto, sans-serif;
    background: var(--pill-unknown-bg);
    color: var(--pill-unknown-fg);
    white-space: nowrap;
  }}
  .state-ok            {{ background: var(--pill-ok-bg);        color: var(--pill-ok-fg); }}
  .state-ok-fallbacks  {{ background: var(--pill-ok-fb-bg);     color: var(--pill-ok-fb-fg); }}
  .state-partial       {{ background: var(--pill-partial-bg);   color: var(--pill-partial-fg); }}
  .state-failed        {{ background: var(--pill-failed-bg);    color: var(--pill-failed-fg); }}
  .state-pending       {{ background: var(--pill-pending-bg);   color: var(--pill-pending-fg); }}
  /* Parse-state distribution strip. Horizontal wrap of (pill + count)
     pairs separated by a subtle dot so the card stays glanceable on
     narrow viewports. */
  ul.pill-list {{
    list-style: none;
    padding: 0;
    margin: 0;
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem 1rem;
    align-items: center;
  }}
  ul.pill-list li {{ display: inline-flex; align-items: center; gap: 0.4rem; }}
  ul.pill-list li + li {{ position: relative; padding-left: 1rem; }}
  ul.pill-list li + li::before {{
    content: "·";
    color: var(--muted);
    position: absolute;
    left: 0.35rem;
  }}
  .pill-count {{
    font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
    font-size: 0.85rem;
    color: var(--fg);
  }}
  footer {{ color: var(--muted); font-size: 0.8rem; margin-top: 1.5rem; }}
</style>
</head>
<body>
<main>
  <h1>{service}<span class="version">v{version}</span></h1>
  <p class="subtitle">Dev-debugging status page. Auto-refreshes every 10s. Not production-safe; firewall off in real deployments.</p>

  <div class="grid-2">
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
      <h2>Auth &amp; security</h2>
      {auth_html}
    </section>
  </div>

  <section class="card">
    <h2>Storage &amp; data</h2>
    <dl>
      <dt>DB pool</dt>
      <dd>size: {pool_size} / idle: {pool_idle} / max: {pool_max}</dd>
      <dt>Devices</dt>
      <dd>{total_devices}</dd>
      <dt>Sessions (sum)</dt>
      <dd>{total_sessions}</dd>
    </dl>
  </section>

  <section class="card">
    <h2>Top devices</h2>
    {top_devices_html}
  </section>

  <section class="card">
    <h2>Parse-state distribution</h2>
    {state_dist_html}
  </section>

  <section class="card">
    <h2>Recent bundles</h2>
    {recent_html}
  </section>

  <section class="card">
    <h2>Top routes</h2>
    {routes_html}
  </section>

  <div class="grid-2">
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
        <li><a href="http://{host_only}:8083/">CMTrace Open viewer</a></li>
        <li><a href="http://{host_only}:8082/">Adminer (Postgres UI)</a></li>
      </ul>
    </section>
  </div>

  <footer>cmtraceopen-api &middot; v{version} &middot; rustc {rustc}</footer>
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
        top_devices_html = top_devices_html,
        state_dist_html = state_dist_html,
        auth_html = auth_html,
        total_devices = total_devices,
        total_sessions = total_sessions,
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
        let configs: Arc<dyn crate::storage::ConfigStore> = meta.clone();
        AppState {
            meta,
            blobs,
            audit: Arc::new(crate::storage::NoopAuditStore),
            configs,
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
            #[cfg(feature = "crl")]
            crl_cache: None,
            // Share the process-wide recorder so handler tests don't double-
            // install (the metrics-rs recorder is global + install-once).
            metrics: crate::state::install_metrics_recorder(),
            rate_limit: Arc::new(crate::state::RateLimitState::disabled()),
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
        // parse_state is rendered as a color-coded pill so the operator can
        // spot unhealthy bundles at a glance.
        assert!(html.contains("<span class=\"state state-ok\">ok</span>"));
    }

    #[test]
    fn state_slug_maps_known_states() {
        // Closed list — every known value the parse worker emits (see
        // `STATE_OK` et al in pipeline::parse_worker) plus `pending` which
        // is the schema default before the worker runs. Anything else
        // falls through to `unknown` so the renderer can't crash on a
        // future state.
        assert_eq!(state_slug("ok"), "ok");
        assert_eq!(state_slug("ok-with-fallbacks"), "ok-fallbacks");
        assert_eq!(state_slug("partial"), "partial");
        assert_eq!(state_slug("failed"), "failed");
        assert_eq!(state_slug("pending"), "pending");
        // Unknown state -> neutral pill.
        assert_eq!(state_slug("timeout"), "unknown");
    }

    #[test]
    fn render_state_distribution_empty() {
        // Empty input renders the muted-copy placeholder so the card
        // surfaces a clear "nothing yet" rather than being silently omitted.
        let html = render_state_distribution(&[]);
        assert!(html.contains("No sessions yet"));
        assert!(html.contains("class=\"muted\""));
    }

    #[test]
    fn render_state_distribution_renders_counts() {
        // Synthetic input from the SQL GROUP BY: two states, distinct
        // counts. Assert that (a) each value is wrapped in the correct
        // state-<slug> class, and (b) the count text appears next to it.
        let dist: Vec<(String, u64)> = vec![
            ("ok".to_string(), 1),
            ("partial".to_string(), 140),
        ];
        let html = render_state_distribution(&dist);
        // Class wrapping for each state.
        assert!(html.contains("<span class=\"state state-ok\">ok</span>"));
        assert!(
            html.contains("<span class=\"state state-partial\">partial</span>"),
        );
        // Counts rendered via the .pill-count span.
        assert!(html.contains("<span class=\"pill-count\">1</span>"));
        assert!(html.contains("<span class=\"pill-count\">140</span>"));
        // Structural wrapper.
        assert!(html.contains("<ul class=\"pill-list\">"));
    }

    #[tokio::test]
    async fn render_html_contains_expected_fields() {
        let state = fake_state("0.0.0.0:8080").await;
        let pool = PoolStats { size: 3, idle: 2, max_size: 8 };
        let routes = vec![("/healthz".to_string(), 42)];
        let html = render_html(
            &state,
            Duration::from_secs(65),
            42,
            &routes,
            pool,
            &[],
            &[],
            &[],
        );
        assert!(html.contains("<!doctype html>"));
        assert!(html.contains("api-server"));
        assert!(html.contains("1m 5s"));
        assert!(html.contains(">42<"));
        assert!(html.contains("0.0.0.0:8080"));
        assert!(html.contains("/healthz"));
        assert!(html.contains(":8082"));
        // Viewer link points at the containerized viewer on :8083, not the
        // legacy :5173 dev-server port.
        assert!(html.contains(":8083/"));
        assert!(!html.contains(":5173"));
        // Auto-refresh meta tag so the dashboard stays "live" without JS.
        assert!(html.contains("http-equiv=\"refresh\""));
        // Storage section populated with the supplied PoolStats.
        assert!(html.contains("Storage &amp; data"));
        assert!(html.contains("DB pool"));
        assert!(html.contains("size: 3 / idle: 2 / max: 8"));
        // Auth panel: disabled by default in the fake_state helper.
        assert!(html.contains("Auth &amp; security"));
        assert!(html.contains("disabled"));
        // Top devices section renders the empty-state copy.
        assert!(html.contains("Top devices"));
        assert!(html.contains("No devices registered yet"));
        // Parse-state distribution card renders between Top devices and
        // Recent bundles, with the empty-state copy when no sessions
        // have been ingested yet. Use the <h2> tag to disambiguate from
        // the same string appearing inside the embedded <style> comments.
        assert!(html.contains("<h2>Parse-state distribution</h2>"));
        assert!(html.contains("No sessions yet"));
        let pos_top_devices = html.find("<h2>Top devices</h2>").unwrap();
        let pos_dist = html.find("<h2>Parse-state distribution</h2>").unwrap();
        let pos_recent = html.find("<h2>Recent bundles</h2>").unwrap();
        assert!(
            pos_top_devices < pos_dist && pos_dist < pos_recent,
            "distribution card must render between Top devices and Recent bundles",
        );
        // Recent bundles section renders the empty-state copy.
        assert!(html.contains("Recent bundles"));
        assert!(html.contains("No bundles ingested yet"));
        // Top routes section shows the supplied route + count.
        assert!(html.contains("Top routes"));
        assert!(html.contains("<dt>/healthz</dt><dd>42</dd>"));
        // Pill CSS is present in the embedded <style> block so the colors
        // are available whether or not any state rows render today.
        assert!(html.contains(".state-ok"));
        assert!(html.contains(".state-failed"));
    }

    #[test]
    fn render_top_devices_empty_state() {
        assert!(render_top_devices(&[]).contains("No devices registered yet"));
    }

    #[test]
    fn render_top_devices_renders_table_and_caps_at_five() {
        let now = Utc::now();
        let mk = |id: &str, count: i64| DeviceRow {
            device_id: id.into(),
            first_seen_utc: now,
            last_seen_utc: now,
            hostname: Some(format!("host-{id}")),
            session_count: count,
        };
        let rows: Vec<DeviceRow> = (0..7)
            .map(|i| mk(&format!("dev{i}"), 10 - i as i64))
            .collect();
        let html = render_top_devices(&rows);
        assert!(html.contains("dev0"));
        assert!(html.contains("dev4"));
        // Capped at 5 rows — dev5 and dev6 must not appear.
        assert!(!html.contains("dev5"));
        assert!(!html.contains("dev6"));
        assert!(html.contains("host-dev0"));
    }

    #[tokio::test]
    async fn render_auth_panel_disabled_by_default() {
        let state = fake_state("0.0.0.0:8080").await;
        let html = render_auth_panel(&state);
        assert!(html.contains("Auth mode"));
        assert!(html.contains("disabled"));
        assert!(html.contains("Rate limit"));
        assert!(html.contains("CORS origins"));
        // fake_state has no Entra config wired up; tenant/audience should
        // not render when auth is disabled.
        assert!(!html.contains("Tenant"));
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
