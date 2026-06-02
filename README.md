# FanController

A lightweight fan manager for **Linux** (Wayland & X11) with a native HTML GUI.
Controls motherboard fans, pumps, and NVIDIA GPU fans via custom curves or fixed
speeds — no browser required, no cloud, everything runs locally.

![Platform](https://img.shields.io/badge/Platform-Linux-blue)
![Language](https://img.shields.io/badge/Rust-2021-orange)

## Features

- **Motherboard fans** via `hwmon` (e.g. nct6798, it8689 …)
- **NVIDIA GPU fans** via **NVML** — works on **Wayland/Hyprland**
  (unlike `nvidia-settings`/Coolbits which is dead on Wayland)
- **Pump detection** — AIO water cooling pumps are shown separately from fans
- Three modes per fan:
  - **Auto** — hand back to BIOS/driver control (Smart Fan IV etc.)
  - **Curve** — custom temperature → speed curve, applied every second
  - **Fixed** — fixed speed percentage (with live RPM display)
- **Reset All** instantly restores everything to BIOS control
- Temperature is only shown when the fan actually has a sensor
- Custom fan labels (saved to `~/.config/fancontroller/config.json`)
- One-time passwordless permission setup (sudoers) — no prompts after that

> **Windows:** A `.exe` is planned but not yet available. The GUI currently uses
> `webkit2gtk` which only builds on Linux. The Windows port (WebView2 via `wry`)
> is tracked in the [Roadmap](#roadmap).

## Quick Install (Linux)

```bash
curl -fsSL https://raw.githubusercontent.com/Smokey-thc/FanController/main/install.sh | bash
```

The script installs required system packages, builds FanController from source,
and places the binary at `/usr/local/bin/fancontroller`. Then just run:

```bash
fancontroller
```

> On **first launch** FanController will ask for your sudo password once to set
> up a `sudoers` rule. After that, fan control runs without any password prompts.

## Manual Build

**Dependencies:**

| Distro | Command |
|--------|---------|
| Arch | `sudo pacman -S --needed rust gtk3 webkit2gtk-4.1 nvidia-utils` |
| Debian/Ubuntu | `sudo apt install rustc cargo libgtk-3-dev libwebkit2gtk-4.1-dev` |
| Fedora | `sudo dnf install rust cargo gtk3-devel webkit2gtk4.1-devel` |

```bash
git clone https://github.com/Smokey-thc/FanController.git
cd FanController
cargo build --release
./target/release/fancontroller
```

The binary is self-contained — the entire GUI (`assets/index.html`) is compiled
in via `include_str!`. No external files need to be copied alongside it.

## How GPU Control Works

NVML puts the fan into **manual mode** which holds until explicitly released.
That is exactly what FanController does:

- **Curve / Fixed** → NVML holds the speed (driver cannot revert it)
- **Auto** → `set_default_fan_speed` hands the fan back to the driver

Since NVML write access requires root, FanController re-execs itself via
`sudo -n fancontroller --gpu-set/--gpu-reset` (the sudoers rule allows exactly
those two subcommands).

## Permissions

Setup writes `/etc/sudoers.d/fancontroller` with three NOPASSWD rules:

```
<user> ALL=(ALL) NOPASSWD: /usr/bin/tee /sys/class/hwmon/*
<user> ALL=(ALL) NOPASSWD: /usr/local/bin/fancontroller --gpu-set *
<user> ALL=(ALL) NOPASSWD: /usr/local/bin/fancontroller --gpu-reset *
```

To remove:

```bash
sudo rm /etc/sudoers.d/fancontroller
```

## Architecture

```
src/
├── main.rs            # GTK window + WebKit WebView, IPC, background thread
├── gpu_nvml.rs        # NVIDIA control via NVML (+ privileged CLI subcommands)
├── ipc.rs             # IPC message types (HTML ↔ Rust)
└── hardware/
    ├── types.rs       # FanInfo, CurvePoint, FanType, FanMode
    ├── controller.rs  # FanController + FanBackend trait, curve interpolation
    ├── linux.rs       # HwmonBackend, NvidiaBackend (NVML), AmdGpuBackend
    └── windows.rs     # Placeholder for the future Windows port
assets/
└── index.html         # Full GUI (HTML/CSS/JS, compiled into the binary)
```

## Roadmap

- [ ] **Windows `.exe`**: port GUI layer to `wry` + WebView2 (`#[cfg(windows)]`),
      hardware logic stays the same
- [ ] AMD GPU control — test and expand
- [x] Autostart (systemd user service)
- [x] Save / load curve profiles

## License

MIT — see [LICENSE](LICENSE).
