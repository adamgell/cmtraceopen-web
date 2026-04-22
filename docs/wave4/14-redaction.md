# 14 — Telemetry Redaction Policy

**Status:** Implemented (Wave 4 P2.2)
**Owner:** Platform Engineering
**Legal review gate:** Required before broader fleet rollout (see §7)

---

## 1  Problem statement

Bundles collected by the CMTrace Open agent contain PII: usernames embedded
in file-system paths (`C:\Users\alice\…`), device GUIDs, e-mail addresses
in log lines, and internal IP addresses. Shipping raw bundles to the
api-server without scrubbing first creates legal exposure under GDPR, CCPA,
and corporate data-handling policy. Server-side redaction is a possible
defence-in-depth layer, but the right place to redact is at the source —
before the data ever leaves the endpoint.

---

## 2  Design

### 2.1  Architecture

```
Collector output (text)
        │
        ▼
┌──────────────────────┐
│  EvidenceOrchestrator│
│  ── collect pass ────│
│  ── redact_staging ──│──► .evtx / .reg skipped (v1 limitation, §6)
│  ── zip ─────────────│
└──────────────────────┘
        │
        ▼
  Bundle ZIP (redacted text files)
        │
        ▼
  Upload queue → api-server
```

The `Redactor` is constructed once from the agent config at startup (regex
compilation is the expensive step) and is shared — as an owned value — across
the entire collection pass. After all collectors have written their output to
the staging directory, `EvidenceOrchestrator::redact_staging_dir` walks every
file:

* **Text files** (`.log`, `.txt`, everything else without a binary extension)
  are read, passed through `Redactor::apply`, and rewritten in-place.
* **Binary files** (`.evtx`, `.reg`) are left untouched — see §6.

`Redactor::apply` returns a `Cow<str>` — when no rule matches the original
`&str` is returned as `Cow::Borrowed`, avoiding any heap allocation.

### 2.2  Configuration schema (`crates/agent/src/config.rs`)

```toml
[redaction]
enabled = true          # set false to forward raw data (e.g. forensic mode)

# Extra rules appended AFTER the built-in defaults. Leave empty for the
# defaults-only behaviour that covers most fleets.
[[redaction.patterns]]
name        = "hostname"
regex       = 'WIN-[A-Z0-9]{6,}'
replacement = "<HOSTNAME>"
```

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | `bool` | `true` | Master switch |
| `patterns` | `Vec<RedactionRule>` | `[]` | Operator-added rules |

Each `RedactionRule` has:
| Field | Type | Description |
|-------|------|-------------|
| `name` | `String` | Human-readable identifier (used in error messages) |
| `regex` | `String` | ECMAScript-compatible regex (Unicode enabled) |
| `replacement` | `String` | Replacement string; back-refs `$1`, `${name}` accepted |

### 2.3  Code layout

| File | Purpose |
|------|---------|
| `crates/agent/src/config.rs` | `RedactionRule`, `RedactionConfig` structs; `redaction` field on `AgentConfig` |
| `crates/agent/src/redact.rs` | `Redactor` struct, default rules, `from_config`, `apply` |
| `crates/agent/src/collectors/evidence.rs` | Wires `Redactor` into `EvidenceOrchestrator`; `redact_staging_dir` |
| `crates/agent/src/lib.rs` | `pub mod redact` export |
| `crates/agent/tests/redact_integration.rs` | Integration tests (requires `--features redaction`) |
| `tools/agent-redact-test.sh` | Operator preview script |

---

## 3  Default ruleset

| Rule name | Pattern | Replacement | Rationale |
|-----------|---------|-------------|-----------|
| `username_path` | `C:\Users\<name>\…` | `C:\Users\<USER>\…` | Windows home-dir path contains the login name |
| `guid` | `xxxxxxxx-xxxx-…` (128-bit hex GUID) | `<GUID>` | Enrollment IDs, device IDs, tenant IDs |
| `email` | `user@host.tld` | `<EMAIL>` | Admin/user addresses appear in dsregcmd output and Intune logs |
| `ipv4_internal` | `10.x.x.x` (RFC 1918 10/8 block) | `<INTERNAL_IP>` | Corp network topology. Public IPs are left in place for diagnostics. |

