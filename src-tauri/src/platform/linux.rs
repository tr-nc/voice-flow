use std::io::Write;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use arboard::Clipboard;
use evdev::{AttributeSet, KeyCode, KeyEvent, uinput::VirtualDevice};
use gtk::prelude::*;
use tracing::{info, warn};

use super::{
    ClipboardRestoreStatus, InsertionReport, TextInjector, linux_overlay, linux_shell_overlay,
};

const CLIPBOARD_SETTLE_DELAY: Duration = Duration::from_millis(70);
const CLIPBOARD_RESTORE_DELAY: Duration = Duration::from_millis(120);
const VIRTUAL_DEVICE_SETTLE_DELAY: Duration = Duration::from_millis(120);
const KEYSTROKE_DELAY: Duration = Duration::from_millis(18);
// systemd's input_id classifies an event device as a keyboard only when it
// advertises the complete low key-code block. GNOME/libinput ignores a uinput
// device that exposes only Shift and Insert as a generic key device.
const KEYBOARD_CLASSIFICATION_KEYS: &[KeyCode] = &[
    KeyCode::KEY_ESC,
    KeyCode::KEY_1,
    KeyCode::KEY_2,
    KeyCode::KEY_3,
    KeyCode::KEY_4,
    KeyCode::KEY_5,
    KeyCode::KEY_6,
    KeyCode::KEY_7,
    KeyCode::KEY_8,
    KeyCode::KEY_9,
    KeyCode::KEY_0,
    KeyCode::KEY_MINUS,
    KeyCode::KEY_EQUAL,
    KeyCode::KEY_BACKSPACE,
    KeyCode::KEY_TAB,
    KeyCode::KEY_Q,
    KeyCode::KEY_W,
    KeyCode::KEY_E,
    KeyCode::KEY_R,
    KeyCode::KEY_T,
    KeyCode::KEY_Y,
    KeyCode::KEY_U,
    KeyCode::KEY_I,
    KeyCode::KEY_O,
    KeyCode::KEY_P,
    KeyCode::KEY_LEFTBRACE,
    KeyCode::KEY_RIGHTBRACE,
    KeyCode::KEY_ENTER,
    KeyCode::KEY_LEFTCTRL,
    KeyCode::KEY_A,
    KeyCode::KEY_S,
    KeyCode::KEY_D,
    KeyCode::KEY_V,
];

static PASTE_DEVICE: Mutex<Option<VirtualDevice>> = Mutex::new(None);

pub struct LinuxTextInjector;

pub fn prepare_runtime() -> Option<&'static str> {
    if !primary_gpu_is_nvidia() || !Path::new("/sys/module/nvidia").exists() {
        return None;
    }

    let (variable, workaround) = if is_wayland_session() {
        ("__NV_DISABLE_EXPLICIT_SYNC", "disable-nvidia-explicit-sync")
    } else {
        ("WEBKIT_DISABLE_DMABUF_RENDERER", "disable-webkit-dmabuf")
    };
    if std::env::var_os(variable).is_some() {
        return None;
    }

    // SAFETY: This is the first operation in `run`, before Tauri, WebKitGTK,
    // logging, or application worker threads are initialized.
    unsafe { std::env::set_var(variable, "1") };
    Some(workaround)
}

impl TextInjector for LinuxTextInjector {
    fn insert_at_active_cursor(&self, text: &str) -> InsertionReport {
        if is_wayland_session() {
            insert_with_wayland_clipboard(text)
        } else {
            insert_with_x11_clipboard(text)
        }
    }
}

