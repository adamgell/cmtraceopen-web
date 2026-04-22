# Audit Log — Design & Implementation

> Wave 4 · P1.2 · Status: shipped (v0.2.0)

## 1  Goal

Every admin/operator action against the api-server lands in a tamper-evident
audit log.  Compliance gate for SOC 2 / SOX / HIPAA-style asks.

## 2  Schema

```sql
CREATE TABLE audit_log (
  id                TEXT    NOT NULL PRIMARY KEY,  -- UUID v7 (time-sortable)
  ts_utc            TEXT    NOT NULL,              -- ISO-8601
  principal_kind    TEXT    NOT NULL,              -- 'operator' | 'admin' | 'device' | 'system'
  principal_id      TEXT    NOT NULL,              -- JWT sub / device cert SAN URI
  principal_display TEXT,                          -- user.name / cert CN (optional)
  action            TEXT    NOT NULL,              -- 'device.disable' | 'audit.list' | …
  target_kind       TEXT,                          -- 'device' | 'session' | 'bundle' | NULL
  target_id         TEXT,                          -- natural key of the resource | NULL
  result            TEXT    NOT NULL,              -- 'success' | 'failure'
  details_json      TEXT,                          -- sanitized extras (no PII)
  request_id        TEXT                           -- optional cross-correlation UUID
);

CREATE INDEX idx_audit_log_ts        ON audit_log(ts_utc DESC);
CREATE INDEX idx_audit_log_principal ON audit_log(principal_id, ts_utc DESC);
CREATE INDEX idx_audit_log_action    ON audit_log(action, ts_utc DESC);
```

Migration: `crates/api-server/migrations/0003_audit_log.sql`

### Column notes

| Column | Notes |
|---|---|
| `id` | UUID v7 — lexicographic sort ≈ insertion order; PK so each row is individually addressable |
| `ts_utc` | ISO-8601 string so both sqlx/chrono and plain `sqlite3` shell reads work without special tooling |
| `principal_kind` | Derived from the JWT `roles` claim: `admin` if `CmtraceOpen.Admin` is present, otherwise `operator`; `anonymous` if auth failed |
| `principal_id` | JWT `sub` (stable OID in Entra).  Never a display name — display names can change. |
| `principal_display` | JWT `name` claim; informational only.  Absent when the token carries no name. |
| `action` | Dot-namespaced verb: `device.disable`, `session.reparse`, `audit.list`, etc. |
| `target_kind` / `target_id` | The entity the action was applied to.  `NULL` for list/meta actions. |
| `result` | `success` = HTTP 2xx; `failure` = everything else (4xx, 5xx, auth-denied). |
| `details_json` | Reserved for action-specific extras.  **MUST NOT** contain PII: no hostnames, no free-text from request bodies. |
| `request_id` | Optional UUID for correlation with distributed tracing.  Populated by clients that set a trace header. |

## 3  Architecture

### 3.1  `AuditStore` trait

```
crates/api-server/src/storage/mod.rs
  trait AuditStore
    insert_audit_row(NewAuditRow) → Result<(), StorageError>
    list_audit_rows(AuditFilters, limit) → Result<Vec<AuditRow>, StorageError>
```

The trait sits alongside `MetadataStore` and `BlobStore` in the storage
abstraction layer.  `NoopAuditStore` (also in `mod.rs`) is used by all
existing test scaffolding that doesn't exercise the audit surface.

### 3.2  SQLite implementation

```
crates/api-server/src/storage/audit_sqlite.rs
  struct AuditSqliteStore { pool: SqlitePool }
```

`AuditSqliteStore` shares the existing WAL-mode connection pool via
`SqliteMetadataStore::audit_store()`, which is a cheap Arc clone.  No second
database file, no second connection pool.

### 3.3  Middleware

```
crates/api-server/src/middleware/audit.rs
  pub async fn audit_middleware(State, Request, Next) → Response
```

Applied to the admin sub-router in `routes/admin.rs` via:

```rust
.layer(middleware::from_fn_with_state(state.clone(), audit_middleware))
```

The middleware:

1. Captures the Axum `MatchedPath` template and the request URI *before*
   forwarding the request so they are available after the inner handler
   consumes the body.
2. Calls `OperatorPrincipal::from_request_parts` to extract the JWT principal.
   Because `from_request_parts` only reads the `Authorization` header (not the
   body), calling it in the middleware is safe and idempotent — the handler's
   own `RequireRole` extractor will run the same check again.
3. Runs `next.run(request)` to invoke the actual handler.
4. Derives `result` from the response status (`2xx = success`, else `failure`).
5. Writes one row to `audit_log` via `AuditStore::insert_audit_row`.
6. If the write fails it **logs a warning but does not propagate the error** —
   an audit side-effect MUST NOT roll back a completed admin action.

