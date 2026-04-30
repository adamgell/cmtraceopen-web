//! Parses the `selector_json` blob and generates a parameter-bound SQL
//! WHERE clause. Allow-list of fields and operators; unknown values
//! return an error before any SQL runs.

use serde_json::Value;
use sqlx::postgres::PgArguments;
use sqlx::Arguments;

#[derive(Debug, thiserror::Error)]
pub enum SelectorError {
    #[error("unknown field: {0}")]
    UnknownField(String),
    #[error("invalid operator value for {field}: {got}")]
    InvalidOperator { field: String, got: String },
    #[error("malformed selector: {0}")]
    Malformed(String),
}

/// Build the `WHERE` clause body (no leading `WHERE`) and the matching
/// PgArguments. Caller composes the final query.
pub fn render(selector: &Value) -> Result<(String, PgArguments), SelectorError> {
    let obj = selector.as_object().ok_or_else(||
        SelectorError::Malformed("expected object".into()))?;
    let mut where_parts: Vec<String> = Vec::new();
    let mut args = PgArguments::default();
    let mut pidx: i32 = 0;

    for (field, val) in obj {
        match field.as_str() {
            "device_id" | "device_name" | "intune_device_id"
            | "ninjaone_device_id" | "asset_tag" | "agent_version" => {
                let frag = render_string_field(field, val, &mut args, &mut pidx)?;
                where_parts.push(frag);
            }
            "os_version" => {
                let frag = render_prefix_field("os_version", val, &mut args, &mut pidx)?;
                where_parts.push(frag);
            }
            "last_seen_within" => {
                let dur_text = val.as_str().ok_or_else(||
                    SelectorError::InvalidOperator {
                        field: field.clone(),
                        got: val.to_string(),
                    })?;
                pidx += 1;
                let _ = args.add(dur_text.to_string());
                where_parts.push(format!(
                    "last_seen_at >= now() - ${pidx}::interval"
                ));
            }
            other => return Err(SelectorError::UnknownField(other.to_string())),
        }
    }

    let where_clause = if where_parts.is_empty() {
        "TRUE".to_string()
    } else {
        where_parts.join(" AND ")
    };
    Ok((where_clause, args))
}

fn render_string_field(
    field: &str, val: &Value, args: &mut PgArguments, pidx: &mut i32,
) -> Result<String, SelectorError> {
    if let Some(s) = val.as_str() {
        if s == "is_set" {
            return Ok(format!("{field} IS NOT NULL"));
        }
        if s == "is_null" {
            return Ok(format!("{field} IS NULL"));
        }
        if let Some(rest) = s.strip_prefix("prefix:") {
            *pidx += 1;
            let _ = args.add(format!("{}%", rest));
            return Ok(format!("{field} LIKE ${pidx}"));
        }
        *pidx += 1;
        let _ = args.add(s.to_string());
        return Ok(format!("{field} = ${pidx}"));
    }
    if let Some(arr) = val.as_array() {
        let mut placeholders = Vec::new();
        for v in arr {
            let s = v.as_str().ok_or_else(||
                SelectorError::InvalidOperator {
                    field: field.to_string(),
                    got: v.to_string(),
                })?;
            *pidx += 1;
            let _ = args.add(s.to_string());
            placeholders.push(format!("${pidx}"));
        }
        if placeholders.is_empty() {
            return Ok("FALSE".into());
        }
        return Ok(format!("{field} IN ({})", placeholders.join(",")));
    }
    Err(SelectorError::InvalidOperator {
        field: field.to_string(),
        got: val.to_string(),
    })
}

fn render_prefix_field(
    field: &str, val: &Value, args: &mut PgArguments, pidx: &mut i32,
) -> Result<String, SelectorError> {
    let s = val.as_str().ok_or_else(||
        SelectorError::InvalidOperator {
            field: field.to_string(),
            got: val.to_string(),
        })?;
    *pidx += 1;
    let _ = args.add(format!("{s}%"));
    Ok(format!("{field} LIKE ${pidx}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_selector_is_true() {
        let (w, _) = render(&json!({})).unwrap();
        assert_eq!(w, "TRUE");
    }

    #[test]
    fn exact_match_generates_param_binding() {
        let (w, _) = render(&json!({"device_id": "GELL-2C3BD243"})).unwrap();
        assert_eq!(w, "device_id = $1");
    }

    #[test]
    fn prefix_uses_like() {
        let (w, _) = render(&json!({"device_name": "prefix:LAB-"})).unwrap();
        assert_eq!(w, "device_name LIKE $1");
    }

    #[test]
    fn is_set_no_param() {
        let (w, _) = render(&json!({"intune_device_id": "is_set"})).unwrap();
        assert_eq!(w, "intune_device_id IS NOT NULL");
    }

    #[test]
    fn list_uses_in() {
        let (w, _) = render(&json!({"asset_tag": ["A", "B", "C"]})).unwrap();
        assert_eq!(w, "asset_tag IN ($1,$2,$3)");
    }

    #[test]
    fn unknown_field_rejected() {
        let err = render(&json!({"hostname": "GELL"})).unwrap_err();
        assert!(matches!(err, SelectorError::UnknownField(_)));
    }

    #[test]
    fn os_version_is_prefix_only() {
        let (w, _) = render(&json!({"os_version": "Windows 10"})).unwrap();
        assert_eq!(w, "os_version LIKE $1");
    }

    #[test]
    fn last_seen_within_uses_interval_cast() {
        let (w, _) = render(&json!({"last_seen_within": "1h"})).unwrap();
        assert_eq!(w, "last_seen_at >= now() - $1::interval");
    }

    #[test]
    fn injection_in_value_is_parameter_bound_not_inlined() {
        let (w, _) = render(&json!({"device_id": "'; DROP TABLE agents; --"})).unwrap();
        assert_eq!(w, "device_id = $1");
        assert!(!w.contains("DROP"));
    }

    #[test]
    fn many_fields_combine_with_and() {
        let (w, _) = render(&json!({
            "intune_device_id": "is_set",
            "os_version": "Windows 10"
        })).unwrap();
        assert!(w.contains("intune_device_id IS NOT NULL"));
        assert!(w.contains("os_version LIKE"));
        assert!(w.contains(" AND "));
    }
}
