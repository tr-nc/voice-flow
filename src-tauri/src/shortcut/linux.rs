use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use device_query::Keycode;
use evdev::{Device, EventSummary, KeyCode};
use tauri::{AppHandle, Manager};
use tracing::{error, info, warn};

use super::ShortcutEvent;
use crate::controller::AppState;

const DEVICE_RESCAN_INTERVAL: Duration = Duration::from_secs(2);

enum DeviceUpdate {
    State {
        path: PathBuf,
        pressed: HashSet<Keycode>,
    },
    Disconnected {
        path: PathBuf,
        error: String,
    },
}

pub(super) fn start_monitor(app: AppHandle, handler: fn(&AppHandle, ShortcutEvent)) -> Result<()> {
    thread::Builder::new()
        .name("voice-flow-shortcut".to_owned())
        .spawn(move || monitor(app, handler))
        .context("failed to start the Linux global shortcut monitor")?;
    Ok(())
}

fn monitor(app: AppHandle, handler: fn(&AppHandle, ShortcutEvent)) {
    let (sender, receiver) = mpsc::channel();
    let mut known_devices = HashSet::new();
    let mut device_states = HashMap::new();
    let mut was_pressed = false;
    let mut reported_missing_permission = false;
    refresh_keyboards(
        &sender,
        &mut known_devices,
        &mut reported_missing_permission,
    );

    loop {
        match receiver.recv_timeout(DEVICE_RESCAN_INTERVAL) {
            Ok(DeviceUpdate::State { path, pressed }) => {
                device_states.insert(path, pressed);
            }
            Ok(DeviceUpdate::Disconnected { path, error }) => {
                warn!(device = %path.display(), %error, "Linux keyboard input device disconnected");
                known_devices.remove(&path);
                device_states.remove(&path);
                refresh_keyboards(
                    &sender,
                    &mut known_devices,
                    &mut reported_missing_permission,
                );
            }
            Err(RecvTimeoutError::Timeout) => {
                refresh_keyboards(
                    &sender,
                    &mut known_devices,
                    &mut reported_missing_permission,
                );
                continue;
            }
            Err(RecvTimeoutError::Disconnected) => {
                error!("Linux keyboard monitor stopped because all device readers exited");
                return;
            }
        }

        let binding = app.state::<AppState>().shortcut_binding();
        let pressed = device_states
            .values()
            .flat_map(|keys| keys.iter().copied())
            .collect();
        let is_pressed = binding.is_pressed(&pressed);

        if is_pressed != was_pressed {
            handler(
                &app,
                if is_pressed {
                    ShortcutEvent::Pressed
                } else {
                    ShortcutEvent::Released
                },
            );
            was_pressed = is_pressed;
        }
    }
}

fn refresh_keyboards(
    sender: &Sender<DeviceUpdate>,
    known_devices: &mut HashSet<PathBuf>,
    reported_missing_permission: &mut bool,
) {
    let discovered = discover_keyboards(sender, known_devices);
    if discovered > 0 {
        info!(
            discovered,
            total = known_devices.len(),
            "Linux keyboard input monitor started"
        );
        *reported_missing_permission = false;
    } else if known_devices.is_empty() && !*reported_missing_permission {
        error!(
            "no readable Linux keyboard event device was found; add the current user to the input group, sign out, and sign back in"
        );
        *reported_missing_permission = true;
    }
}

fn discover_keyboards(
    sender: &Sender<DeviceUpdate>,
    known_devices: &mut HashSet<PathBuf>,
) -> usize {
    let mut discovered = 0;
    for (path, device) in evdev::enumerate() {
        if known_devices.contains(&path) || !is_keyboard(&device) {
            continue;
        }

        let name = device.name().unwrap_or("unnamed keyboard").to_owned();
        known_devices.insert(path.clone());
        match spawn_device_reader(path.clone(), name.clone(), device, sender.clone()) {
            Ok(()) => {
                info!(device = %path.display(), %name, "monitoring Linux keyboard input device");
                discovered += 1;
            }
            Err(error) => {
                known_devices.remove(&path);
                warn!(device = %path.display(), %error, "failed to start Linux keyboard reader");
            }
        }
    }
    discovered
}

fn is_keyboard(device: &Device) -> bool {
    device.supported_keys().is_some_and(|keys| {
        keys.contains(KeyCode::KEY_A)
            && keys.contains(KeyCode::KEY_Z)
            && keys.contains(KeyCode::KEY_SPACE)
            && keys.contains(KeyCode::KEY_ENTER)
    })
}