> **Why only 10/8?** The other RFC 1918 blocks (`192.168.x.x`, `172.16–31.x`)
> are also private but appear far less frequently in CM/Intune logs. They can
> be added via operator-supplied rules if needed. Erring on the side of
> preserving public IPs avoids hiding CDN/proxy addresses that aid
> connectivity diagnostics.

---

## 4  How to add custom rules

Add a `[[redaction.patterns]]` entry to the agent's `config.toml`:

```toml
[[redaction.patterns]]
name        = "asset_tag"
regex       = "ASSET-\\d{6}"
replacement = "<ASSET_TAG>"
```

Custom rules are appended **after** the built-in defaults so they cannot
accidentally shadow a default replacement.

Preview the effect before deploying with the operator script:

```bash
# Preview a live log file
./tools/agent-redact-test.sh /path/to/ccmexec.log

# Preview the built-in fixture (no real files needed)
./tools/agent-redact-test.sh --fixture

# Pipe stdin
cat /var/log/intune.log | ./tools/agent-redact-test.sh -
```

The script builds a small Rust binary in a temp directory that reuses the
same `Redactor::from_config` path as the production agent, so what you see
is exactly what the agent will produce.

---

## 5  Disabling redaction

Set `enabled = false` to operate in *forensic mode* — all collected text is
forwarded verbatim. This is useful when investigating a specific incident
where the operator has legal authority to process the raw data.

```toml
[redaction]
enabled = false
```

When disabled, `Redactor::is_noop()` returns `true` and
`EvidenceOrchestrator` skips the staging-dir walk entirely — zero overhead.

---

## 6  Known limitations (v1)

* **Binary files are NOT redacted.** `.evtx` (Windows Event Log export) and
  `.reg` (registry export) files are opaque binary formats. Redacting them
  would require a parse → extract text → redact → reserialize pipeline that
  is deferred to v2.  The agent flags these collectors in the manifest
  (`note: binary-not-redacted`) so the parse worker can treat them
  appropriately (e.g. restrict access, encrypt at rest on the server side).

* **172.16/12 and 192.168/16 private ranges** are not covered by default;
  add operator rules if needed.

* **Non-UTF-8 text files** are skipped silently (treated as binary). This
  is exceedingly rare in CM/Intune log outputs but is logged at `debug`
  level if it occurs.

---

## 7  Legal review checklist

- [ ] Legal sign-off that the default ruleset is sufficient for GDPR Art. 25
      (data minimisation) for the jurisdictions in scope.
- [ ] Privacy Impact Assessment (PIA) updated.
- [ ] Confirm that `<INTERNAL_IP>` token does not constitute "pseudonymisation"
      under applicable law (it is deterministic given the original IP).
- [ ] Operator runbook updated to document the `enabled = false` forensic-mode
      workflow and the access controls required when using it.

---

## 8  Performance

Regex compilation is amortised at startup. The `apply` hot path uses
`regex::Regex::replace_all`, which internally uses the Aho-Corasick multi-
pattern engine for literal literals and the NFA/DFA engine otherwise.

Benchmark target: **< 5 % overhead on 100 MB of collected text** (acceptance
criterion from the Wave 4 plan). The `Cow::Borrowed` fast path (no match ⇒
no allocation) keeps the common case (log files with no PII) close to a
single linear scan.

A micro-benchmark can be added under `crates/agent/benches/` using
`criterion` when the baseline is established.

---

## 9  Cross-references

* `docs/wave4/04-day2-operations.md` §5 — PII concerns in collected bundles
* `crates/agent/src/redact.rs` — implementation
* `crates/agent/tests/redact_integration.rs` — tests
* `tools/agent-redact-test.sh` — operator preview script
