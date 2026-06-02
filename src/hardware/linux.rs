use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// Write `value` to a sysfs file.
/// Falls back to `sudo -n tee` when running as a normal user
/// (credentials must be pre-cached with `sudo -v`).
fn write_sysfs(path: &Path, value: &str) -> anyhow::Result<()> {
    if fs::write(path, value).is_ok() {
        return Ok(());
    }
    let mut child = std::process::Command::new("sudo")
        .args(["-n", "tee", &path.to_string_lossy().to_string()])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(value.as_bytes())?;
    }
    let out = child.wait_with_output()?;
    if !out.status.success() {
        anyhow::bail!(
            "sudo tee failed for {}: {}",
            path.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}
use crate::hardware::{FanInfo, FanType, FanMode};
use crate::hardware::controller::FanBackend;

// ─── hwmon (system fans + some GPUs) ──────────────────────────────────────────

pub struct HwmonBackend {
    /// Stores the original pwm_enable value per fan_id so reset restores
    /// the exact BIOS mode (e.g. 5 = SMART FAN IV) instead of a hardcoded fallback.
    original_enables: std::collections::HashMap<String, u32>,
}

impl HwmonBackend {
    pub fn new() -> Self { Self { original_enables: std::collections::HashMap::new() } }

    fn hwmon_paths() -> Vec<PathBuf> {
        glob::glob("/sys/class/hwmon/hwmon*")
            .unwrap()
            .filter_map(|e| e.ok())
            .collect()
    }

    fn read_str(path: &Path) -> Option<String> {
        fs::read_to_string(path).ok().map(|s| s.trim().to_string())
    }

    fn read_u32(path: &Path) -> Option<u32> {
        Self::read_str(path)?.parse().ok()
    }
}

impl FanBackend for HwmonBackend {
    fn name(&self) -> &str { "hwmon" }

    fn scan(&mut self) -> Vec<FanInfo> {
        let mut fans = Vec::new();

        for hwmon in Self::hwmon_paths() {
            let chip_name = Self::read_str(&hwmon.join("name"))
                .unwrap_or_else(|| "unknown".into());

            let mut chip_fans: Vec<FanInfo> = Vec::new();

            for i in 1..=10u32 {
                let input_path = hwmon.join(format!("fan{i}_input"));
                if !input_path.exists() { continue; } // use continue, not break

                let rpm = Self::read_u32(&input_path);

                let pwm_path = hwmon.join(format!("pwm{i}"));
                let enable_path = hwmon.join(format!("pwm{i}_enable"));
                let pwm_exists = pwm_path.exists();
                let pwm_val = if pwm_exists { Self::read_u32(&pwm_path) } else { None };
                let enable_val = Self::read_u32(&enable_path);

                // Skip completely empty headers: no RPM reading and no PWM control
                if rpm == Some(0) && !pwm_exists { continue; }

                let controllable = pwm_exists;
                let speed_pct = pwm_val.map(|v| ((v as f32 / 255.0) * 100.0) as u8);

                // Pump: has PWM running (> 0) but reports 0 RPM — typical for water pumps
                // that lack a tachometer wire or have it disconnected.
                // Pump detection by label only here; RPM-ratio heuristic runs
                // after the full chip scan so we have the median available.
                let label_str = Self::read_str(&hwmon.join(format!("fan{i}_label")))
                    .unwrap_or_default().to_ascii_lowercase();
                let is_pump = label_str.contains("pump") || label_str.contains("w_pump");

                // Read which temperature sensor controls this fan via pwm_temp_sel.
                // Map the selector index to the matching temp{n}_input file.
                let temp_c = Self::read_u32(&hwmon.join(format!("pwm{i}_temp_sel")))
                    .filter(|&sel| sel > 0)
                    .and_then(|sel| {
                        let raw = Self::read_u32(&hwmon.join(format!("temp{sel}_input")))?;
                        let celsius = raw as f32 / 1000.0;
                        // Sanity check: ignore bogus readings
                        if celsius > -40.0 && celsius < 120.0 { Some(celsius) } else { None }
                    });

                let label = Self::read_str(&hwmon.join(format!("fan{i}_label")))
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| if is_pump {
                        format!("Pump {i}")
                    } else {
                        format!("Fan {i}")
                    });

                let id = format!("hwmon:{}/{}/fan{i}",
                    hwmon.file_name().unwrap_or_default().to_string_lossy(),
                    chip_name);

                // Only store once — subsequent scans may read enable=1 (manual)
                // after a user change, which would corrupt the stored BIOS value.
                if let Some(ev) = enable_val {
                    self.original_enables.entry(id.clone()).or_insert(ev);
                }

                chip_fans.push(FanInfo {
                    id,
                    label,
                    fan_type: FanType::System,
                    mode: FanMode::Auto,
                    rpm,
                    speed_pct,
                    temp_c,
                    curve: Vec::new(),
                    rpm_min: Self::read_u32(&hwmon.join(format!("fan{i}_min"))),
                    rpm_max: Self::read_u32(&hwmon.join(format!("fan{i}_max"))),
                    controllable,
                    is_pump,
                });
            }

            // Pump heuristic: a fan spinning >3× the median RPM of its peers
            // is almost certainly an AIO/water pump on a CPU_FAN header.
            let rpms: Vec<u32> = chip_fans.iter()
                .filter(|f| !f.is_pump && f.rpm.map_or(false, |r| r > 0))
                .filter_map(|f| f.rpm)
                .collect();
            if !rpms.is_empty() {
                let mut sorted = rpms.clone();
                sorted.sort_unstable();
                let median = sorted[sorted.len() / 2];
                for f in &mut chip_fans {
                    if !f.is_pump && f.rpm.map_or(false, |r| r > median * 3) {
                        f.is_pump = true;
                        f.label = "Pump".into();
                    }
                }
            }

            fans.extend(chip_fans);
        }
        fans
    }

    fn set_speed_pct(&mut self, fan_id: &str, pct: u8) -> anyhow::Result<()> {
        // Parse hwmon path from id "hwmon:hwmon2/it8689/fan1"
        let parts: Vec<&str> = fan_id.splitn(3, '/').collect();
        if parts.len() < 3 { anyhow::bail!("invalid hwmon fan_id"); }

        let hwmon_dev = parts[0].trim_start_matches("hwmon:");
        let fan_num = parts[2].trim_start_matches("fan");
        let base = PathBuf::from(format!("/sys/class/hwmon/{hwmon_dev}"));

        let enable_path = base.join(format!("pwm{fan_num}_enable"));
        let pwm_path = base.join(format!("pwm{fan_num}"));
        // Enforce minimum 20% — setting a CPU/system fan to 0 risks overheating.
        let pct = pct.max(20);
        let pwm_val = (pct as u32 * 255 / 100).min(255);
        // enable=1 (manual) MUST come before writing the pwm value.
        // In Smart Fan IV (enable=5) the kernel rejects direct pwm writes.
        if enable_path.exists() {
            write_sysfs(&enable_path, "1\n")?;
        }
        write_sysfs(&pwm_path, &format!("{pwm_val}\n"))?;
        Ok(())
    }

    fn reset_to_auto(&mut self, fan_id: &str) -> anyhow::Result<()> {
        let parts: Vec<&str> = fan_id.splitn(3, '/').collect();
        if parts.len() < 3 { anyhow::bail!("invalid hwmon fan_id"); }
        let hwmon_dev = parts[0].trim_start_matches("hwmon:");
        let fan_num = parts[2].trim_start_matches("fan");
        let base = PathBuf::from(format!("/sys/class/hwmon/{hwmon_dev}"));
        let enable_path = base.join(format!("pwm{fan_num}_enable"));

        if enable_path.exists() {
            let original = self.original_enables.get(fan_id).copied()
                .unwrap_or(5); // 5 = SMART FAN IV (nct6798 BIOS default)
            // If the stored value is 1 (manual) the app was last quit while a fan
            // was under manual control — fall back to SMART FAN IV.
            let restore = if original == 1 { 5 } else { original };
            write_sysfs(&enable_path, &format!("{restore}\n"))?;
        }
        Ok(())
    }
}

