mod hardware;
mod ipc;
mod gpu_nvml;

use hardware::FanController;
use ipc::IpcMessage;

use std::sync::{Arc, Mutex};
use gtk::prelude::*;
use webkit2gtk::{
    URISchemeRequestExt, UserContentManager, UserContentManagerExt,
    WebContext, WebContextExt, WebView, WebViewExt, WebViewExtManual,
};
use serde_json::json;
#[cfg(target_os = "linux")]
use glob::glob;

const DAEMON_PID_FILE: &str = "/tmp/fancontroller-daemon.pid";

fn main() -> anyhow::Result<()> {
    // Privileged NVML subcommands — re-exec via sudo -n, exit before GUI.
    if let Some(code) = gpu_nvml::run_cli() {
        std::process::exit(code);
    }

    // Background daemon mode: apply saved curves without any GUI.
    if std::env::args().any(|a| a == "--daemon") {
        return run_daemon();
    }

    // GUI mode: stop the daemon if it's running so we can take over.
    stop_daemon();

    unsafe {
        std::env::set_var("GDK_BACKEND", "x11");
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    };

    if !is_root() && needs_setup() {
        setup_permissions();
    }

    gtk::init()?;

    let controller = Arc::new(Mutex::new(FanController::new()));
    {
        let mut ctrl = controller.lock().unwrap();
        ctrl.scan_all();
    }

    // Register app:// scheme to serve the embedded HTML
    let context = WebContext::default().unwrap();
    context.register_uri_scheme("app", |request| {
        let html = include_str!("../assets/index.html");
        let bytes = glib::Bytes::from(html.as_bytes());
        let stream = gio::MemoryInputStream::from_bytes(&bytes);
        request.finish(&stream, html.len() as i64, Some("text/html; charset=utf-8"));
    });

    // IPC: HTML → Rust via window.webkit.messageHandlers.ipc.postMessage(msg)
    let ucm = UserContentManager::new();
    ucm.register_script_message_handler("ipc");

    // Bridge so existing HTML code (window.ipc.postMessage) keeps working
    let bridge = webkit2gtk::UserScript::new(
        "window.ipc = { postMessage: (msg) => window.webkit.messageHandlers.ipc.postMessage(msg) };",
        webkit2gtk::UserContentInjectedFrames::TopFrame,
        webkit2gtk::UserScriptInjectionTime::Start,
        &[],
        &[],
    );
    ucm.add_script(&bridge);

    let webview = WebView::new_with_context_and_user_content_manager(&context, &ucm);
    webview.load_uri("app://localhost/");

    // IPC handler
    let controller_ipc = Arc::clone(&controller);
    ucm.connect_script_message_received(Some("ipc"), move |_ucm, result| {
        if let Some(js) = result.js_value() {
            handle_ipc_message(&js.to_string(), Arc::clone(&controller_ipc));
        }
    });

    // Window
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    window.set_title("FanController");
    window.set_default_size(1100, 750);
    window.set_size_request(800, 600);
    window.add(&webview);
    window.show_all();

    window.connect_delete_event(|_, _| {
        gtk::main_quit();
        glib::Propagation::Stop
    });

    // Background thread: push fan data to GUI every second via glib channel
    let (sender, receiver) = glib::MainContext::channel(glib::Priority::DEFAULT);
    let controller_bg = Arc::clone(&controller);
    std::thread::spawn(move || {
        let mut ticks = 0u32;
        loop {
            std::thread::sleep(std::time::Duration::from_millis(1000));
            let mut ctrl = controller_bg.lock().unwrap();
            // Refresh hardware readings every 3 s
            if ticks % 3 == 0 { ctrl.scan_all(); }
            // Apply active fan curves every second
            ctrl.tick();
            ticks += 1;
            let fans = ctrl.get_all_fans();
            let custom_profile = ctrl.get_custom_profile().clone();
            let payload = json!({ "type": "fan_update", "fans": fans, "custom_profile": custom_profile });
            let _ = sender.send(payload.to_string());
        }
    });

    receiver.attach(None, move |payload| {
        let js = format!("window.__rustUpdate({})", payload);
        webview.run_javascript(&js, None::<&gio::Cancellable>, |_| {});
        glib::ControlFlow::Continue
    });

    gtk::main();
    Ok(())
}