fn is_wayland_session() -> bool {
    std::env::var("XDG_SESSION_TYPE").as_deref() == Ok("wayland")
        || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

fn insert_with_wayland_clipboard(text: &str) -> InsertionReport {
    // TODO: Preserve every advertised MIME type instead of only the plain-text
    // representation when the product expands beyond the first implementation.
    let original = read_wayland_text();
    let mut owner = match publish_wayland_text_foreground(text) {
        Ok(owner) => owner,
        Err(error) => return InsertionReport::failed_before_publish(error.to_string()),
    };
    thread::sleep(CLIPBOARD_SETTLE_DELAY);
    match owner.try_wait() {
        Ok(None) => {}
        Ok(Some(status)) => {
            return InsertionReport {
                insertion_error: Some(format!(
                    "Wayland clipboard ownership was lost before paste with status {status}"
                )),
                clipboard_restore: Some(ClipboardRestoreStatus::SkippedExternalChange),
            };
        }
        Err(error) => {
            return InsertionReport {
                insertion_error: Some(format!(
                    "failed to inspect the Wayland clipboard owner: {error}"
                )),
                clipboard_restore: Some(ClipboardRestoreStatus::Failed(
                    "could not safely determine clipboard ownership".to_owned(),
                )),
            };
        }
    }

    let insertion_error = emit_virtual_paste().err().map(|error| error.to_string());
    thread::sleep(CLIPBOARD_RESTORE_DELAY);
    let restore = match owner.try_wait() {
        Ok(None) => match stop_clipboard_owner(&mut owner) {
            Ok(()) => restore_wayland_text(original),
            Err(error) => ClipboardRestoreStatus::Failed(error.to_string()),
        },
        Ok(Some(_)) => ClipboardRestoreStatus::SkippedExternalChange,
        Err(error) => ClipboardRestoreStatus::Failed(format!(
            "failed to inspect the Wayland clipboard owner before restore: {error}"
        )),
    };
    InsertionReport {
        insertion_error,
        clipboard_restore: Some(restore),
    }
}

fn insert_with_x11_clipboard(text: &str) -> InsertionReport {
    let mut clipboard = match Clipboard::new() {
        Ok(clipboard) => clipboard,
        Err(error) => {
            return InsertionReport::failed_before_publish(format!(
                "failed to open the X11 clipboard: {error}"
            ));
        }
    };
    // TODO: Preserve every advertised clipboard target instead of only text.
    let original = clipboard.get_text().ok();
    if let Err(error) = clipboard.set_text(text.to_owned()) {
        return InsertionReport::failed_before_publish(format!(
            "failed to publish the transcript to the X11 clipboard: {error}"
        ));
    }
    thread::sleep(CLIPBOARD_SETTLE_DELAY);
    let insertion_error = emit_virtual_paste().err().map(|error| error.to_string());
    thread::sleep(CLIPBOARD_RESTORE_DELAY);

    let restore = match clipboard.get_text() {
        Ok(current) if current == text => restore_arboard_text(&mut clipboard, original),
        Ok(_) | Err(_) => ClipboardRestoreStatus::SkippedExternalChange,
    };
    InsertionReport {
        insertion_error,
        clipboard_restore: Some(restore),
    }
}

fn read_wayland_text() -> Option<String> {
    let output = Command::new("wl-paste")
        .arg("--no-newline")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8(output.stdout).ok()
}

fn publish_wayland_text_foreground(text: &str) -> Result<Child> {
    let mut child = Command::new("wl-copy")
        .args(["--foreground", "--type", "text/plain;charset=utf-8"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("failed to start wl-copy; install the wl-clipboard package")?;

    child
        .stdin
        .take()
        .context("failed to open wl-copy stdin")?
        .write_all(text.as_bytes())
        .context("failed to send the transcript to wl-copy")?;
    Ok(child)
}

fn publish_wayland_text(text: &str) -> Result<()> {
    let mut child = Command::new("wl-copy")
        .args(["--type", "text/plain;charset=utf-8"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        // The normal restore path lets wl-copy fork a persistent owner. Never
        // capture stderr here because the background owner inherits that pipe.
        .stderr(Stdio::null())
        .spawn()
        .context("failed to start wl-copy while restoring the clipboard")?;
    child
        .stdin
        .take()
        .context("failed to open wl-copy stdin while restoring the clipboard")?
        .write_all(text.as_bytes())
        .context("failed to restore the original text to wl-copy")?;
    let status = child.wait().context("failed to wait for wl-copy")?;
    if !status.success() {
        bail!("wl-copy failed to restore the clipboard with status {status}");
    }
    Ok(())
}

fn clear_wayland_clipboard() -> Result<()> {
    let status = Command::new("wl-copy")
        .arg("--clear")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("failed to clear the temporary Wayland clipboard")?;
    if !status.success() {
        bail!("wl-copy failed to clear the clipboard with status {status}");
    }
    Ok(())
}

fn stop_clipboard_owner(owner: &mut Child) -> Result<()> {
    if owner.try_wait()?.is_none() {
        owner
            .kill()
            .context("failed to stop the temporary Wayland clipboard owner")?;
    }
    owner
        .wait()
        .context("failed to reap the temporary Wayland clipboard owner")?;
    Ok(())
}

fn restore_wayland_text(original: Option<String>) -> ClipboardRestoreStatus {
    let (result, restored) = match original {
        Some(original) => (publish_wayland_text(&original), true),
        None => (clear_wayland_clipboard(), false),
    };
    match result {
        Ok(()) if restored => ClipboardRestoreStatus::Restored,
        Ok(()) => ClipboardRestoreStatus::OriginalUnavailable,
        Err(error) => ClipboardRestoreStatus::Failed(error.to_string()),
    }
}

fn restore_arboard_text(
    clipboard: &mut Clipboard,
    original: Option<String>,
) -> ClipboardRestoreStatus {
    let (result, restored) = match original {
        Some(original) => (clipboard.set_text(original), true),
        None => (clipboard.clear(), false),
    };
    match result {
        Ok(()) if restored => ClipboardRestoreStatus::Restored,
        Ok(()) => ClipboardRestoreStatus::OriginalUnavailable,
        Err(error) => ClipboardRestoreStatus::Failed(error.to_string()),
    }
}

pub fn initialize() -> Result<()> {
    if let Err(error) = paste_device() {
        warn!(%error, "Linux clipboard insertion is unavailable");
    }
    let shell_overlay_available = linux_shell_overlay::initialize()?;
    if let Err(error) = linux_overlay::initialize() {
        if !shell_overlay_available {
            return Err(error);
        }
        warn!(%error, "could not initialize the X11 dictation preview fallback");
    }
    Ok(())
}

pub fn initialize_settings_window(window: &tauri::WebviewWindow) {
    if !is_wayland_session() {
        return;
    }

    let gtk_window = match window.gtk_window() {
        Ok(window) => window,
        Err(error) => {
            warn!(%error, "could not access the Linux settings window title bar");
            return;
        }
    };
    let Some(titlebar) = gtk_window.titlebar() else {
        warn!("Linux settings window has no title bar to initialize");
        return;
    };
    let event_box = match titlebar.downcast::<gtk::EventBox>() {
        Ok(event_box) => event_box,
        Err(titlebar) => {
            warn!(
                widget_type = %titlebar.type_().name(),
                "Linux settings window uses an unexpected title bar widget"
            );
            return;
        }
    };

    // tao 0.35's Wayland title bar places this event box above its HeaderBar,
    // swallowing clicks on the native window controls until a maximize cycle
    // happens to restack the input windows. Keep the drag surface but let its
    // child buttons receive pointer events.
    let was_above_child = event_box.is_above_child();
    event_box.set_above_child(false);
    info!(
        was_above_child,
        above_child = event_box.is_above_child(),
        "initialized clickable Wayland settings window controls"
    );
}

pub fn activate_external_dictation_overlay() -> bool {
    linux_shell_overlay::is_available() || linux_overlay::is_available()
}

pub fn select_external_dictation_monitor(
    name: Option<&str>,
    physical_width: u32,
    physical_height: u32,
    scale: f64,
) {
    linux_overlay::select_monitor(name, physical_width, physical_height, scale);
}

pub fn publish_external_dictation_preview(phase: &str, text: &str, message: &str) {
    if linux_shell_overlay::is_available() {
        linux_shell_overlay::show(phase, text, message);
    } else {
        linux_overlay::show(phase, text, message);
    }
}

pub fn hide_external_dictation_preview() {
    linux_shell_overlay::hide();
    linux_overlay::hide();
}

fn primary_gpu_is_nvidia() -> bool {
    let Ok(entries) = std::fs::read_dir("/sys/class/drm") else {
        return false;
    };
    entries.flatten().any(|entry| {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            return false;
        };
        if !name.starts_with("card") || name.contains('-') {
            return false;
        }

        let device = path.join("device");
        let vendor = std::fs::read_to_string(device.join("vendor")).unwrap_or_default();
        let boot_vga = std::fs::read_to_string(device.join("boot_vga")).unwrap_or_default();
        vendor.trim().eq_ignore_ascii_case("0x10de") && boot_vga.trim() == "1"
    })
}

fn paste_device() -> Result<MutexGuard<'static, Option<VirtualDevice>>> {
    let mut device = PASTE_DEVICE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if device.is_none() {
        let mut keys = AttributeSet::<KeyCode>::new();
        for key in KEYBOARD_CLASSIFICATION_KEYS {
            keys.insert(*key);
        }
        keys.insert(KeyCode::KEY_LEFTSHIFT);

        *device = Some(
            VirtualDevice::builder()
                .context("failed to open /dev/uinput")?
                .name("Voice Flow Paste Keyboard")
                .with_keys(&keys)
                .context("failed to configure the Linux paste keyboard")?
                .build()
                .context("failed to create the Linux paste keyboard")?,
        );
        thread::sleep(VIRTUAL_DEVICE_SETTLE_DELAY);
        info!("Linux virtual paste keyboard initialized");
    }
    Ok(device)
}

fn emit_virtual_paste() -> Result<()> {
    let mut paste_device = paste_device()?;
    let result = emit_paste(
        paste_device
            .as_mut()
            .expect("paste device must be initialized"),
    );
    if result.is_err() {
        // Destroying the virtual device also releases keys if an I/O error
        // happened between the press and release event batches.
        *paste_device = None;
    }
    result.context("Linux blocked automatic paste")
}

fn emit_paste(device: &mut VirtualDevice) -> Result<()> {
    device
        .emit(&[
            *KeyEvent::new(KeyCode::KEY_LEFTCTRL, 1),
            *KeyEvent::new(KeyCode::KEY_LEFTSHIFT, 1),
            *KeyEvent::new(KeyCode::KEY_V, 1),
        ])
        .context("failed to press Ctrl+Shift+V")?;
    thread::sleep(KEYSTROKE_DELAY);
    device
        .emit(&[
            *KeyEvent::new(KeyCode::KEY_V, 0),
            *KeyEvent::new(KeyCode::KEY_LEFTSHIFT, 0),
            *KeyEvent::new(KeyCode::KEY_LEFTCTRL, 0),
        ])
        .context("failed to release Ctrl+Shift+V")
}
