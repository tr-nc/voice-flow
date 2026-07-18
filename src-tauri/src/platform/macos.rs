use std::process::Command;
use std::ptr::NonNull;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use arboard::Clipboard;
use objc2_application_services::{AXError, AXUIElement, AXValue, AXValueType};
use objc2_core_foundation::{CFRetained, CFString, CFType, CGPoint, CGSize, ConcreteType};

use super::TextInjector;

pub struct MacOsTextInjector;

pub fn initialize_settings_window(_window: &tauri::WebviewWindow) {}

pub fn focused_window_center() -> Option<(f64, f64)> {
    // SAFETY: The system-wide AX element is returned with a +1 retain count,
    // which CFRetained manages for the remainder of this function.
    let system = unsafe { AXUIElement::new_system_wide() };
    let application = copy_attribute::<AXUIElement>(&system, "AXFocusedApplication")?;
    let window = copy_attribute::<AXUIElement>(&application, "AXFocusedWindow")?;
    let position_value = copy_attribute::<AXValue>(&window, "AXPosition")?;
    let size_value = copy_attribute::<AXValue>(&window, "AXSize")?;

    let mut position = CGPoint::ZERO;
    let mut size = CGSize::ZERO;
    // SAFETY: AXPosition and AXSize are documented to contain CGPoint and
    // CGSize respectively, and both output pointers remain valid for the call.
    let has_position =
        unsafe { position_value.value(AXValueType::CGPoint, NonNull::from(&mut position).cast()) };
    // SAFETY: See the matching AXPosition call above.
    let has_size =
        unsafe { size_value.value(AXValueType::CGSize, NonNull::from(&mut size).cast()) };
    if !has_position
        || !has_size
        || !position.x.is_finite()
        || !position.y.is_finite()
        || !size.width.is_finite()
        || !size.height.is_finite()
        || size.width <= 0.0
        || size.height <= 0.0
    {
        return None;
    }

    Some((
        position.x + size.width / 2.0,
        position.y + size.height / 2.0,
    ))
}

fn copy_attribute<T: ConcreteType>(
    element: &AXUIElement,
    attribute: &'static str,
) -> Option<CFRetained<T>> {
    let attribute = CFString::from_static_str(attribute);
    let mut value: *const CFType = std::ptr::null();
    // SAFETY: `value` is a valid out-pointer. On success the Copy API returns
    // a non-null Core Foundation object with a +1 retain count.
    let result = unsafe { element.copy_attribute_value(&attribute, NonNull::from(&mut value)) };
    if result != AXError::Success {
        return None;
    }

    let value = NonNull::new(value.cast_mut())?;
    // SAFETY: AXUIElementCopyAttributeValue follows the Core Foundation Copy
    // rule, so the returned pointer owns the retain consumed by CFRetained.
    let value = unsafe { CFRetained::<CFType>::from_raw(value) };
    value.downcast::<T>().ok()
}

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