// ─── NVIDIA via NVML ─────────────────────────────────────────────────────────
//
// nvidia-settings + Coolbits is architecturally dead on Wayland/XWayland (the
// NVIDIA Xorg DDX driver never loads), so we use NVML instead. Reads work as a
// normal user; *setting* a fan speed needs root, so set/reset re-exec this same
// binary through `sudo -n` (see `gpu_nvml::run_cli` and `setup_permissions`).

pub struct NvidiaBackend {
    available: bool,
}

impl NvidiaBackend {
    pub fn new() -> Self {
        // Available if NVML can enumerate at least one device.
        let available = !crate::gpu_nvml::read_all().is_empty();
        Self { available }
    }

    /// Parse a "nvidia:{gpu}:{fan}" id into its two indices.
    fn parse_id(fan_id: &str) -> anyhow::Result<(u32, u32)> {
        if !fan_id.starts_with("nvidia:") { anyhow::bail!("not nvidia"); }
        let rest = fan_id.trim_start_matches("nvidia:");
        let mut parts = rest.splitn(2, ':');
        let gpu: u32 = parts.next().unwrap_or("0").parse().unwrap_or(0);
        let fan: u32 = parts.next().unwrap_or("0").parse().unwrap_or(0);
        Ok((gpu, fan))
    }

