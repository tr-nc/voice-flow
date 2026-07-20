use std::collections::HashSet;
use std::fmt::{self, Display};
use std::str::FromStr;
#[cfg(not(target_os = "linux"))]
use std::thread;
#[cfg(not(target_os = "linux"))]
use std::time::Duration;

use anyhow::{Context, Result, bail};
use device_query::Keycode;
#[cfg(not(target_os = "linux"))]
use device_query::{DeviceQuery, DeviceState};
use tauri::AppHandle;
#[cfg(not(target_os = "linux"))]
use tauri::Manager;
#[cfg(not(target_os = "linux"))]
use tracing::{error, info};

#[cfg(not(target_os = "linux"))]
use crate::controller::AppState;

#[cfg(target_os = "linux")]
mod linux;

#[cfg(not(target_os = "linux"))]
const POLL_INTERVAL: Duration = Duration::from_millis(8);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutEvent {
    Pressed,
    Released,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutBinding {
    keys: Vec<Keycode>,
}

impl ShortcutBinding {
    pub fn parse(value: &str) -> Result<Self> {
        let raw_keys = value.split('+').map(str::trim).collect::<Vec<_>>();
        if raw_keys.is_empty() || raw_keys.iter().any(|key| key.is_empty()) {
            bail!("A shortcut must contain at least one key");
        }

        let mut keys = Vec::with_capacity(raw_keys.len());
        for raw_key in raw_keys {
            let key = parse_key(raw_key)
                .with_context(|| format!("Unsupported shortcut key: {raw_key}"))?;
            if keys.contains(&key) {
                bail!("Shortcut contains the same key more than once: {raw_key}");
            }
            keys.push(key);
        }
        Ok(Self { keys })
    }

    pub fn is_pressed(&self, pressed: &HashSet<Keycode>) -> bool {
        self.keys.iter().all(|key| pressed.contains(key))
    }
}

impl Display for ShortcutBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = self
            .keys
            .iter()
            .map(Keycode::to_string)
            .collect::<Vec<_>>()
            .join("+");
        formatter.write_str(&value)
    }
}

#[cfg(target_os = "linux")]
pub fn start_monitor(app: AppHandle, handler: fn(&AppHandle, ShortcutEvent)) -> Result<()> {
    linux::start_monitor(app, handler)
}

#[cfg(not(target_os = "linux"))]
pub fn start_monitor(app: AppHandle, handler: fn(&AppHandle, ShortcutEvent)) -> Result<()> {
    thread::Builder::new()
        .name("voice-flow-shortcut".to_owned())
        .spawn(move || {
            let Some(device) = DeviceState::checked_new() else {
                error!("global shortcut monitor requires Accessibility permission; grant permission and restart Voice Flow");
                return;
            };
            info!(poll_interval_ms = POLL_INTERVAL.as_millis(), "global shortcut monitor started");

            let mut was_pressed = false;
            loop {
                let binding = app.state::<AppState>().shortcut_binding();
                let pressed = device.get_keys().into_iter().collect::<HashSet<_>>();
                let is_pressed = binding.is_pressed(&pressed);

                if is_pressed != was_pressed {
                    let event = if is_pressed {
                        ShortcutEvent::Pressed
                    } else {
                        ShortcutEvent::Released
                    };
                    handler(&app, event);
                    was_pressed = is_pressed;
                }
                thread::sleep(POLL_INTERVAL);
            }
        })
        .context("failed to start the global shortcut monitor")?;
    Ok(())
}

fn parse_key(value: &str) -> Result<Keycode> {
    let canonical = match value {
        // Backward-compatible aliases from the original Tauri shortcut format.
        "CommandOrControl" | "CommandOrCtrl" | "CmdOrControl" | "CmdOrCtrl" => {
            if cfg!(target_os = "macos") {
                "Command"
            } else {
                "LControl"
            }
        }
        "Command" | "Cmd" | "Super" | "MetaLeft" => "Command",
        "RCommand" | "MetaRight" => "RCommand",
        "Control" | "Ctrl" | "ControlLeft" => "LControl",
        "ControlRight" => "RControl",
        "Shift" | "ShiftLeft" => "LShift",
        "ShiftRight" => "RShift",
        "Alt" | "Option" | "AltLeft" => "LOption",
        "AltRight" => "ROption",
        "ArrowUp" => "Up",
        "ArrowDown" => "Down",
        "ArrowLeft" => "Left",
        "ArrowRight" => "Right",
        "Backquote" => "Grave",
        "BracketLeft" => "LeftBracket",
        "BracketRight" => "RightBracket",
        "Backslash" => "BackSlash",
        "Quote" => "Apostrophe",
        "Period" => "Dot",
        "NumpadEqual" => "NumpadEquals",
        key if key.starts_with("Key") && key.len() == 4 => &key[3..],
        key if key.starts_with("Digit") && key.len() == 6 => {
            return Keycode::from_str(&format!("Key{}", &key[5..])).map_err(anyhow::Error::msg);
        }
        key => key,
    };
    Keycode::from_str(canonical).map_err(anyhow::Error::msg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_a_single_modifier_with_side_information() {
        let left = ShortcutBinding::parse("MetaLeft").unwrap();
        let right = ShortcutBinding::parse("MetaRight").unwrap();
        assert_eq!(left.to_string(), "Command");
        assert_eq!(right.to_string(), "RCommand");
        assert_ne!(left, right);
    }

    #[test]
    fn accepts_an_unmodified_regular_key() {
        let binding = ShortcutBinding::parse("KeyC").unwrap();
        assert_eq!(binding.to_string(), "C");
    }

    #[test]
    fn matches_all_keys_in_a_chord() {
        let binding = ShortcutBinding::parse("ControlLeft+KeyC").unwrap();
        let pressed = HashSet::from([Keycode::LControl, Keycode::C]);
        assert!(binding.is_pressed(&pressed));
    }
}
