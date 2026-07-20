#[cfg(not(target_os = "linux"))]
use anyhow::Context;
use anyhow::Result;
#[cfg(not(target_os = "linux"))]
use arboard::Clipboard;
use tauri::{PhysicalPosition, PhysicalSize};

#[cfg(not(target_os = "macos"))]
const DICTATION_BOTTOM_MARGIN: f64 = 92.0;

pub trait TextInjector {
    fn insert_at_active_cursor(&self, text: &str) -> Result<()>;
}

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos::MacOsTextInjector as CurrentTextInjector;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
use linux::LinuxTextInjector as CurrentTextInjector;

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
mod unsupported;
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
use unsupported::UnsupportedTextInjector as CurrentTextInjector;

pub fn prepare_runtime() -> Option<&'static str> {
    #[cfg(target_os = "linux")]
    return linux::prepare_runtime();
    #[cfg(not(target_os = "linux"))]
    None
}

pub fn initialize() -> Result<()> {
    #[cfg(target_os = "linux")]
    linux::initialize()?;
    Ok(())
}

pub fn initialize_settings_window(window: &tauri::WebviewWindow) {
    #[cfg(target_os = "linux")]
    return linux::initialize_settings_window(window);
    #[cfg(target_os = "macos")]
    return macos::initialize_settings_window(window);
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    unsupported::initialize_settings_window(window);
}

pub fn insert_at_active_cursor(text: &str) -> Result<()> {
    CurrentTextInjector.insert_at_active_cursor(text)
}

pub fn focused_window_center() -> Option<(f64, f64)> {
    #[cfg(target_os = "macos")]
    return macos::focused_window_center();
    #[cfg(not(target_os = "macos"))]
    None
}

pub fn dictation_overlay_position(
    monitor_position: &PhysicalPosition<i32>,
    monitor_size: &PhysicalSize<u32>,
    overlay_size: &PhysicalSize<u32>,
    scale_factor: f64,
) -> PhysicalPosition<i32> {
    let x =
        monitor_position.x + ((monitor_size.width.saturating_sub(overlay_size.width)) / 2) as i32;

    #[cfg(target_os = "macos")]
    let y =
        monitor_position.y + ((monitor_size.height.saturating_sub(overlay_size.height)) / 2) as i32;

    #[cfg(not(target_os = "macos"))]
    let y = monitor_position.y + monitor_size.height as i32
        - overlay_size.height as i32
        - (DICTATION_BOTTOM_MARGIN * scale_factor) as i32;

    #[cfg(target_os = "macos")]
    let _ = scale_factor;

    PhysicalPosition::new(x, y)
}

pub fn copy_to_clipboard(text: &str) -> Result<()> {
    #[cfg(target_os = "linux")]
    return linux::copy_to_clipboard(text);
    #[cfg(not(target_os = "linux"))]
    {
        let mut clipboard = Clipboard::new().context("failed to open the clipboard")?;
        clipboard
            .set_text(text.to_owned())
            .context("failed to copy the transcript")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn centers_dictation_overlay_on_macos() {
        let position = dictation_overlay_position(
            &PhysicalPosition::new(-1920, 0),
            &PhysicalSize::new(1920, 1080),
            &PhysicalSize::new(720, 94),
            2.0,
        );

        assert_eq!(position, PhysicalPosition::new(-1320, 493));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn keeps_dictation_overlay_near_the_bottom_on_other_platforms() {
        let position = dictation_overlay_position(
            &PhysicalPosition::new(0, 0),
            &PhysicalSize::new(1920, 1080),
            &PhysicalSize::new(720, 94),
            1.0,
        );

        assert_eq!(position, PhysicalPosition::new(600, 894));
    }
}