    /// Re-exec this binary as root to perform a privileged NVML operation.
    fn run_privileged(args: &[String]) -> anyhow::Result<()> {
        let exe = std::env::current_exe()?;
        // If we're already root (e.g. launched via sudo) call NVML directly.
        if crate::is_root() {
            match args.first().map(|s| s.as_str()) {
                Some("--gpu-set") => {
                    let g = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
                    let f = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
                    let p = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(50);
                    return crate::gpu_nvml::set_fan(g, f, p);
                }
                Some("--gpu-reset") => {
                    let g = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
                    return crate::gpu_nvml::reset_gpu(g);
                }
                _ => {}
            }
        }
        let out = std::process::Command::new("sudo")
            .arg("-n")
            .arg(&exe)
            .args(args)
            .output()
            .map_err(|e| anyhow::anyhow!("failed to run sudo: {e}"))?;
        if !out.status.success() {
            anyhow::bail!(
                "privileged NVML call failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(())
    }
}

impl FanBackend for NvidiaBackend {
    fn name(&self) -> &str { "nvidia" }

    fn scan(&mut self) -> Vec<FanInfo> {
        if !self.available { return Vec::new(); }

        crate::gpu_nvml::read_all()
            .into_iter()
            .map(|g| {
                let label = if g.fan_count == 1 {
                    format!("{} Fan", g.gpu_name)
                } else {
                    format!("{} Fan {}", g.gpu_name, g.fan_idx + 1)
                };
                FanInfo {
                    id: format!("nvidia:{}:{}", g.gpu_idx, g.fan_idx),
                    label,
                    fan_type: FanType::Nvidia,
                    mode: FanMode::Auto,
                    rpm: g.rpm,
                    speed_pct: g.speed_pct,
                    temp_c: g.temp_c,
                    curve: Vec::new(),
                    rpm_min: Some(0),
                    rpm_max: g.rpm.map(|r| r.max(3500)).or(Some(3500)),
                    controllable: true,
                    is_pump: false,
                }
            })
            .collect()
    }

    fn set_speed_pct(&mut self, fan_id: &str, pct: u8) -> anyhow::Result<()> {
        let (gpu, fan) = Self::parse_id(fan_id)?;
        // NVIDIA enforces a ~30% floor; clamp low values so the fan keeps spinning.
        let pct = pct.max(30);
        Self::run_privileged(&[
            "--gpu-set".into(),
            gpu.to_string(),
            fan.to_string(),
            pct.to_string(),
        ])
    }

    fn reset_to_auto(&mut self, fan_id: &str) -> anyhow::Result<()> {
        let (gpu, _fan) = Self::parse_id(fan_id)?;
        Self::run_privileged(&["--gpu-reset".into(), gpu.to_string()])
    }
}

// ─── AMD GPU via amdgpu sysfs ─────────────────────────────────────────────────

pub struct AmdGpuBackend;

impl AmdGpuBackend {
    pub fn new() -> Self { Self }

    fn amd_drm_paths() -> Vec<PathBuf> {
        glob::glob("/sys/class/drm/card*/device")
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|p| {
                // Check vendor ID = 0x1002 (AMD)
                let vendor = p.join("vendor");
                fs::read_to_string(&vendor)
                    .map(|v| v.trim() == "0x1002")
                    .unwrap_or(false)
            })
            .collect()
    }
}

impl FanBackend for AmdGpuBackend {
    fn name(&self) -> &str { "amdgpu" }

