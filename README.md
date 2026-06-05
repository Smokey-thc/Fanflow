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

## Install

### Arch Linux (AUR)

```bash
yay -S fancontroller
```

Or manually:
```bash
git clone https://github.com/Smokey-thc/FanController.git
cd FanController/packaging
makepkg -si
```

After install, enable autostart:
```bash
systemctl --user enable --now fancontroller-daemon
fancontroller   # first launch sets up permissions (asks for sudo once)
```

### Other distros (build from source)

```bash
curl -fsSL https://raw.githubusercontent.com/Smokey-thc/FanController/main/install.sh | bash
```

The script installs required packages, builds from source, copies the binary to
`/usr/local/bin/fancontroller`, and enables the autostart daemon.

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

## Security

### Threat Model

The worst-case outcome of a bug or exploit is **fans running at the wrong speed** — too low
(overheating) or too high (noise). The SIGTERM handler ensures fans return to BIOS control
on any clean exit. No user data, no network, no persistence outside `~/.config/fancontroller/`.

| Surface | Risk | Mitigation |
|---|---|---|
| hwmon sysfs writes | Fan too low → overheat | 20 % floor enforced in code; SIGTERM resets to BIOS |
| NVML sudoers rule | Privilege escalation | Limited to exactly `--gpu-set` / `--gpu-reset`; index range validated (max 7) |
| WebKit renderer | JS code execution | Local-only, `PrivateNetwork=yes` in daemon, IPC strictly typed via Rust enums |
| IPC messages | Malformed input | `serde` rejects unknown variants at parse time — no free-form shell execution |

### Minimal Privilege Design

- **Motherboard fans** — written via `hwmon` group (udev rule), no sudo needed at runtime
- **NVIDIA GPU** — re-exec via `sudo -n fancontroller --gpu-set/--gpu-reset` only; no persistent root process
- **Daemon** — runs fully as your user; systemd sandbox restricts what it can touch

### systemd Sandbox (daemon)

```ini
ProtectSystem=strict       # /usr, /boot, /etc are read-only
PrivateNetwork=yes         # zero network access
ProtectKernelModules=yes   # cannot load kernel modules
ProtectKernelTunables=yes  # cannot modify kernel parameters
MemoryDenyWriteExecute=yes # no JIT / shellcode
LockPersonality=yes        # cannot change execution domain
```

`NoNewPrivileges` is intentionally omitted so the NVIDIA re-exec via `sudo -n` still works.
All motherboard fan control runs without any elevated privileges.

### Removing All Permissions

```bash
sudo rm /etc/sudoers.d/fancontroller /etc/udev/rules.d/60-fancontroller.rules
sudo udevadm control --reload-rules
# optionally remove the hwmon group if no other app uses it:
sudo groupdel hwmon
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
