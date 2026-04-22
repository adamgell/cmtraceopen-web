//! Schema-parity check between `migrations/` (SQLite) and `migrations-pg/`
//! (Postgres) trees.
//!
//! Per PR #77 review feedback, the two trees diverge intentionally on
//! type names (`INTEGER` vs `BIGINT`, `INTEGER PRIMARY KEY AUTOINCREMENT`
//! vs `BIGSERIAL PRIMARY KEY`) but MUST stay in lockstep on the *shape*
//! of the schema:
//!
//!   - The same set of tables exists in both trees.
//!   - The same set of column names exists in each table.
//!   - The same set of indexes exists.
//!
//! Without this check, a future PR can land an `audit_log` table on the
//! SQLite tree and silently forget the Postgres tree (the next operator
//! to flip `CMTRACE_DATABASE_URL=postgres://...` then hits a runtime
//! `relation "audit_log" does not exist` error). PR #79 is exactly that
//! shape — adds an `audit_log` table that needs to land in both trees.
//!
//! ## What this test does NOT check
//!
//! - **Column types**: deliberately diverge (`INTEGER` vs `BIGINT`,
//!   `INTEGER PRIMARY KEY AUTOINCREMENT` vs `BIGSERIAL`). The `Postgres
//!   storage types` ADR (`docs/adr/0001-postgres-storage-types.md`)
//!   explains why we picked TEXT for timestamps + JSON in both trees.
//! - **Constraint clauses**: `IF NOT EXISTS`, `REFERENCES`, `DEFAULT`
//!   wording differs across the two trees on purpose.
//! - **Comments / whitespace**: ignored.
//!
//! ## How
//!
//! Both trees are pure SQL files. We do a deliberately-simple line-based
//! parse: scan for `CREATE TABLE [IF NOT EXISTS] <name>` and
//! `CREATE INDEX [IF NOT EXISTS] <name>` headers, then for each table
//! collect the comma-separated column-name tokens between the parens.
//! No real SQL parser is needed — the migrations are hand-written, the
//! syntax is constrained, and a parse failure here just means the test
//! needs a small extension.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

/// Names of the two migration trees, relative to the api-server crate
/// root. Kept paired so the assert below scans them identically.
const TREES: &[(&str, &str)] = &[
    ("sqlite", "migrations"),
    ("postgres", "migrations-pg"),
];

/// Tables we deliberately allow to exist on only one tree.
///
/// `audit_log` is on the SQLite tree (PR #79) but not yet ported to the
/// Postgres tree — the ADR (`docs/adr/0001-postgres-storage-types.md`)
/// flags this as a follow-up, tracked in issue #110. Operators running
/// `CMTRACE_DATABASE_URL=postgres://...` won't get audit logging until
/// the Postgres translation lands; that's intentionally documented as a
/// known limitation. Once the migration lands, drop the entry.
const ALLOW_DIVERGENT_TABLES: &[&str] = &[
    // PR #79 — Postgres translation deferred per ADR 0001 (issue #110).
    "audit_log",
    // PR #106 (server-side config push) — Postgres translation deferred,
    // same Wave 4 follow-up batch as audit_log.
    "default_config_override",
    "device_config_overrides",
];

/// Indexes attached to tables in `ALLOW_DIVERGENT_TABLES`. Same rationale.
fn is_index_on_divergent_table(name: &str) -> bool {
    // Repo convention: `idx_<table>_<col>`. Trim the `idx_` prefix and
    // check whether the remainder begins with any allowed-divergent table
    // name. Conservative match — we'd rather skip an index incorrectly
    // than silently let a real drift through.
    if let Some(rest) = name.strip_prefix("idx_") {
        return ALLOW_DIVERGENT_TABLES
            .iter()
            .any(|t| rest.starts_with(&format!("{t}_")));
    }
    false
}

#[derive(Debug, Default, PartialEq, Eq)]
struct ParsedTree {
    /// table name → ordered set of column names.
    tables: BTreeMap<String, BTreeSet<String>>,
    /// index names. We don't track which table the index belongs to —
    /// the index name itself usually encodes the table (per the
    /// `idx_<table>_<col>` convention in this repo) and the parity
    /// check only cares that both trees define the same set of indexes.
    indexes: BTreeSet<String>,
}

