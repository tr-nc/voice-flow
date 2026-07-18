use anyhow::{Result, bail};

use super::TextInjector;

pub struct UnsupportedTextInjector;

pub fn initialize_settings_window(_window: &tauri::WebviewWindow) {}

impl TextInjector for UnsupportedTextInjector {
    fn insert_at_active_cursor(&self, _text: &str) -> Result<()> {
        bail!("cursor insertion is not supported on this platform")
    }
}
