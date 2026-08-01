use anyhow::Result;
use tauri::{PhysicalPosition, PhysicalSize};

pub trait TextInjector {
    fn insert_at_active_cursor(&self, text: &str) -> InsertionReport;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ClipboardFinalization {
    KeepTranscript,
    RestoreOriginal,
}

#[derive(Debug)]
pub enum ClipboardRestoreStatus {
    TranscriptCopied,
    Restored,
    OriginalUnavailable,
    SkippedExternalChange,
    Failed(String),
}

impl ClipboardRestoreStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::TranscriptCopied => "transcript_copied",
            Self::Restored => "restored",
            Self::OriginalUnavailable => "original_unavailable",
            Self::SkippedExternalChange => "skipped_external_change",
            Self::Failed(_) => "failed",
        }
    }

    pub fn error(&self) -> Option<&str> {
        match self {
            Self::Failed(error) => Some(error),
            _ => None,
        }
    }
}

#[derive(Debug)]
pub struct InsertionReport {
    pub insertion_error: Option<String>,
    pub clipboard_restore: Option<ClipboardRestoreStatus>,
}

impl InsertionReport {
    pub fn failed_before_publish(error: impl Into<String>) -> Self {
        Self {
            insertion_error: Some(error.into()),
            clipboard_restore: None,
        }
    }

    pub fn insertion_error(&self) -> Option<&str> {
        self.insertion_error.as_deref()
    }

    pub fn clipboard_restore(&self) -> Option<&ClipboardRestoreStatus> {
        self.clipboard_restore.as_ref()
    }

    pub fn succeeded(&self) -> bool {
        self.insertion_error.is_none()
    }

    pub fn insertion_status(&self) -> &'static str {
        match (self.succeeded(), &self.clipboard_restore) {
            (false, Some(ClipboardRestoreStatus::TranscriptCopied)) => "failed_transcript_copied",
            (false, _) => "failed_clipboard",
            (true, Some(ClipboardRestoreStatus::TranscriptCopied)) => {
                "inserted_clipboard_transcript_copied"
            }
            (true, Some(ClipboardRestoreStatus::Restored)) => "inserted_clipboard_restored",
            (true, Some(ClipboardRestoreStatus::OriginalUnavailable)) => {
                "inserted_clipboard_original_unavailable"
            }
            (true, Some(ClipboardRestoreStatus::SkippedExternalChange)) => {
                "inserted_clipboard_external_change"
            }
            (true, Some(ClipboardRestoreStatus::Failed(_))) => "inserted_clipboard_restore_failed",
            (true, None) => "inserted_clipboard_restore_not_attempted",
        }
    }

    pub fn notice(&self) -> Option<&'static str> {
        if !self.succeeded() {
            return Some(match &self.clipboard_restore {
                Some(ClipboardRestoreStatus::TranscriptCopied) => {
                    "Insertion failed · copied to clipboard"
                }
                Some(ClipboardRestoreStatus::Restored) => {
                    "Insertion failed · plain-text clipboard restored"
                }
                Some(ClipboardRestoreStatus::SkippedExternalChange) => {
                    "Insertion failed · newer clipboard content kept"
                }
                Some(ClipboardRestoreStatus::OriginalUnavailable) => {
                    "Insertion failed · non-text clipboard not restored"
                }
                Some(ClipboardRestoreStatus::Failed(_)) => {
                    "Insertion failed · copying to clipboard also failed"
                }
                None => "Insertion failed",
            });
        }
        match &self.clipboard_restore {
            Some(ClipboardRestoreStatus::TranscriptCopied) => {
                Some("Inserted · transcript kept in clipboard")
            }
            Some(ClipboardRestoreStatus::Restored) => None,
            Some(ClipboardRestoreStatus::OriginalUnavailable) => {
                Some("Inserted · non-text clipboard not restored")
            }
            Some(ClipboardRestoreStatus::SkippedExternalChange) => {
                Some("Inserted · newer clipboard content kept")
            }
            Some(ClipboardRestoreStatus::Failed(_)) => Some("Inserted · clipboard restore failed"),
            None => Some("Inserted · clipboard restore was not attempted"),
        }
    }
}

fn finish_clipboard_insertion(
    insertion_error: Option<String>,
    finalize_clipboard: impl FnOnce(ClipboardFinalization) -> ClipboardRestoreStatus,
) -> InsertionReport {
    let finalization = if insertion_error.is_some() {
        ClipboardFinalization::KeepTranscript
    } else {
        ClipboardFinalization::RestoreOriginal
    };
    let clipboard_restore = finalize_clipboard(finalization);
    InsertionReport {
        insertion_error,
        clipboard_restore: Some(clipboard_restore),
    }
}

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos::MacOsTextInjector as CurrentTextInjector;

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "linux")]
mod linux_overlay;
#[cfg(target_os = "linux")]
mod linux_shell_overlay;
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

