# FanController

A lightweight fan manager for **Linux** (Wayland & X11) with a native HTML GUI.
Controls motherboard fans, pumps, and NVIDIA GPU fans via custom curves or fixed
speeds — no browser required, no cloud, everything runs locally.

![Platform](https://img.shields.io/badge/Platform-Linux-blue)
![Language](https://img.shields.io/badge/Rust-2021-orange)

## Features

- **Motherboard fans** via `hwmon` (e.g. nct6798, it8689 …)
- **NVIDIA GPU fans** via **NVML** — works on **Wayland/Hyprland**
  (unlike `nvidia-settings`/Coolbits which require a running Xorg server)
- **AMD GPU fans** via amdgpu sysfs
- **Pump detection** — AIO pumps are identified automatically (label or RPM heuristic)
- Three modes per fan:
  - **Auto** — hand back to BIOS/driver control (restores exact original mode, e.g. Smart Fan IV)
  - **Curve** — custom temperature → speed curve, applied every second
  - **Fixed** — fixed speed percentage
- **Profiles** — Silent, Balanced, Gaming presets; Custom profile saved to disk
- **Drag-and-drop curve editor** — click to add points, right-click to remove, drag to move
- **Reset All** instantly restores every fan to BIOS control
- Custom fan labels
- **Autostart** — systemd user service, active from boot (set up automatically by the installer)
- **Failsafe** — on `systemctl stop` or Ctrl-C the daemon resets all fans to BIOS before exiting

> **Windows is not supported** — the GUI uses webkit2gtk which is Linux-only.
> Open an issue if you need Windows support.

## Quick Install (Linux)

```bash
curl -fsSL https://raw.githubusercontent.com/Smokey-thc/FanController/main/install.sh | bash
```

The script installs required packages, builds FanController from source, copies the binary to
`/usr/local/bin/fancontroller`, and enables the autostart daemon. After that:

```bash
fancontroller          # open the GUI
```

> On **first launch** FanController asks for your sudo password once to set up permissions.
> After that, fan control runs without any password prompts.

## Autostart (systemd)

The installer sets this up automatically. To manage it manually:

```bash
# Status
systemctl --user status fancontroller-daemon

# Stop / Start
systemctl --user stop fancontroller-daemon
systemctl --user start fancontroller-daemon

# Disable autostart
systemctl --user disable fancontroller-daemon
```

The daemon applies your saved **Custom** profile curves in the background — no GUI needed.
Opening the GUI stops the daemon, takes over fan control, and restarts the daemon on close.

## Failsafe

When the daemon receives **SIGTERM** (systemd stop, system shutdown) or **SIGINT** (Ctrl-C),
it resets every fan back to BIOS/driver control before exiting. This means:

- `systemctl stop fancontroller-daemon` → fans go to Auto
- System shutdown → fans go to Auto (systemd sends SIGTERM before powering off)
- `systemctl --user restart fancontroller-daemon` → brief Auto, then curves reapplied

> **Note:** SIGKILL (`kill -9`) cannot be intercepted by any process.
> In that case fans stay at their last set speed until reboot or manual reset.

## Files Modified

FanController writes only to these locations:

| Path | What |
|------|------|
| `~/.config/fancontroller/config.json` | Custom fan labels |
| `~/.config/fancontroller/custom_profile.json` | Saved curve profile |
| `~/.config/systemd/user/fancontroller-daemon.service` | Autostart service (installer only) |
| `/etc/udev/rules.d/60-fancontroller.rules` | hwmon group permissions (one-time setup, requires sudo) |
| `/etc/sudoers.d/fancontroller` | NOPASSWD rules for NVIDIA NVML (one-time setup, requires sudo) |
| `/tmp/fancontroller-daemon.pid` | Daemon PID (runtime only, deleted on exit) |
| `/sys/class/hwmon/hwmon*/pwm*` | Fan speed and enable registers (runtime, reset on exit) |

No registry, no global config, no hidden state.

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

The binary is self-contained — the entire GUI (`assets/index.html`) is compiled in via
`include_str!`. No external files need to be copied alongside it.

## How GPU Control Works

NVML puts the GPU fan into **manual mode** which holds until explicitly released.

- **Curve / Fixed** → NVML holds the speed (driver cannot override it)
- **Auto** → `set_default_fan_speed` hands the fan back to the driver

Since NVML write access requires root, FanController re-execs itself via
`sudo -n fancontroller --gpu-set/--gpu-reset` (the sudoers rule covers exactly those two
subcommands, nothing else).

## Permissions

On first launch, setup writes:

**`/etc/udev/rules.d/60-fancontroller.rules`**
```
SUBSYSTEM=="hwmon", KERNEL=="hwmon[0-9]*", ACTION=="add", GROUP="hwmon", MODE="0660"
```
Creates an `hwmon` group and adds your user to it, so hwmon sysfs files are writable
without sudo. No tee-through-sudo needed for motherboard fans.

**`/etc/sudoers.d/fancontroller`**
```
<user> ALL=(ALL) NOPASSWD: /usr/local/bin/fancontroller --gpu-set *
<user> ALL=(ALL) NOPASSWD: /usr/local/bin/fancontroller --gpu-reset *
```
Only needed for NVIDIA GPU fan control via NVML.

To remove everything:
```bash
sudo rm /etc/sudoers.d/fancontroller /etc/udev/rules.d/60-fancontroller.rules
sudo udevadm control --reload-rules
```

## Architecture

```
src/
├── main.rs            # GTK window, WebKit WebView, IPC, background thread, daemon mode
├── gpu_nvml.rs        # NVIDIA control via NVML (+ privileged CLI subcommands)
├── ipc.rs             # IPC message types (HTML ↔ Rust)
└── hardware/
    ├── types.rs       # FanInfo, CurvePoint, FanType, FanMode
    ├── controller.rs  # FanController + FanBackend trait, curve interpolation
    ├── linux.rs       # HwmonBackend, NvidiaBackend (NVML), AmdGpuBackend
    └── windows.rs     # Placeholder for a future Windows port
assets/
└── index.html         # Full GUI (HTML/CSS/JS, compiled into the binary)
packaging/
├── fancontroller-daemon.service  # systemd user service
├── fancontroller.desktop         # .desktop entry
└── fancontroller.svg             # App icon
```

## Roadmap

- [ ] AMD GPU control — broader testing
- [ ] Live temperature/RPM history graph
- [ ] Windows port (needs wry/WebView2 in place of webkit2gtk)

## License

MIT — see [LICENSE](LICENSE).