/// Run without GUI: apply saved custom profile curves in a loop.
fn run_daemon() -> anyhow::Result<()> {
    if !is_root() && needs_setup() {
        setup_permissions();
    }

    std::fs::write(DAEMON_PID_FILE, std::process::id().to_string()).ok();

    let ctrl = Arc::new(Mutex::new(FanController::new()));
    {
        let mut c = ctrl.lock().unwrap();
        c.scan_all();
        let custom = c.get_custom_profile().clone();
        for (fan_id, points) in custom {
            if points.len() >= 2 {
                let _ = c.set_curve(&fan_id, points);
            }
        }
    }

    // SIGTERM / SIGINT → reset all fans to BIOS control before exiting.
    // systemd sends SIGTERM on `systemctl stop`; Ctrl-C sends SIGINT.
    {
        let ctrl_exit = Arc::clone(&ctrl);
        ctrlc::set_handler(move || {
            let mut c = ctrl_exit.lock().unwrap();
            let ids: Vec<String> = c.get_all_fans().iter().map(|f| f.id.clone()).collect();
            for id in &ids {
                c.reset_to_default(id);
            }
            std::fs::remove_file(DAEMON_PID_FILE).ok();
            std::process::exit(0);
        }).ok();
    }

    let mut ticks = 0u32;
    loop {
        std::thread::sleep(std::time::Duration::from_millis(1000));
        let mut c = ctrl.lock().unwrap();
        if ticks % 3 == 0 { c.scan_all(); }
        c.tick();
        ticks += 1;
    }
}

/// Kill a running daemon instance (if any) so the GUI can take over.
fn stop_daemon() {
    if let Ok(pid_str) = std::fs::read_to_string(DAEMON_PID_FILE) {
        if let Ok(pid) = pid_str.trim().parse::<u32>() {
            let _ = std::process::Command::new("kill").arg(pid.to_string()).status();
            std::thread::sleep(std::time::Duration::from_millis(300));
        }
        std::fs::remove_file(DAEMON_PID_FILE).ok();
    }
}

fn is_root() -> bool {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("Uid:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .map(|uid| uid == "0")
        })
        .unwrap_or(false)
}

fn pwm_needs_root() -> bool {
    glob("/sys/class/hwmon/hwmon*/pwm[0-9]")
        .map(|paths| {
            paths
                .filter_map(|e| e.ok())
                .any(|p| p.exists() && std::fs::OpenOptions::new().write(true).open(&p).is_err())
        })
        .unwrap_or(false)
}

/// Bump this when the sudoers rule format changes so existing installs refresh.
const SETUP_VERSION: u32 = 2;

fn setup_marker() -> std::path::PathBuf {
    std::env::var("HOME")
        .map(|h| std::path::PathBuf::from(h).join(".config/fancontroller/.setup_version"))
        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp/fancontroller-setup"))
}

fn needs_setup() -> bool {
    // Only check the udev rule — it's world-readable unlike the sudoers file (0440 root:root).
    // Path::exists() returns false on permission denied, so we can't rely on it for sudoers.
    !std::path::Path::new("/etc/udev/rules.d/60-fancontroller.rules").exists()
}