fn crate_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn parse_tree(dir: &Path) -> ParsedTree {
    let mut out = ParsedTree::default();
    let mut entries: Vec<_> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("failed to read migration dir {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|s| s.to_str()) == Some("sql"))
        .collect();
    entries.sort();
    for path in entries {
        let sql = fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
        parse_sql_into(&sql, &mut out);
    }
    out
}

/// Strip line comments (`-- ...`) and normalise whitespace.
fn strip_comments(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    for line in sql.lines() {
        let line = line.split("--").next().unwrap_or("");
        out.push_str(line);
        out.push('\n');
    }
    out
}

fn parse_sql_into(sql: &str, parsed: &mut ParsedTree) {
    let cleaned = strip_comments(sql);
    let lower = cleaned.to_ascii_lowercase();

    // CREATE TABLE [IF NOT EXISTS] <name> ( ... );
    let mut search_from = 0usize;
    while let Some(rel_start) = lower[search_from..].find("create table") {
        let abs = search_from + rel_start;
        let after = &cleaned[abs + "create table".len()..];
        let after_lower = after.to_ascii_lowercase();
        let after_trim = after_lower.trim_start();
        let mut name_part: &str = after.trim_start();
        if after_trim.starts_with("if not exists") {
            // Skip "IF NOT EXISTS" in the original (case-preserving) slice.
            let leading_ws = after.len() - after.trim_start().len();
            let pos = leading_ws + "if not exists".len();
            name_part = after[pos..].trim_start();
        }
        let paren = match name_part.find('(') {
            Some(p) => p,
            None => break,
        };
        let table_name = name_part[..paren].trim().to_ascii_lowercase();
        // Body is between this `(` and the matching `)` — the migrations
        // are hand-written so balanced-paren counting is fine.
        let body_start = paren + 1;
        let mut depth = 1;
        let mut idx = body_start;
        let bytes = name_part.as_bytes();
        while idx < bytes.len() && depth > 0 {
            match bytes[idx] {
                b'(' => depth += 1,
                b')' => depth -= 1,
                _ => {}
            }
            idx += 1;
        }
        let body = &name_part[body_start..idx.saturating_sub(1)];
        let columns = parse_column_names(body);
        parsed
            .tables
            .entry(table_name)
            .or_default()
            .extend(columns);
        // Continue scanning after this CREATE TABLE.
        search_from = abs + "create table".len();
    }

    // CREATE [UNIQUE] INDEX [IF NOT EXISTS] <name> ON ...;
    let mut search_from = 0usize;
    while let Some(rel_start) = lower[search_from..].find("create ") {
        let abs = search_from + rel_start;
        let after = &lower[abs + "create ".len()..];
        // Allow "UNIQUE INDEX" too.
        let after = after.trim_start_matches("unique ").trim_start();
        if !after.starts_with("index") {
            search_from = abs + "create ".len();
            continue;
        }
        let mut idx_after: &str = &after["index".len()..];
        idx_after = idx_after.trim_start();
        if idx_after.starts_with("if not exists") {
            idx_after = idx_after["if not exists".len()..].trim_start();
        }
        let on_pos = match idx_after.find(" on ") {
            Some(p) => p,
            None => break,
        };
        let name = idx_after[..on_pos].trim().to_ascii_lowercase();
        if !name.is_empty() {
            parsed.indexes.insert(name);
        }
        search_from = abs + "create ".len();
    }
}

/// Parse column names from a `CREATE TABLE` body. Ignores constraint
/// rows (`PRIMARY KEY (...)`, `UNIQUE (...)`, `FOREIGN KEY ...`,
/// table-level `REFERENCES` clauses standalone).
fn parse_column_names(body: &str) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    // Split on top-level commas (depth 0). Some constraints contain
    // commas inside their own parens (e.g. `UNIQUE(a, b)`), so we have
    // to track depth instead of a naive split(',').
    let mut depth = 0i32;
    let mut start = 0usize;
    let bytes = body.as_bytes();
    let mut pieces: Vec<&str> = Vec::new();
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'(' => depth += 1,
            b')' => depth -= 1,
            b',' if depth == 0 => {
                pieces.push(&body[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    pieces.push(&body[start..]);

    for piece in pieces {
        let trimmed = piece.trim();
        if trimmed.is_empty() {
            continue;
        }
        let lower = trimmed.to_ascii_lowercase();
        // Skip table-level constraint rows that don't start with a
        // column name.
        if lower.starts_with("primary key")
            || lower.starts_with("unique")
            || lower.starts_with("foreign key")
            || lower.starts_with("check")
            || lower.starts_with("constraint")
        {
            continue;
        }
        // First whitespace-separated token is the column name.
        let name = trimmed.split_ascii_whitespace().next().unwrap_or("");
        if !name.is_empty() {
            out.insert(name.to_ascii_lowercase());
        }
    }
    out
}

#[test]
fn migration_trees_have_identical_table_set() {
    let mut parsed: BTreeMap<&str, ParsedTree> = BTreeMap::new();
    for (label, dir) in TREES {
        let path = crate_root().join(dir);
        parsed.insert(label, parse_tree(&path));
    }
    let sqlite_tables: BTreeSet<_> = parsed["sqlite"]
        .tables
        .keys()
        .filter(|t| !ALLOW_DIVERGENT_TABLES.contains(&t.as_str()))
        .cloned()
        .collect();
    let pg_tables: BTreeSet<_> = parsed["postgres"]
        .tables
        .keys()
        .filter(|t| !ALLOW_DIVERGENT_TABLES.contains(&t.as_str()))
        .cloned()
        .collect();
    let only_sqlite: Vec<_> = sqlite_tables.difference(&pg_tables).collect();
    let only_pg: Vec<_> = pg_tables.difference(&sqlite_tables).collect();
    assert!(
        only_sqlite.is_empty() && only_pg.is_empty(),
        "migration trees diverged: tables only in sqlite={only_sqlite:?}, \
         tables only in postgres={only_pg:?}.\n\
         If a new table was added to one tree, mirror it in the other tree \
         (with engine-appropriate type names) before merging. See \
         docs/adr/0001-postgres-storage-types.md for the typing policy. \
         If the divergence is intentional + tracked, add the table name to \
         ALLOW_DIVERGENT_TABLES.",
    );
}

#[test]
fn migration_trees_have_identical_columns_per_table() {
    let mut parsed: BTreeMap<&str, ParsedTree> = BTreeMap::new();
    for (label, dir) in TREES {
        let path = crate_root().join(dir);
        parsed.insert(label, parse_tree(&path));
    }
    let mut diffs: Vec<String> = Vec::new();
    let sqlite_tables = &parsed["sqlite"].tables;
    let pg_tables = &parsed["postgres"].tables;
    for (table, sqlite_cols) in sqlite_tables {
        let pg_cols = match pg_tables.get(table) {
            Some(c) => c,
            None => continue, // table-set test catches this.
        };
        let only_sqlite: Vec<_> = sqlite_cols.difference(pg_cols).cloned().collect();
        let only_pg: Vec<_> = pg_cols.difference(sqlite_cols).cloned().collect();
        if !only_sqlite.is_empty() || !only_pg.is_empty() {
            diffs.push(format!(
                "  table {table:?}: only-in-sqlite={only_sqlite:?}, only-in-postgres={only_pg:?}"
            ));
        }
    }
    assert!(
        diffs.is_empty(),
        "migration trees have per-table column drift:\n{}\n\
         Mirror the column on the other tree (with engine-appropriate type) \
         before merging.",
        diffs.join("\n"),
    );
}

#[test]
fn migration_trees_have_identical_index_set() {
    let mut parsed: BTreeMap<&str, ParsedTree> = BTreeMap::new();
    for (label, dir) in TREES {
        let path = crate_root().join(dir);
        parsed.insert(label, parse_tree(&path));
    }
    let sqlite_idx: BTreeSet<_> = parsed["sqlite"]
        .indexes
        .iter()
        .filter(|i| !is_index_on_divergent_table(i))
        .cloned()
        .collect();
    let pg_idx: BTreeSet<_> = parsed["postgres"]
        .indexes
        .iter()
        .filter(|i| !is_index_on_divergent_table(i))
        .cloned()
        .collect();
    let only_sqlite: Vec<_> = sqlite_idx.difference(&pg_idx).collect();
    let only_pg: Vec<_> = pg_idx.difference(&sqlite_idx).collect();
    assert!(
        only_sqlite.is_empty() && only_pg.is_empty(),
        "index drift between migration trees: \
         only-in-sqlite={only_sqlite:?}, only-in-postgres={only_pg:?}.\n\
         A missing index on Postgres causes silent O(N) scan regressions \
         in production — keep the index sets identical across trees.",
    );
}

// Sanity: the parser itself should yield non-empty results so a buggy
// migration-loader regression doesn't masquerade as "trees in sync".
#[test]
fn parser_recognises_at_least_one_table_in_each_tree() {
    for (_label, dir) in TREES {
        let path = crate_root().join(dir);
        let parsed = parse_tree(&path);
        assert!(
            !parsed.tables.is_empty(),
            "no tables parsed from {} — parser regression?",
            path.display(),
        );
    }
}
