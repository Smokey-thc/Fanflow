use std::collections::HashMap;
use crate::hardware::{FanInfo, CurvePoint, FanMode};

fn config_dir() -> std::path::PathBuf {
    let dir = std::env::var("HOME")
        .map(|h| std::path::PathBuf::from(h).join(".config").join("fancontroller"))
        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp/fancontroller"));
    std::fs::create_dir_all(&dir).ok();
    dir
}

fn config_path() -> std::path::PathBuf { config_dir().join("config.json") }
fn custom_profile_path() -> std::path::PathBuf { config_dir().join("custom_profile.json") }

/// Trait that every hardware backend must implement
pub trait FanBackend: Send + Sync {
    fn name(&self) -> &str;
    fn scan(&mut self) -> Vec<FanInfo>;
    fn set_speed_pct(&mut self, fan_id: &str, pct: u8) -> anyhow::Result<()>;
    fn reset_to_auto(&mut self, fan_id: &str) -> anyhow::Result<()>;
}

/// Manages all fan backends and aggregates fan data
pub struct FanController {
    backends: Vec<Box<dyn FanBackend>>,
    fans: HashMap<String, FanInfo>,
    curves: HashMap<String, Vec<CurvePoint>>,
    /// User-defined labels that override hardware-detected names
    custom_labels: HashMap<String, String>,
    /// Saved curves for the Custom profile (persisted to disk)
    custom_profile: HashMap<String, Vec<CurvePoint>>,
}

impl FanController {
    pub fn new() -> Self {
        let mut backends: Vec<Box<dyn FanBackend>> = Vec::new();

        #[cfg(target_os = "linux")]
        {
            use crate::hardware::linux::{HwmonBackend, NvidiaBackend, AmdGpuBackend};
            backends.push(Box::new(HwmonBackend::new()));
            backends.push(Box::new(NvidiaBackend::new()));
            backends.push(Box::new(AmdGpuBackend::new()));
        }

        #[cfg(target_os = "windows")]
        {
            use crate::hardware::windows::{WmiBackend, NvidiaBackend, AmdBackend};
            backends.push(Box::new(WmiBackend::new()));
            backends.push(Box::new(NvidiaBackend::new()));
            backends.push(Box::new(AmdBackend::new()));
        }

        let custom_labels = std::fs::read_to_string(config_path())
            .ok()
            .and_then(|s| serde_json::from_str::<HashMap<String, String>>(&s).ok())
            .unwrap_or_default();

        let custom_profile = std::fs::read_to_string(custom_profile_path())
            .ok()
            .and_then(|s| serde_json::from_str::<HashMap<String, Vec<CurvePoint>>>(&s).ok())
            .unwrap_or_default();

        Self {
            backends,
            fans: HashMap::new(),
            curves: HashMap::new(),
            custom_labels,
            custom_profile,
        }
    }

    pub fn scan_all(&mut self) {
        self.fans.clear();
        for backend in &mut self.backends {
            for fan in backend.scan() {
                self.fans.insert(fan.id.clone(), fan);
            }
        }
    }

    pub fn get_all_fans(&self) -> Vec<FanInfo> {
        self.fans.values().map(|f| {
            let mut fan = f.clone();
            if let Some(label) = self.custom_labels.get(&f.id) {
                fan.label = label.clone();
            }
            fan
        }).collect()
    }

    /// Returns the saved Custom profile curves (all fans).
    pub fn get_custom_profile(&self) -> &HashMap<String, Vec<CurvePoint>> {
        &self.custom_profile
    }

    /// Save one fan's curve into the Custom profile and persist to disk.
    pub fn save_custom_curve(&mut self, fan_id: &str, points: Vec<CurvePoint>) {
        self.custom_profile.insert(fan_id.to_string(), points);
        let json = serde_json::to_string_pretty(&self.custom_profile).unwrap_or_default();
        std::fs::write(custom_profile_path(), json).ok();
    }

    pub fn rename_fan(&mut self, fan_id: &str, label: String) {
        self.custom_labels.insert(fan_id.to_string(), label);
        let json = serde_json::to_string_pretty(&self.custom_labels).unwrap_or_default();
        std::fs::write(config_path(), json).ok();
    }

    pub fn set_curve(&mut self, fan_id: &str, points: Vec<CurvePoint>) -> anyhow::Result<()> {
        if points.len() < 2 {
            anyhow::bail!("Curve needs at least 2 points");
        }
        for w in points.windows(2) {
            if w[0].temp_c >= w[1].temp_c {
                anyhow::bail!("Curve points must be sorted by temperature");
            }
        }
        self.curves.insert(fan_id.to_string(), points);
        if let Some(fan) = self.fans.get_mut(fan_id) {
            fan.mode = FanMode::Curve;
        }
        Ok(())
    }

    pub fn set_fixed_speed(&mut self, fan_id: &str, pct: u8) -> anyhow::Result<()> {
        let pct = pct.min(100);
        let mut last_err = String::new();
        for backend in &mut self.backends {
            match backend.set_speed_pct(fan_id, pct) {
                Ok(_) => {
                    if let Some(fan) = self.fans.get_mut(fan_id) {
                        fan.mode = FanMode::Fixed;
                        fan.speed_pct = Some(pct);
                    }
                    return Ok(());
                }
                Err(e) => { last_err = e.to_string(); }
            }
        }
        anyhow::bail!("{last_err}")
    }

    pub fn reset_to_default(&mut self, fan_id: &str) {
        for backend in &mut self.backends {
            let _ = backend.reset_to_auto(fan_id);
        }
        self.curves.remove(fan_id);
        if let Some(fan) = self.fans.get_mut(fan_id) {
            fan.mode = FanMode::Auto;
        }
    }

    pub fn tick(&mut self) {
        let curve_ids: Vec<String> = self.curves.keys().cloned().collect();
        for fan_id in curve_ids {
            let temp = self.fans.get(&fan_id).and_then(|f| f.temp_c);
            if let Some(temp) = temp {
                let points = self.curves[&fan_id].clone();
                let speed = interpolate_curve(&points, temp);
                for backend in &mut self.backends {
                    if backend.set_speed_pct(&fan_id, speed).is_ok() {
                        if let Some(fan) = self.fans.get_mut(&fan_id) {
                            fan.mode = FanMode::Curve;
                            fan.speed_pct = Some(speed);
                        }
                        break;
                    }
                }
            }
        }
    }
}

fn interpolate_curve(points: &[CurvePoint], temp: f32) -> u8 {
    if temp <= points[0].temp_c as f32 { return points[0].speed_pct; }
    if temp >= points[points.len() - 1].temp_c as f32 { return points[points.len() - 1].speed_pct; }
    for window in points.windows(2) {
        let t0 = window[0].temp_c as f32;
        let t1 = window[1].temp_c as f32;
        if temp >= t0 && temp <= t1 {
            let ratio = (temp - t0) / (t1 - t0);
            let s0 = window[0].speed_pct as f32;
            let s1 = window[1].speed_pct as f32;
            return (s0 + ratio * (s1 - s0)).round() as u8;
        }
    }
    points[points.len() - 1].speed_pct
}