fn setup_permissions() {
    eprintln!("FanController: one-time setup — enter sudo password:");

    let user = std::env::var("USER").unwrap_or_else(|_| "ALL".into());
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "/usr/local/bin/fancontroller".into());

    let ok = (|| -> anyhow::Result<()> {
        use std::io::Write as _;

        // 1. Create hwmon group and add the user to it (secure: only group members
        //    can write fan files, not every process on the system).
        std::process::Command::new("sudo")
            .args(["groupadd", "-f", "hwmon"])
            .status()?;
        std::process::Command::new("sudo")
            .args(["usermod", "-aG", "hwmon", &user])
            .status()?;

        // 2. udev rule — as recommended by the Arch Wiki Fan speed control article.
        //    Sets group ownership directly via TAG/GROUP, no shell script needed.
        let udev_rule = "SUBSYSTEM==\"hwmon\", KERNEL==\"hwmon[0-9]*\", \
            ACTION==\"add\", GROUP=\"hwmon\", MODE=\"0660\"\n";
        let mut child = std::process::Command::new("sudo")
            .args(["tee", "/etc/udev/rules.d/60-fancontroller.rules"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()?;
        child.stdin.as_mut().unwrap().write_all(udev_rule.as_bytes())?;
        child.wait()?;

        // Reload udev rules and trigger immediately (no reboot needed).
        std::process::Command::new("sudo")
            .args(["sh", "-c",
                "udevadm control --reload-rules && udevadm trigger --subsystem-match=hwmon"])
            .status()?;

        // 3. sudoers rule — only for NVIDIA NVML (--gpu-set / --gpu-reset).
        let sudoers = format!(
            "{user} ALL=(ALL) NOPASSWD: {exe} --gpu-set *\n\
             {user} ALL=(ALL) NOPASSWD: {exe} --gpu-reset *\n"
        );
        let mut child = std::process::Command::new("sudo")
            .args(["tee", "/etc/sudoers.d/fancontroller"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()?;
        child.stdin.as_mut().unwrap().write_all(sudoers.as_bytes())?;
        child.wait()?;
        std::process::Command::new("sudo")
            .args(["chmod", "0440", "/etc/sudoers.d/fancontroller"])
            .status()?;

        Ok(())
    })();

    match ok {
        Ok(()) => eprintln!(
            "Setup complete. NOTE: log out and back in once so your user joins the hwmon group."
        ),
        Err(e) => eprintln!("Setup failed ({e})."),
    }
}

fn handle_ipc_message(raw: &str, controller: Arc<Mutex<FanController>>) {
    let msg: IpcMessage = match serde_json::from_str(raw) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("[IPC] Parse error: {e}");
            return;
        }
    };

    match msg {
        IpcMessage::SetCurve { fan_id, points } => {
            let mut ctrl = controller.lock().unwrap();
            if let Err(e) = ctrl.set_curve(&fan_id, points) {
                eprintln!("[IPC] set_curve error: {e}");
            }
        }
        IpcMessage::SetSpeed { fan_id, rpm_percent } => {
            let mut ctrl = controller.lock().unwrap();
            if let Err(e) = ctrl.set_fixed_speed(&fan_id, rpm_percent) {
                eprintln!("[IPC] set_speed error: {e}");
            }
        }
        IpcMessage::ResetToDefault { fan_id } => {
            let mut ctrl = controller.lock().unwrap();
            ctrl.reset_to_default(&fan_id);
        }
        IpcMessage::Rescan => {
            let mut ctrl = controller.lock().unwrap();
            ctrl.scan_all();
        }
        IpcMessage::RenameFan { fan_id, label } => {
            let mut ctrl = controller.lock().unwrap();
            ctrl.rename_fan(&fan_id, label);
        }
        IpcMessage::ResetAll => {
            let mut ctrl = controller.lock().unwrap();
            let ids: Vec<String> = ctrl.get_all_fans().iter().map(|f| f.id.clone()).collect();
            for id in ids {
                ctrl.reset_to_default(&id);
            }
        }
        IpcMessage::SaveCustomProfile { fan_id, points } => {
            let mut ctrl = controller.lock().unwrap();
            ctrl.save_custom_curve(&fan_id, points);
        }
    }
}
