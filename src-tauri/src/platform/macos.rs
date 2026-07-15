use std::process::Command;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use arboard::Clipboard;

use super::TextInjector;

pub struct MacOsTextInjector;

impl TextInjector for MacOsTextInjector {
    fn insert_at_active_cursor(&self, text: &str) -> Result<()> {
        let mut clipboard = Clipboard::new().context("failed to open the macOS clipboard")?;
        clipboard
            .set_text(text.to_owned())
            .context("failed to copy the transcript")?;

        // The transcript overlay is non-focusable, so the user's target application
        // remains frontmost. A short delay gives the pasteboard time to publish.
        thread::sleep(Duration::from_millis(70));
        let output = Command::new("osascript")
            .args([
                "-e",
                "tell application \"System Events\" to keystroke \"v\" using command down",
            ])
            .output()
            .context("failed to run the macOS paste command")?;

        if !output.status.success() {
            let detail = String::from_utf8_lossy(&output.stderr).trim().to_owned();
            bail!(
                "the transcript was copied, but macOS blocked automatic paste{}",
                if detail.is_empty() {
                    String::new()
                } else {
                    format!(": {detail}")
                }
            );
        }
        Ok(())
    }
}
