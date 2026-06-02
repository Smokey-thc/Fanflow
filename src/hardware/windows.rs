use crate::hardware::{FanInfo, FanType, FanMode};
use crate::hardware::controller::FanBackend;

// ─── WMI Backend (system fans) ────────────────────────────────────────────────

pub struct WmiBackend;

impl WmiBackend {
    pub fn new() -> Self { Self }
}

impl FanBackend for WmiBackend {
    fn name(&self) -> &str { "wmi" }

    fn scan(&mut self) -> Vec<FanInfo> {
        // TODO: Query Win32_Fan via WMI
        // For now, returns placeholder - full WMI integration requires
        // unsafe COM initialization and is complex
        //
        // Example (needs wmi crate initialized):
        //   let wmi_con = WMIConnection::new(COMLibrary::new()?.into())?;
        //   let fans: Vec<Win32_Fan> = wmi_con.query()?;
        println!("[WMI] Fan scan - WMI integration placeholder");
        Vec::new()
    }

    fn set_speed_pct(&mut self, _fan_id: &str, _pct: u8) -> anyhow::Result<()> {
        // WMI doesn't directly support PWM control on Windows
        // This would need vendor-specific tools (e.g. LibreHardwareMonitor)
        anyhow::bail!("WMI fan control not yet implemented")
    }

    fn reset_to_auto(&mut self, _fan_id: &str) -> anyhow::Result<()> {
        anyhow::bail!("WMI fan control not yet implemented")
    }
}

// ─── NVIDIA (Windows) ─────────────────────────────────────────────────────────

pub struct NvidiaBackend {
    available: bool,
}

impl NvidiaBackend {
    pub fn new() -> Self {
        let available = std::process::Command::new("nvidia-smi.exe")
            .arg("--query-gpu=name")
            .arg("--format=csv,noheader")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        Self { available }
    }
}

impl FanBackend for NvidiaBackend {
    fn name(&self) -> &str { "nvidia_win" }

    fn scan(&mut self) -> Vec<FanInfo> {
        if !self.available { return Vec::new(); }

        let out = std::process::Command::new("nvidia-smi")
            .args([
                "--query-gpu=name,fan.speed,temperature.gpu",
                "--format=csv,noheader,nounits",
            ])
            .output()
            .ok();

        let mut fans = Vec::new();
        if let Some(out) = out {
            for (i, line) in String::from_utf8_lossy(&out.stdout).lines().enumerate() {
                let parts: Vec<&str> = line.split(',').map(|s| s.trim()).collect();
                if parts.len() == 3 {
                    fans.push(FanInfo {
                        id: format!("nvidia:{i}"),
                        label: format!("{} Fan", parts[0]),
                        fan_type: FanType::Nvidia,
                        mode: FanMode::Auto,
                        rpm: None,
                        speed_pct: parts[1].parse().ok(),
                        temp_c: parts[2].parse().ok(),
                        curve: Vec::new(),
                        rpm_min: None,
                        rpm_max: None,
                        controllable: true,
                        is_pump: false,
                    });
                }
            }
        }
        fans
    }

    fn set_speed_pct(&mut self, fan_id: &str, pct: u8) -> anyhow::Result<()> {
        if !fan_id.starts_with("nvidia:") { anyhow::bail!("not nvidia"); }
        let idx: u32 = fan_id.trim_start_matches("nvidia:").parse()?;

        // nvidia-smi on Windows supports fan speed control since driver 520+
        std::process::Command::new("nvidia-smi")
            .args(["-i", &idx.to_string(), "--fan-speed-control=1"])
            .output()?;

        std::process::Command::new("nvidia-smi")
            .args(["-i", &idx.to_string(),
                   &format!("--fan-speed={pct}")])
            .output()?;

        Ok(())
    }

    fn reset_to_auto(&mut self, fan_id: &str) -> anyhow::Result<()> {
        if !fan_id.starts_with("nvidia:") { anyhow::bail!("not nvidia"); }
        let idx: u32 = fan_id.trim_start_matches("nvidia:").parse()?;

        std::process::Command::new("nvidia-smi")
            .args(["-i", &idx.to_string(), "--fan-speed-control=0"])
            .output()?;
        Ok(())
    }
}

// ─── AMD (Windows) ────────────────────────────────────────────────────────────

pub struct AmdBackend;

impl AmdBackend {
    pub fn new() -> Self { Self }
}

impl FanBackend for AmdBackend {
    fn name(&self) -> &str { "amd_win" }

    fn scan(&mut self) -> Vec<FanInfo> {
        // TODO: AMD on Windows requires ADL (AMD Display Library) SDK
        // or reading via LibreHardwareMonitor.
        // Placeholder for now.
        println!("[AMD Win] Fan scan placeholder - needs ADL SDK integration");
        Vec::new()
    }

    fn set_speed_pct(&mut self, _fan_id: &str, _pct: u8) -> anyhow::Result<()> {
        anyhow::bail!("AMD Windows fan control not yet implemented")
    }

    fn reset_to_auto(&mut self, _fan_id: &str) -> anyhow::Result<()> {
        anyhow::bail!("AMD Windows fan control not yet implemented")
    }
}
