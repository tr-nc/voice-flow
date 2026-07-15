use anyhow::{Result, bail};
use arboard::Clipboard;

use super::TextInjector;

pub struct LinuxTextInjector;

impl TextInjector for LinuxTextInjector {
    fn insert_at_active_cursor(&self, text: &str) -> Result<()> {
        let mut clipboard = Clipboard::new()?;
        clipboard.set_text(text.to_owned())?;
        bail!("Linux cursor injection is not implemented yet; the transcript was copied")
    }
}
