//! PII-redaction layer.
//!
//! ## Overview
//!
//! [`Redactor`] compiles a set of regular-expression rules once at startup
//! and exposes a single [`Redactor::apply`] method that replaces PII in an
//! arbitrary text string. When there are no matches the method returns a
//! borrowed [`Cow`] — no allocation occurs on the fast path.
//!
//! ## Default rules
//!
//! | Name | Pattern | Replacement |
//! |------|---------|-------------|
//! | `username_path` | `C:\Users\<name>\…` | `C:\Users\<USER>\…` |
//! | `guid` | `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx` | `<GUID>` |
//! | `email` | `user@example.com` | `<EMAIL>` |
//! | `ipv4_internal` | `10.x.x.x` | `<INTERNAL_IP>` |
//!
//! Operator-supplied rules are appended after the defaults (see
//! [`crate::config::RedactionConfig::patterns`]). The defaults can be
//! bypassed entirely by setting `enabled = false` in the config and
//! providing an explicit list.
//!
//! ## Integration
//!
//! The [`crate::collectors::evidence::EvidenceOrchestrator`] holds a
//! `Redactor` and applies it to every text file in the staging directory
//! before the bundle is zipped. Binary files (`.evtx`, `.reg`) are skipped
//! — see `docs/wave4/14-redaction.md` for the known-limitation rationale.

use std::borrow::Cow;

use regex::Regex;

use crate::config::{AgentConfig, RedactionRule};

// ─── Error type ──────────────────────────────────────────────────────────────

/// Error returned when a regex rule fails to compile.
#[derive(Debug, thiserror::Error)]
pub enum RedactorError {
    #[error("invalid regex in rule '{name}': {source}")]
    InvalidRegex {
        name: String,
        #[source]
        source: regex::Error,
    },
}

// ─── Internal ────────────────────────────────────────────────────────────────

struct CompiledRule {
    #[allow(dead_code)] // stored for future diagnostics / tracing
    name: String,
    pattern: Regex,
    replacement: String,
}

// ─── Default rules ───────────────────────────────────────────────────────────

/// Rules baked into every agent build. Operator rules from
/// [`crate::config::RedactionConfig::patterns`] are appended after these.
pub fn default_rules() -> Vec<RedactionRule> {
    vec![
        RedactionRule {
            name: "username_path".into(),
            // Matches `C:\Users\<anything up to next backslash>`
            regex: r"(C:\\Users\\)([^\\]+)".into(),
            replacement: r"$1<USER>".into(),
        },
        RedactionRule {
            name: "guid".into(),
            // `\b` anchors so a longer hex token containing a 36-char
            // GUID-shaped substring isn't partially mangled.
            regex: r"\b[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}\b".into(),
            replacement: "<GUID>".into(),
        },
        RedactionRule {
            name: "email".into(),
            regex: r"\b[\w._%+\-]+@[\w.\-]+\.[a-zA-Z]{2,}\b".into(),
            replacement: "<EMAIL>".into(),
        },
        RedactionRule {
            name: "ipv4_internal".into(),
            // All three RFC 1918 private blocks: 10.0.0.0/8, 172.16.0.0/12,
            // 192.168.0.0/16. Public IPs are left intact for diagnostics.
            regex: concat!(
                r"\b(?:",
                r"10\.\d{1,3}\.\d{1,3}\.\d{1,3}",
                r"|172\.(?:1[6-9]|2\d|3[01])\.\d{1,3}\.\d{1,3}",
                r"|192\.168\.\d{1,3}\.\d{1,3}",
                r")\b",
            ).into(),
            replacement: "<INTERNAL_IP>".into(),
        },
        RedactionRule {
            name: "windows_sid".into(),
            // Domain SIDs of the form S-1-5-21-X-Y-Z-RID. Common in
            // dsregcmd /status output, Group Policy logs, security event
            // exports. The constant `S-1-5-21-` prefix scopes this to
            // domain principals (skips well-known SIDs like S-1-5-18).
            regex: r"\bS-1-5-21-\d+-\d+-\d+-\d+\b".into(),
            replacement: "<SID>".into(),
        },
    ]
}

// ─── Redactor ────────────────────────────────────────────────────────────────

/// Compiled PII-redaction engine.
///
/// Construct once via [`Redactor::from_config`] (or [`Redactor::noop`] when
/// redaction is disabled) and reuse across many [`Redactor::apply`] calls.
/// Regex compilation is amortized — only the replacement step pays per call.
pub struct Redactor {
    rules: Vec<CompiledRule>,
}

impl std::fmt::Debug for Redactor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Redactor")
            .field("rule_count", &self.rules.len())
            .finish()
    }
}

impl Redactor {
    /// Build a no-op redactor that returns its input unchanged.
    /// Used when `config.redaction.enabled = false`.
    pub fn noop() -> Self {
        Self { rules: Vec::new() }
    }

