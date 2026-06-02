use serde::{Deserialize, Serialize};
use crate::hardware::CurvePoint;

/// Messages the HTML frontend can send to Rust via window.ipc.postMessage(...)
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum IpcMessage {
    /// Apply a fan curve to a specific fan
    SetCurve {
        fan_id: String,
        points: Vec<CurvePoint>,
    },
    /// Set a fixed speed (0–100 %)
    SetSpeed {
        fan_id: String,
        rpm_percent: u8,
    },
    /// Reset fan to default/automatic control
    ResetToDefault {
        fan_id: String,
    },
    /// Rescan all hardware
    Rescan,
    /// Reset every fan to automatic BIOS control
    ResetAll,
    /// Rename a fan
    RenameFan {
        fan_id: String,
        label: String,
    },
    /// Save a per-fan curve as part of the Custom profile
    SaveCustomProfile {
        fan_id: String,
        points: Vec<CurvePoint>,
    },
}