pub fn activate_external_dictation_overlay() -> bool {
    #[cfg(target_os = "linux")]
    return linux::activate_external_dictation_overlay();
    #[cfg(not(target_os = "linux"))]
    false
}

pub fn select_external_dictation_monitor(
    name: Option<&str>,
    physical_width: u32,
    physical_height: u32,
    scale: f64,
) {
    #[cfg(target_os = "linux")]
    linux::select_external_dictation_monitor(name, physical_width, physical_height, scale);
    #[cfg(not(target_os = "linux"))]
    let _ = (name, physical_width, physical_height, scale);
}

pub fn publish_dictation_preview(phase: &str, text: &str, message: &str) {
    #[cfg(target_os = "linux")]
    linux::publish_external_dictation_preview(phase, text, message);
    #[cfg(not(target_os = "linux"))]
    let _ = (phase, text, message);
}

pub fn hide_external_dictation_overlay() {
    #[cfg(target_os = "linux")]
    linux::hide_external_dictation_preview();
}

#[cfg(target_os = "linux")]
pub fn run_overlay_helper() -> Result<()> {
    linux_overlay::run_helper()
}

pub fn insert_at_active_cursor(text: &str) -> InsertionReport {
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
    _scale_factor: f64,
) -> PhysicalPosition<i32> {
    let x =
        monitor_position.x + ((monitor_size.width.saturating_sub(overlay_size.width)) / 2) as i32;
    let y =
        monitor_position.y + ((monitor_size.height.saturating_sub(overlay_size.height)) / 2) as i32;

    PhysicalPosition::new(x, y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insertion_reports_explain_clipboard_restore_outcomes() {
        let restored = InsertionReport {
            insertion_error: None,
            clipboard_restore: Some(ClipboardRestoreStatus::Restored),
        };
        assert_eq!(restored.insertion_status(), "inserted_clipboard_restored");
        assert_eq!(restored.notice(), None);

        let changed = InsertionReport {
            insertion_error: None,
            clipboard_restore: Some(ClipboardRestoreStatus::SkippedExternalChange),
        };
        assert_eq!(
            changed.insertion_status(),
            "inserted_clipboard_external_change"
        );
        assert_eq!(
            changed.notice(),
            Some("Inserted · newer clipboard content kept")
        );
    }

    #[test]
    fn failed_insertion_keeps_transcript_in_clipboard() {
        let original = "original clipboard";
        let transcript = "words that failed to insert";
        let mut clipboard = transcript.to_owned();

        let report =
            finish_clipboard_insertion(
                Some("paste was blocked".to_owned()),
                |action| match action {
                    ClipboardFinalization::KeepTranscript => {
                        clipboard = transcript.to_owned();
                        ClipboardRestoreStatus::TranscriptCopied
                    }
                    ClipboardFinalization::RestoreOriginal => {
                        clipboard = original.to_owned();
                        ClipboardRestoreStatus::Restored
                    }
                },
            );

        assert_eq!(clipboard, transcript);
        assert_eq!(report.insertion_status(), "failed_transcript_copied");
        assert_eq!(
            report.notice(),
            Some("Insertion failed · copied to clipboard")
        );
    }

    #[test]
    fn successful_insertion_restores_original_clipboard() {
        let original = "original clipboard";
        let transcript = "inserted words";
        let mut clipboard = transcript.to_owned();

        let report = finish_clipboard_insertion(None, |action| match action {
            ClipboardFinalization::KeepTranscript => {
                clipboard = transcript.to_owned();
                ClipboardRestoreStatus::TranscriptCopied
            }
            ClipboardFinalization::RestoreOriginal => {
                clipboard = original.to_owned();
                ClipboardRestoreStatus::Restored
            }
        });

        assert_eq!(clipboard, original);
        assert_eq!(report.insertion_status(), "inserted_clipboard_restored");
        assert_eq!(report.notice(), None);
    }

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

    #[test]
    fn centers_dictation_overlay_with_positive_origin() {
        let position = dictation_overlay_position(
            &PhysicalPosition::new(0, 0),
            &PhysicalSize::new(1920, 1080),
            &PhysicalSize::new(720, 94),
            1.0,
        );

        assert_eq!(position, PhysicalPosition::new(600, 493));
    }
}
