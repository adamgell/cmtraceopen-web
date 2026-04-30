//! Handshake-time auth + per-frame device_id verification. v1 uses the
//! `X-Device-Id` header (same model as ingest); mTLS upgrade is a
//! separate workstream.

use axum::http::HeaderMap;
use common_wire::ws::AgentFrame;

#[derive(Debug, thiserror::Error)]
pub enum WsAuthError {
    #[error("missing X-Device-Id header")]
    MissingDeviceId,
    #[error("X-Device-Id is not valid UTF-8 / too long")]
    BadDeviceId,
    #[error("frame device_id {got} does not match auth identity {expected}")]
    DeviceIdMismatch { expected: String, got: String },
}

/// Pulls `X-Device-Id` from the upgrade request. Length-capped at 128
/// chars (way more than the agent's <hostname>-<hex> format).
pub fn extract_device_id(headers: &HeaderMap) -> Result<String, WsAuthError> {
    let v = headers
        .get("x-device-id")
        .ok_or(WsAuthError::MissingDeviceId)?
        .to_str()
        .map_err(|_| WsAuthError::BadDeviceId)?;
    if v.is_empty() || v.len() > 128 {
        return Err(WsAuthError::BadDeviceId);
    }
    Ok(v.to_string())
}

/// Every frame from the agent must carry `device_id` matching the auth.
/// HeartbeatAck doesn't apply (server → agent).
pub fn verify_frame_device_id(
    expected: &str,
    frame: &AgentFrame,
) -> Result<(), WsAuthError> {
    let got = match frame {
        AgentFrame::Heartbeat(h) => &h.device_id,
        AgentFrame::RequestAck { device_id, .. } => device_id,
        AgentFrame::RequestComplete { device_id, .. } => device_id,
    };
    if got != expected {
        return Err(WsAuthError::DeviceIdMismatch {
            expected: expected.to_string(),
            got: got.clone(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use common_wire::ws::Heartbeat;
    use uuid::Uuid;

    fn hb(device_id: &str) -> AgentFrame {
        AgentFrame::Heartbeat(Heartbeat {
            device_id: device_id.into(),
            device_name: "n".into(),
            intune_device_id: None,
            ninjaone_device_id: None,
            asset_tag: None,
            ts: Utc::now(),
            agent_version: "0.2.0".into(),
            os_version: "win".into(),
            last_collect_at: None,
            queue_depth: 0,
            errors_24h: 0,
            disk_free_pct: 0,
            uptime_seconds: 0,
        })
    }

    #[test]
    fn missing_header_rejected() {
        let h = HeaderMap::new();
        assert!(matches!(extract_device_id(&h), Err(WsAuthError::MissingDeviceId)));
    }

    #[test]
    fn empty_or_too_long_rejected() {
        let mut h = HeaderMap::new();
        h.insert("x-device-id", "".parse().unwrap());
        assert!(matches!(extract_device_id(&h), Err(WsAuthError::BadDeviceId)));
        h.insert("x-device-id", "a".repeat(129).parse().unwrap());
        assert!(matches!(extract_device_id(&h), Err(WsAuthError::BadDeviceId)));
    }

    #[test]
    fn valid_header_extracted() {
        let mut h = HeaderMap::new();
        h.insert("x-device-id", "GELL-2C3BD243".parse().unwrap());
        assert_eq!(extract_device_id(&h).unwrap(), "GELL-2C3BD243");
    }

    #[test]
    fn matching_frame_passes() {
        let frame = hb("dev-1");
        verify_frame_device_id("dev-1", &frame).unwrap();
    }

    #[test]
    fn mismatched_frame_rejected() {
        let frame = hb("attacker-id");
        let err = verify_frame_device_id("dev-1", &frame).unwrap_err();
        assert!(matches!(err, WsAuthError::DeviceIdMismatch { .. }));
    }

    #[test]
    fn request_ack_device_id_checked() {
        let f = AgentFrame::RequestAck {
            device_id: "wrong".into(),
            device_name: "n".into(),
            request_id: Uuid::nil(),
            accepted: true,
        };
        assert!(verify_frame_device_id("dev-1", &f).is_err());
    }
}
