use serde::{Deserialize, Serialize};

/// Type of fan / controller
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FanType {
    /// Standard PWM / DC fan via motherboard header (hwmon / WMI)
    System,
    /// NVIDIA GPU fan (via NVML)
    Nvidia,
    /// AMD GPU fan (via ROCm / amdgpu sysfs)
    Amd,
}

/// How the fan is currently controlled
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum FanMode {
    /// OS / driver controls speed automatically
    Auto,
    /// User-defined temperature → speed curve
    Curve,
    /// Fixed speed percentage
    Fixed,
}

/// A single point on a fan curve
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurvePoint {
    /// Temperature in °C
    pub temp_c: u8,
    /// Fan speed in % (0–100)
    pub speed_pct: u8,
}

/// Full info about one fan
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FanInfo {
    /// Unique identifier (e.g. "hwmon2/fan1" or "nvidia:0")
    pub id: String,
    /// Human-readable label
    pub label: String,
    /// Fan type
    pub fan_type: FanType,
    /// Current mode
    pub mode: FanMode,
    /// Current RPM (if readable)
    pub rpm: Option<u32>,
    /// Current speed percentage (if readable)
    pub speed_pct: Option<u8>,
    /// Current temperature source in °C
    pub temp_c: Option<f32>,
    /// Active curve points (if mode == Curve)
    pub curve: Vec<CurvePoint>,
    /// Min/max RPM reported by hardware
    pub rpm_min: Option<u32>,
    pub rpm_max: Option<u32>,
    /// Whether this fan supports PWM control
    pub controllable: bool,
    /// True when this is a pump header (fixed speed / water cooling pump)
    pub is_pump: bool,
}