    /// Build a [`Redactor`] from the agent config.
    ///
    /// * If `config.redaction.enabled` is `false`, returns [`Redactor::noop`].
    /// * Otherwise compiles the built-in defaults followed by any
    ///   operator-supplied rules from `config.redaction.patterns`.
    ///
    /// # Errors
    ///
    /// Returns [`RedactorError::InvalidRegex`] if any rule's `regex` field
    /// fails to compile. Callers should treat this as a fatal startup error
    /// so a misconfigured rule doesn't silently leave PII unredacted.
    pub fn from_config(config: &AgentConfig) -> Result<Self, RedactorError> {
        if !config.redaction.enabled {
            return Ok(Self::noop());
        }

        let all_rules: Vec<RedactionRule> = default_rules()
            .into_iter()
            .chain(config.redaction.patterns.iter().cloned())
            .collect();

        Self::from_rules(&all_rules)
    }

    /// Compile an arbitrary slice of rules. Useful in tests and the
    /// operator preview tool.
    pub fn from_rules(rules: &[RedactionRule]) -> Result<Self, RedactorError> {
        let compiled = rules
            .iter()
            .map(|rule| {
                Regex::new(&rule.regex)
                    .map_err(|source| RedactorError::InvalidRegex {
                        name: rule.name.clone(),
                        source,
                    })
                    .map(|pattern| CompiledRule {
                        name: rule.name.clone(),
                        pattern,
                        replacement: rule.replacement.clone(),
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(Self { rules: compiled })
    }

    /// Apply all rules to `input`.
    ///
    /// Returns a [`Cow::Borrowed`] reference to the original string when no
    /// rule matches — no heap allocation on the fast path. Returns
    /// [`Cow::Owned`] with the substituted content on the first match.
    pub fn apply<'a>(&self, input: &'a str) -> Cow<'a, str> {
        if self.rules.is_empty() {
            return Cow::Borrowed(input);
        }

        // `result` starts as `None`; the first rule that actually fires
        // causes an allocation and subsequent rules operate on the owned
        // copy. Rules that don't match are a no-op (no extra allocation).
        let mut result: Option<String> = None;

        for rule in &self.rules {
            let src: &str = result.as_deref().unwrap_or(input);
            let replaced = rule.pattern.replace_all(src, rule.replacement.as_str());
            if let Cow::Owned(s) = replaced {
                result = Some(s);
            }
            // Cow::Borrowed means no match — src is unchanged, keep going.
        }

        match result {
            Some(s) => Cow::Owned(s),
            None => Cow::Borrowed(input),
        }
    }

    /// Returns `true` when the redactor has no rules (i.e. was constructed
    /// via [`Redactor::noop`] or with an empty rule list).
    pub fn is_noop(&self) -> bool {
        self.rules.is_empty()
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AgentConfig, RedactionConfig};

    fn redactor_with_defaults() -> Redactor {
        let cfg = AgentConfig::default(); // enabled = true, no extra patterns
        Redactor::from_config(&cfg).expect("compile default rules")
    }

    #[test]
    fn noop_returns_borrowed() {
        let r = Redactor::noop();
        let result = r.apply("hello world");
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(result, "hello world");
    }

    #[test]
    fn no_match_returns_borrowed() {
        let r = redactor_with_defaults();
        let input = "nothing sensitive here";
        let result = r.apply(input);
        assert!(matches!(result, Cow::Borrowed(_)));
    }

    #[test]
    fn username_path_is_redacted() {
        let r = redactor_with_defaults();
        let input = r"C:\Users\johndoe\AppData\Local\Temp\setup.log";
        let result = r.apply(input);
        assert_eq!(result, r"C:\Users\<USER>\AppData\Local\Temp\setup.log");
        assert!(matches!(result, Cow::Owned(_)));
    }

    #[test]
    fn guid_is_redacted() {
        let r = redactor_with_defaults();
        let input = "Device id: 550e8400-e29b-41d4-a716-446655440000 status ok";
        let result = r.apply(input);
        assert_eq!(result, "Device id: <GUID> status ok");
    }

    #[test]
    fn email_is_redacted() {
        let r = redactor_with_defaults();
        let input = "User alice@corp.example.com authenticated";
        let result = r.apply(input);
        assert_eq!(result, "User <EMAIL> authenticated");
    }

    #[test]
    fn ipv4_internal_is_redacted() {
        let r = redactor_with_defaults();
        // Internal corp 10.x should be redacted
        let input = "Connected to 10.10.20.30 for update";
        let result = r.apply(input);
        assert_eq!(result, "Connected to <INTERNAL_IP> for update");
    }

    #[test]
    fn public_ip_is_preserved() {
        let r = redactor_with_defaults();
        // 8.8.8.8 is not a 10.x address — should be untouched
        let input = "DNS query to 8.8.8.8 succeeded";
        let result = r.apply(input);
        assert_eq!(result, "DNS query to 8.8.8.8 succeeded");
    }

    #[test]
    fn multiple_pii_types_in_one_line() {
        let r = redactor_with_defaults();
        let input = r"C:\Users\bob\log.txt contacted 10.0.0.1 from bob@example.com";
        let result = r.apply(input);
        assert!(result.contains("<USER>"), "username redacted: {result}");
        assert!(result.contains("<INTERNAL_IP>"), "ip redacted: {result}");
        assert!(result.contains("<EMAIL>"), "email redacted: {result}");
    }

    #[test]
    fn disabled_redaction_returns_raw() {
        let cfg = AgentConfig {
            redaction: RedactionConfig {
                enabled: false,
                patterns: Vec::new(),
            },
            ..AgentConfig::default()
        };
        let r = Redactor::from_config(&cfg).unwrap();
        assert!(r.is_noop());
        let input = "bob@example.com visited 10.0.0.1";
        let result = r.apply(input);
        assert_eq!(result, input);
    }

    #[test]
    fn custom_rule_appended_after_defaults() {
        let cfg = AgentConfig {
            redaction: RedactionConfig {
                enabled: true,
                patterns: vec![RedactionRule {
                    name: "hostname".into(),
                    regex: r"\bWIN-[A-Z0-9]+\b".into(),
                    replacement: "<HOSTNAME>".into(),
                }],
            },
            ..AgentConfig::default()
        };
        let r = Redactor::from_config(&cfg).unwrap();
        let input = "Machine WIN-ABC123 completed policy";
        let result = r.apply(input);
        assert_eq!(result, "Machine <HOSTNAME> completed policy");
    }

    #[test]
    fn invalid_regex_returns_error() {
        let rules = vec![RedactionRule {
            name: "bad".into(),
            regex: "[invalid".into(),
            replacement: "X".into(),
        }];
        let err = Redactor::from_rules(&rules).unwrap_err();
        assert!(err.to_string().contains("bad"));
    }

    /// `\b` anchors must prevent the GUID rule from chewing on longer hex
    /// tokens (e.g. a 64-char SHA-256 with a GUID-shaped substring).
    #[test]
    fn guid_anchors_protect_longer_hex_tokens() {
        let r = redactor_with_defaults();
        // `aabbccdd-eeff-0011-2233-445566778899` is a valid GUID. Embed
        // it inside a longer hex run with no separators on either side.
        let input = "abcdef0aabbccdd-eeff-0011-2233-445566778899abcdef";
        let result = r.apply(input);
        // Should NOT have replaced the middle (no word-boundary either side).
        assert_eq!(result, input, "GUID inside longer hex must NOT be redacted: {result}");
    }

    /// Standalone GUID still gets redacted.
    #[test]
    fn standalone_guid_still_redacted() {
        let r = redactor_with_defaults();
        let input = "id=aabbccdd-eeff-0011-2233-445566778899 status=ok";
        let result = r.apply(input);
        assert!(result.contains("<GUID>"), "standalone GUID should be redacted: {result}");
    }

    /// 192.168.0.0/16 must redact.
    #[test]
    fn ipv4_192_168_is_redacted() {
        let r = redactor_with_defaults();
        let input = "Reaching gateway 192.168.1.1 for sync";
        let result = r.apply(input);
        assert_eq!(result, "Reaching gateway <INTERNAL_IP> for sync");
    }

    /// 172.16.0.0/12 must redact (172.16-172.31 second-octet).
    #[test]
    fn ipv4_172_16_is_redacted() {
        let r = redactor_with_defaults();
        for (input, expected) in &[
            ("From 172.16.0.5 to host", "From <INTERNAL_IP> to host"),
            ("From 172.20.10.1 to host", "From <INTERNAL_IP> to host"),
            ("From 172.31.255.254 to host", "From <INTERNAL_IP> to host"),
        ] {
            let result = r.apply(input);
            assert_eq!(&result, expected);
        }
    }

    /// 172.x.x.x outside the /12 must NOT be redacted.
    #[test]
    fn ipv4_172_outside_12_preserved() {
        let r = redactor_with_defaults();
        for input in &[
            "Reaching public 172.15.0.1 host",
            "Reaching public 172.32.0.1 host",
        ] {
            let result = r.apply(input);
            assert_eq!(&result, input, "non-RFC-1918 172.x must be preserved");
        }
    }

    /// SID redaction (Windows domain principals).
    #[test]
    fn windows_sid_is_redacted() {
        let r = redactor_with_defaults();
        let input = "User S-1-5-21-1004336348-1177238915-682003330-1013 logged on";
        let result = r.apply(input);
        assert_eq!(result, "User <SID> logged on");
    }

    /// Well-known SIDs (LocalSystem, etc.) are NOT touched by the domain-SID
    /// rule because they don't have the `21-` sub-authority prefix.
    #[test]
    fn well_known_sid_preserved() {
        let r = redactor_with_defaults();
        let input = "Service running as S-1-5-18";
        let result = r.apply(input);
        assert_eq!(result, input);
    }
}
