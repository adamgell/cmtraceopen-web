//! WebSocket frame schemas for the heartbeat + on-demand-bundle protocol.
//! Owned by `common-wire` so agent and server share the types.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Server → Agent
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerFrame {
    RequestBundle {
        request_id: Uuid,
        reason: BundleReason,
        #[serde(skip_serializing_if = "Option::is_none")]
        schedule_name: Option<String>,
    },
    HeartbeatAck {
        ts: chrono::DateTime<chrono::Utc>,
    },
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BundleReason {
    Operator,
    Scheduled,
}

/// Agent → Server
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentFrame {
    Heartbeat(Heartbeat),
    RequestAck {
        device_id: String,
        device_name: String,
        request_id: Uuid,
        accepted: bool,
    },
    RequestComplete {
        device_id: String,
        device_name: String,
        request_id: Uuid,
        bundle_id: Uuid,
        outcome: RequestOutcome,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Heartbeat {
    pub device_id: String,
    pub device_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub intune_device_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ninjaone_device_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset_tag: Option<String>,
    pub ts: chrono::DateTime<chrono::Utc>,
    pub agent_version: String,
    pub os_version: String,
    pub last_collect_at: Option<chrono::DateTime<chrono::Utc>>,
    pub queue_depth: i32,
    pub errors_24h: i32,
    pub disk_free_pct: i32,
    pub uptime_seconds: i64,
}

#[derive(Copy, Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RequestOutcome {
    Ok,
    Error,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heartbeat_round_trips() {
        let hb = Heartbeat {
            device_id: "GELL-2C3BD243".into(),
            device_name: "GELL-LAPTOP-01".into(),
            intune_device_id: None,
            ninjaone_device_id: None,
            asset_tag: None,
            ts: chrono::Utc::now(),
            agent_version: "0.2.0".into(),
            os_version: "Windows 10.0.22631".into(),
            last_collect_at: None,
            queue_depth: 0,
            errors_24h: 42,
            disk_free_pct: 71,
            uptime_seconds: 189342,
        };
        let frame = AgentFrame::Heartbeat(hb.clone());
        let json = serde_json::to_string(&frame).unwrap();
        assert!(json.contains("\"type\":\"heartbeat\""));
        assert!(json.contains("\"device_id\":\"GELL-2C3BD243\""));
        let back: AgentFrame = serde_json::from_str(&json).unwrap();
        assert_eq!(back, AgentFrame::Heartbeat(hb));
    }

    #[test]
    fn request_bundle_with_optional_schedule() {
        let f = ServerFrame::RequestBundle {
            request_id: Uuid::nil(),
            reason: BundleReason::Operator,
            schedule_name: None,
        };
        let json = serde_json::to_string(&f).unwrap();
        assert!(!json.contains("schedule_name"));
        let f2: ServerFrame = serde_json::from_str(&json).unwrap();
        assert_eq!(f, f2);
    }

    #[test]
    fn request_complete_serializes_outcome() {
        let f = AgentFrame::RequestComplete {
            device_id: "d".into(),
            device_name: "n".into(),
            request_id: Uuid::nil(),
            bundle_id: Uuid::nil(),
            outcome: RequestOutcome::Ok,
            error: None,
        };
        let json = serde_json::to_string(&f).unwrap();
        assert!(json.contains("\"outcome\":\"ok\""));
        assert!(!json.contains("\"error\""));
    }
}
