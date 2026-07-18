use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use arboard::Clipboard;
use evdev::{AttributeSet, KeyCode, KeyEvent, uinput::VirtualDevice};
use tracing::{debug, info};

use super::TextInjector;

const CLIPBOARD_SETTLE_DELAY: Duration = Duration::from_millis(70);
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
    fn insert_at_active_cursor(&self, text: &str) -> Result<()> {
        copy_to_clipboard(text)?;
        thread::sleep(CLIPBOARD_SETTLE_DELAY);

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
        result.context("the transcript was copied, but Linux blocked automatic paste")
    }
}

pub fn copy_to_clipboard(text: &str) -> Result<()> {
    if is_wayland_session() {
        copy_with_wl_copy(text)
    } else {
        copy_with_arboard(text)
    }
}

fn is_wayland_session() -> bool {
    std::env::var("XDG_SESSION_TYPE").as_deref() == Ok("wayland")
        || std::env::var_os("WAYLAND_DISPLAY").is_some()
}

fn copy_with_wl_copy(text: &str) -> Result<()> {
    let mut child = Command::new("wl-copy")
        .args(["--type", "text/plain;charset=utf-8"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        // wl-copy forks a background clipboard owner. Inheriting a captured
        // stderr pipe would keep wait_with_output blocked on that owner.
        .stderr(Stdio::null())
        .spawn()
        .context("failed to start wl-copy; install the wl-clipboard package")?;

    child
        .stdin
        .take()
        .context("failed to open wl-copy stdin")?
        .write_all(text.as_bytes())
        .context("failed to send the transcript to wl-copy")?;

    let status = child.wait().context("failed to wait for wl-copy")?;
    if !status.success() {
        bail!("wl-copy failed to publish the transcript with status {status}");
    }

    debug!(provider = "wl-copy", "Linux clipboard updated");
    Ok(())
}

fn copy_with_arboard(text: &str) -> Result<()> {
    let mut clipboard = Clipboard::new().context("failed to open the Linux clipboard")?;
    clipboard
        .set_text(text.to_owned())
        .context("failed to copy the transcript")?;
    debug!(provider = "arboard", "Linux clipboard updated");
    Ok(())
}

pub fn initialize() -> Result<()> {
    drop(paste_device()?);
    Ok(())
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