`GET /v1/admin/audit` is explicitly excluded from self-logging to prevent the
audit table growing with self-referential read entries.

### 3.4  `AppState` wiring

`AppState` now carries `pub audit: Arc<dyn AuditStore>`.  The two constructors
used by `main.rs` (`full_with_audit` and `with_cors_crl_and_audit`) accept an
explicit store; all existing convenience constructors (`new`, `with_cors`,
`new_auth_disabled`, etc.) default to `NoopAuditStore` so no existing test
code had to change.

## 4  Read API

```
GET /v1/admin/audit
  ?after_ts=<ISO-8601>    exclusive lower bound on ts_utc
  ?principal=<sub>        filter to a specific principal_id
  ?action=device.disable  filter to a specific action string
  ?limit=100              max rows (1–1 000; default 100)
```

Returns:

```json
{
  "items": [
    {
      "id": "019...",
      "ts_utc": "2025-04-22T03:40:00Z",
      "principal_kind": "admin",
      "principal_id": "00000000-0000-0000-0000-000000000000",
      "principal_display": "Alice Admin",
      "action": "device.disable",
      "target_kind": "device",
      "target_id": "my-device-id",
      "result": "failure"
    }
  ],
  "count": 1
}
```

Results are ordered `ts_utc DESC` (most recent first).  Pagination via the
`after_ts` lower bound.

Role required: `CmtraceOpen.Admin`.

## 5  Action strings

| Route | Method | Action |
|---|---|---|
| `/v1/admin/devices/{device_id}/disable` | POST | `device.disable` |
| `/v1/admin/audit` | GET | `audit.list` (not self-logged) |

New routes added to the admin sub-router MUST be registered in `route_to_action`
(in `middleware/audit.rs`) to get a human-readable action string.

## 6  PII policy

`details_json` MUST NOT contain:

- Device hostnames or IP addresses
- User-supplied free-text from request bodies
- FQDN or SAN URI values beyond what's already in `principal_id`

The middleware sets `details_json = NULL` for the MVP.  Future handlers that
need structured extras must sanitize before inserting.

## 7  Tamper evidence (P2 — not yet shipped)

The current implementation is insert-only (no UPDATE / DELETE).  Hash
chaining (each row's `prev_hash = sha256(previous_row_bytes)`) and periodic
anchor commits to a signed external log are designed but deferred to v2.

The schema already has the `id` / `ts_utc` columns that a hash-chain column
(`prev_hash TEXT`) could be added to in a future migration without breaking
existing rows.

## 8  Retention

Default retention: **90 days** (see `docs/wave4/04-day2-operations.md` §4).
A periodic cleanup job (e.g. cron or a Tokio background task) should `DELETE
FROM audit_log WHERE ts_utc < datetime('now', '-90 days')`.  Configurable via
`CMTRACE_AUDIT_RETENTION_DAYS`.  Not yet implemented — P2.

## 9  Testing

`crates/api-server/tests/audit_integration.rs` covers:

- `disable_device_writes_audit_row_with_correct_fields` — happy path
- `failed_request_still_writes_audit_row` — non-2xx is still logged
- `audit_endpoint_returns_rows_reverse_chronological_with_limit` — pagination
- `audit_endpoint_action_filter` — action query param filters correctly
- `reading_audit_log_is_not_self_logged` — no self-logging

Unit tests for `route_to_action` and `route_to_target` live in
`src/middleware/audit.rs`.

## 10  Files changed

| Path | Change |
|---|---|
| `migrations/0003_audit_log.sql` | NEW — `audit_log` DDL |
| `src/storage/mod.rs` | Add `AuditStore` trait, `AuditRow`, `NewAuditRow`, `AuditFilters`, `NoopAuditStore` |
| `src/storage/audit_sqlite.rs` | NEW — `AuditSqliteStore` |
| `src/storage/meta_sqlite.rs` | Add `SqliteMetadataStore::audit_store()` |
| `src/middleware/mod.rs` | NEW — module declaration |
| `src/middleware/audit.rs` | NEW — `audit_middleware` |
| `src/state.rs` | Add `audit: Arc<dyn AuditStore>`; add `full_with_audit` + `with_cors_crl_and_audit` constructors |
| `src/routes/admin.rs` | Wire audit middleware; add `GET /v1/admin/audit` |
| `src/lib.rs` | Expose `pub mod middleware` |
| `src/main.rs` | Build audit store; pass to AppState |
| `src/routes/status.rs` | Update inline unit-test `fake_state` for new field |
| `tests/audit_integration.rs` | NEW — 5 integration tests |
| `docs/wave4/11-audit-log.md` | NEW — this document |