fn spawn_device_reader(
    path: PathBuf,
    name: String,
    mut device: Device,
    sender: Sender<DeviceUpdate>,
) -> Result<()> {
    let thread_name = format!(
        "voice-flow-{}",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("keyboard")
    );
    let context_path = path.clone();
    thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            let mut pressed = current_keys(&device);
            if sender
                .send(DeviceUpdate::State {
                    path: path.clone(),
                    pressed: pressed.clone(),
                })
                .is_err()
            {
                return;
            }

            loop {
                let events = match device.fetch_events() {
                    Ok(events) => events,
                    Err(error) => {
                        let _ = sender.send(DeviceUpdate::Disconnected {
                            path,
                            error: error.to_string(),
                        });
                        return;
                    }
                };

                let mut changed = false;
                for event in events {
                    let EventSummary::Key(_, key, value) = event.destructure() else {
                        continue;
                    };
                    let Some(key) = map_key(key) else {
                        continue;
                    };
                    match value {
                        0 => changed |= pressed.remove(&key),
                        1 => changed |= pressed.insert(key),
                        _ => {}
                    }
                }

                if changed
                    && sender
                        .send(DeviceUpdate::State {
                            path: path.clone(),
                            pressed: pressed.clone(),
                        })
                        .is_err()
                {
                    return;
                }
            }
        })
        .with_context(|| {
            format!(
                "failed to start the reader for {} ({name})",
                context_path.display()
            )
        })?;
    Ok(())
}

fn current_keys(device: &Device) -> HashSet<Keycode> {
    device
        .get_key_state()
        .map(|keys| keys.iter().filter_map(map_key).collect())
        .unwrap_or_default()
}

