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

fn main() -> anyhow::Result<()> {
    // Privileged NVML subcommands (`--gpu-set` / `--gpu-reset`). When the app
    // needs to change a GPU fan it re-execs itself through `sudo -n`; that
    // re-exec lands here and exits before any GUI/GTK initialisation.
    if let Some(code) = gpu_nvml::run_cli() {
        std::process::exit(code);
    }

    unsafe {
        std::env::set_var("GDK_BACKEND", "x11");
        std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1");
    };

    // One-time setup: install a sudoers rule so pwm files and GPU control work
    // without a password prompt. After first run the app needs no interaction.
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

/// True when first-time (or upgrade) setup is required: the pwm files still need
/// root, the sudoers rule is missing, or it was written by an older version that
/// lacked the GPU control whitelist.
fn needs_setup() -> bool {
    let marker_ok = std::fs::read_to_string(setup_marker())
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .map(|v| v >= SETUP_VERSION)
        .unwrap_or(false);
    !std::path::Path::new("/etc/sudoers.d/fancontroller").exists() || pwm_needs_root() || !marker_ok
}

fn setup_permissions() {
    let sudoers_path = "/etc/sudoers.d/fancontroller";

    eprintln!("Fan control needs elevated access. Enter sudo password (one-time setup):");

    // 1. Make pwm files writable for this session immediately
    let _ = std::process::Command::new("sudo")
        .args(["sh", "-c",
            "chmod a+w /sys/class/hwmon/hwmon*/pwm[0-9] \
                       /sys/class/hwmon/hwmon*/pwm[0-9]_enable 2>/dev/null; true"])
        .status();

    // 2. Write a sudoers NOPASSWD rule so future `sudo -n` calls work without a
    //    password prompt. Three entries:
    //      - `tee /sys/class/hwmon/*`  → motherboard (hwmon) fan writes
    //      - `<this binary> --gpu-set/--gpu-reset` → privileged NVML GPU control
    //    Always (re)written so upgrades pick up new rules (see SETUP_VERSION).
    let user = std::env::var("USER").unwrap_or_else(|_| "ALL".into());
    let exe = std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "/usr/local/bin/fancontroller".into());
    let rule = format!(
        "{user} ALL=(ALL) NOPASSWD: /usr/bin/tee /sys/class/hwmon/*\n\
         {user} ALL=(ALL) NOPASSWD: {exe} --gpu-set *\n\
         {user} ALL=(ALL) NOPASSWD: {exe} --gpu-reset *\n"
    );
    let ok = (|| -> anyhow::Result<()> {
        use std::io::Write as _;
        let mut child = std::process::Command::new("sudo")
            .args(["tee", sudoers_path])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()?;
        child.stdin.as_mut().unwrap().write_all(rule.as_bytes())?;
        child.wait()?;
        // sudoers files must be 0440
        std::process::Command::new("sudo")
            .args(["chmod", "0440", sudoers_path])
            .status()?;
        Ok(())
    })();
    match ok {
        Ok(()) => {
            // Record the installed version so we don't prompt again until the
            // rule format changes.
            let marker = setup_marker();
            if let Some(dir) = marker.parent() { std::fs::create_dir_all(dir).ok(); }
            std::fs::write(&marker, SETUP_VERSION.to_string()).ok();
            eprintln!("Setup complete — no password needed from now on.");
        }
        Err(e) => eprintln!("Could not write sudoers rule ({e}). You may need to enter the password on each start."),
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
