#[cfg(not(target_os = "linux"))]
use anyhow::Context;
use anyhow::Result;
#[cfg(not(target_os = "linux"))]
use arboard::Clipboard;

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