    fn scan(&mut self) -> Vec<FanInfo> {
        let mut fans = Vec::new();

        for (i, dev) in Self::amd_drm_paths().into_iter().enumerate() {
            let hwmon = glob::glob(&format!("{}/hwmon/hwmon*", dev.display()))
                .unwrap()
                .filter_map(|e| e.ok())
                .next();

            if let Some(hwmon) = hwmon {
                let rpm = fs::read_to_string(hwmon.join("fan1_input"))
                    .ok()
                    .and_then(|s| s.trim().parse().ok());

                let temp_c = fs::read_to_string(hwmon.join("temp1_input"))
                    .ok()
                    .and_then(|s| s.trim().parse::<u32>().ok())
                    .map(|mv| mv as f32 / 1000.0);

                let pwm_val = fs::read_to_string(hwmon.join("pwm1"))
                    .ok()
                    .and_then(|s| s.trim().parse::<u32>().ok());

                fans.push(FanInfo {
                    id: format!("amd:{i}"),
                    label: format!("AMD GPU {i} Fan"),
                    fan_type: FanType::Amd,
                    mode: FanMode::Auto,
                    rpm,
                    speed_pct: pwm_val.map(|v| ((v as f32 / 255.0) * 100.0) as u8),
                    temp_c,
                    curve: Vec::new(),
                    rpm_min: None,
                    rpm_max: None,
                    controllable: hwmon.join("pwm1").exists(),
                    is_pump: false,
                });
            }
        }
        fans
    }

    fn set_speed_pct(&mut self, fan_id: &str, pct: u8) -> anyhow::Result<()> {
        if !fan_id.starts_with("amd:") { anyhow::bail!("not amd"); }
        let idx: usize = fan_id.trim_start_matches("amd:").parse()?;

        let dev = Self::amd_drm_paths().into_iter().nth(idx)
            .ok_or_else(|| anyhow::anyhow!("AMD GPU {idx} not found"))?;

        let hwmon = glob::glob(&format!("{}/hwmon/hwmon*", dev.display()))
            .unwrap()
            .filter_map(|e| e.ok())
            .next()
            .ok_or_else(|| anyhow::anyhow!("no hwmon for AMD GPU {idx}"))?;

        write_sysfs(&hwmon.join("pwm1_enable"), "1\n")?;
        let pwm = (pct as u32 * 255 / 100).min(255);
        write_sysfs(&hwmon.join("pwm1"), &format!("{pwm}\n"))?;
        Ok(())
    }

    fn reset_to_auto(&mut self, fan_id: &str) -> anyhow::Result<()> {
        if !fan_id.starts_with("amd:") { anyhow::bail!("not amd"); }
        let idx: usize = fan_id.trim_start_matches("amd:").parse()?;

        let dev = Self::amd_drm_paths().into_iter().nth(idx)
            .ok_or_else(|| anyhow::anyhow!("AMD GPU {idx} not found"))?;

        let hwmon = glob::glob(&format!("{}/hwmon/hwmon*", dev.display()))
            .unwrap()
            .filter_map(|e| e.ok())
            .next()
            .ok_or_else(|| anyhow::anyhow!("no hwmon"))?;

        write_sysfs(&hwmon.join("pwm1_enable"), "2\n")?;
        Ok(())
    }
}
