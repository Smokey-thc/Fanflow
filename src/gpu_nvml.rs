//! NVIDIA GPU fan control via NVML.
//!
//! NVML works on Wayland (unlike nvidia-settings + Coolbits, which require a
//! real Xorg server). Reads work as a normal user; *setting* the fan speed
//! requires root, so the set/reset operations are invoked through a privileged
//! re-exec of this same binary (see `run_cli` and the NvidiaBackend).

use nvml_wrapper::enum_wrappers::device::TemperatureSensor;
use nvml_wrapper::Nvml;

/// One readable GPU fan snapshot.
pub struct GpuFan {
    pub gpu_idx: u32,
    pub fan_idx: u32,
    pub gpu_name: String,
    pub fan_count: u32,
    pub speed_pct: Option<u8>,
    pub rpm: Option<u32>,
    pub temp_c: Option<f32>,
}

/// Read all GPU fans via NVML. Returns empty vec if NVML is unavailable.
pub fn read_all() -> Vec<GpuFan> {
    let nvml = match Nvml::init() {
        Ok(n) => n,
        Err(_) => return Vec::new(),
    };
    let count = nvml.device_count().unwrap_or(0);
    let mut out = Vec::new();

    for g in 0..count {
        let dev = match nvml.device_by_index(g) {
            Ok(d) => d,
            Err(_) => continue,
        };
        let name = dev.name().unwrap_or_else(|_| "NVIDIA GPU".into());
        let temp = dev.temperature(TemperatureSensor::Gpu).ok().map(|t| t as f32);
        let fan_count = dev.num_fans().unwrap_or(1).max(1);

        for f in 0..fan_count {
            out.push(GpuFan {
                gpu_idx: g,
                fan_idx: f,
                gpu_name: name.clone(),
                fan_count,
                speed_pct: dev.fan_speed(f).ok().map(|s| s.min(100) as u8),
                rpm: dev.fan_speed_rpm(f).ok(),
                temp_c: temp,
            });
        }
    }
    out
}

/// Set a single GPU fan to a fixed speed percentage (requires root).
pub fn set_fan(gpu_idx: u32, fan_idx: u32, pct: u8) -> anyhow::Result<()> {
    let nvml = Nvml::init()?;
    let mut dev = nvml.device_by_index(gpu_idx)?;
    // Enforce minimum 30% — setting GPU fan to 0 risks overheating.
    let pct = pct.clamp(30, 100);
    dev.set_fan_speed(fan_idx, pct as u32)?;
    Ok(())
}

/// Release all fans on a GPU back to automatic driver control (requires root).
pub fn reset_gpu(gpu_idx: u32) -> anyhow::Result<()> {
    let nvml = Nvml::init()?;
    let mut dev = nvml.device_by_index(gpu_idx)?;
    let fans = dev.num_fans().unwrap_or(1).max(1);
    for f in 0..fans {
        let _ = dev.set_default_fan_speed(f);
    }
    Ok(())
}

/// Handle privileged CLI subcommands. Returns true if a command was handled
/// (and the process should exit afterwards).
///
/// Usage (invoked via `sudo -n`):
///   fancontroller --gpu-set <gpu> <fan> <pct>
///   fancontroller --gpu-reset <gpu>
pub fn run_cli() -> Option<i32> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(|s| s.as_str()) {
        Some("--gpu-set") => {
            let gpu: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
            let fan: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);
            let pct: u8  = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(50);
            // Reject out-of-range indices — prevents misuse via the sudoers wildcard.
            if gpu > 7 || fan > 7 {
                eprintln!("gpu-set: invalid gpu/fan index (max 7)");
                return Some(1);
            }
            match set_fan(gpu, fan, pct) {
                Ok(()) => Some(0),
                Err(e) => { eprintln!("gpu-set failed: {e}"); Some(1) }
            }
        }
        Some("--gpu-reset") => {
            let gpu: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);
            if gpu > 7 {
                eprintln!("gpu-reset: invalid gpu index (max 7)");
                return Some(1);
            }
            match reset_gpu(gpu) {
                Ok(()) => Some(0),
                Err(e) => { eprintln!("gpu-reset failed: {e}"); Some(1) }
            }
        }
        _ => None,
    }
}