fn map_key(key: KeyCode) -> Option<Keycode> {
    Some(match key {
        KeyCode::KEY_0 => Keycode::Key0,
        KeyCode::KEY_1 => Keycode::Key1,
        KeyCode::KEY_2 => Keycode::Key2,
        KeyCode::KEY_3 => Keycode::Key3,
        KeyCode::KEY_4 => Keycode::Key4,
        KeyCode::KEY_5 => Keycode::Key5,
        KeyCode::KEY_6 => Keycode::Key6,
        KeyCode::KEY_7 => Keycode::Key7,
        KeyCode::KEY_8 => Keycode::Key8,
        KeyCode::KEY_9 => Keycode::Key9,
        KeyCode::KEY_A => Keycode::A,
        KeyCode::KEY_B => Keycode::B,
        KeyCode::KEY_C => Keycode::C,
        KeyCode::KEY_D => Keycode::D,
        KeyCode::KEY_E => Keycode::E,
        KeyCode::KEY_F => Keycode::F,
        KeyCode::KEY_G => Keycode::G,
        KeyCode::KEY_H => Keycode::H,
        KeyCode::KEY_I => Keycode::I,
        KeyCode::KEY_J => Keycode::J,
        KeyCode::KEY_K => Keycode::K,
        KeyCode::KEY_L => Keycode::L,
        KeyCode::KEY_M => Keycode::M,
        KeyCode::KEY_N => Keycode::N,
        KeyCode::KEY_O => Keycode::O,
        KeyCode::KEY_P => Keycode::P,
        KeyCode::KEY_Q => Keycode::Q,
        KeyCode::KEY_R => Keycode::R,
        KeyCode::KEY_S => Keycode::S,
        KeyCode::KEY_T => Keycode::T,
        KeyCode::KEY_U => Keycode::U,
        KeyCode::KEY_V => Keycode::V,
        KeyCode::KEY_W => Keycode::W,
        KeyCode::KEY_X => Keycode::X,
        KeyCode::KEY_Y => Keycode::Y,
        KeyCode::KEY_Z => Keycode::Z,
        KeyCode::KEY_F1 => Keycode::F1,
        KeyCode::KEY_F2 => Keycode::F2,
        KeyCode::KEY_F3 => Keycode::F3,
        KeyCode::KEY_F4 => Keycode::F4,
        KeyCode::KEY_F5 => Keycode::F5,
        KeyCode::KEY_F6 => Keycode::F6,
        KeyCode::KEY_F7 => Keycode::F7,
        KeyCode::KEY_F8 => Keycode::F8,
        KeyCode::KEY_F9 => Keycode::F9,
        KeyCode::KEY_F10 => Keycode::F10,
        KeyCode::KEY_F11 => Keycode::F11,
        KeyCode::KEY_F12 => Keycode::F12,
        KeyCode::KEY_F13 => Keycode::F13,
        KeyCode::KEY_F14 => Keycode::F14,
        KeyCode::KEY_F15 => Keycode::F15,
        KeyCode::KEY_F16 => Keycode::F16,
        KeyCode::KEY_F17 => Keycode::F17,
        KeyCode::KEY_F18 => Keycode::F18,
        KeyCode::KEY_F19 => Keycode::F19,
        KeyCode::KEY_F20 => Keycode::F20,
        KeyCode::KEY_ESC => Keycode::Escape,
        KeyCode::KEY_SPACE => Keycode::Space,
        KeyCode::KEY_LEFTCTRL => Keycode::LControl,
        KeyCode::KEY_RIGHTCTRL => Keycode::RControl,
        KeyCode::KEY_LEFTSHIFT => Keycode::LShift,
        KeyCode::KEY_RIGHTSHIFT => Keycode::RShift,
        KeyCode::KEY_LEFTALT => Keycode::LOption,
        KeyCode::KEY_RIGHTALT => Keycode::ROption,
        KeyCode::KEY_LEFTMETA => Keycode::Command,
        KeyCode::KEY_RIGHTMETA => Keycode::RCommand,
        KeyCode::KEY_ENTER => Keycode::Enter,
        KeyCode::KEY_UP => Keycode::Up,
        KeyCode::KEY_DOWN => Keycode::Down,
        KeyCode::KEY_LEFT => Keycode::Left,
        KeyCode::KEY_RIGHT => Keycode::Right,
        KeyCode::KEY_BACKSPACE => Keycode::Backspace,
        KeyCode::KEY_CAPSLOCK => Keycode::CapsLock,
        KeyCode::KEY_TAB => Keycode::Tab,
        KeyCode::KEY_HOME => Keycode::Home,
        KeyCode::KEY_END => Keycode::End,
        KeyCode::KEY_PAGEUP => Keycode::PageUp,
        KeyCode::KEY_PAGEDOWN => Keycode::PageDown,
        KeyCode::KEY_INSERT => Keycode::Insert,
        KeyCode::KEY_DELETE => Keycode::Delete,
        KeyCode::KEY_KP0 => Keycode::Numpad0,
        KeyCode::KEY_KP1 => Keycode::Numpad1,
        KeyCode::KEY_KP2 => Keycode::Numpad2,
        KeyCode::KEY_KP3 => Keycode::Numpad3,
        KeyCode::KEY_KP4 => Keycode::Numpad4,
        KeyCode::KEY_KP5 => Keycode::Numpad5,
        KeyCode::KEY_KP6 => Keycode::Numpad6,
        KeyCode::KEY_KP7 => Keycode::Numpad7,
        KeyCode::KEY_KP8 => Keycode::Numpad8,
        KeyCode::KEY_KP9 => Keycode::Numpad9,
        KeyCode::KEY_KPMINUS => Keycode::NumpadSubtract,
        KeyCode::KEY_KPPLUS => Keycode::NumpadAdd,
        KeyCode::KEY_KPSLASH => Keycode::NumpadDivide,
        KeyCode::KEY_KPASTERISK => Keycode::NumpadMultiply,
        KeyCode::KEY_KPEQUAL => Keycode::NumpadEquals,
        KeyCode::KEY_KPENTER => Keycode::NumpadEnter,
        KeyCode::KEY_KPDOT => Keycode::NumpadDecimal,
        KeyCode::KEY_GRAVE => Keycode::Grave,
        KeyCode::KEY_MINUS => Keycode::Minus,
        KeyCode::KEY_EQUAL => Keycode::Equal,
        KeyCode::KEY_LEFTBRACE => Keycode::LeftBracket,
        KeyCode::KEY_RIGHTBRACE => Keycode::RightBracket,
        KeyCode::KEY_BACKSLASH => Keycode::BackSlash,
        KeyCode::KEY_SEMICOLON => Keycode::Semicolon,
        KeyCode::KEY_APOSTROPHE => Keycode::Apostrophe,
        KeyCode::KEY_COMMA => Keycode::Comma,
        KeyCode::KEY_DOT => Keycode::Dot,
        KeyCode::KEY_SLASH => Keycode::Slash,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_linux_modifier_sides_to_canonical_shortcut_keys() {
        assert_eq!(map_key(KeyCode::KEY_LEFTMETA), Some(Keycode::Command));
        assert_eq!(map_key(KeyCode::KEY_RIGHTMETA), Some(Keycode::RCommand));
        assert_eq!(map_key(KeyCode::KEY_LEFTALT), Some(Keycode::LOption));
        assert_eq!(map_key(KeyCode::KEY_RIGHTALT), Some(Keycode::ROption));
    }
}
