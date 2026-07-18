use std::any::Any;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use futures_util::FutureExt;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize};
use tokio::sync::oneshot;
use tracing::{Instrument, debug, error, info, info_span, warn};
use uuid::Uuid;

use crate::asr::{self, StreamEvent, TranscriptSegment};
use crate::audio::{AudioCapture, AudioEvent};
use crate::config::{AppConfig, InteractionMode};
use crate::diagnostics::DiagnosticSession;
use crate::platform;
use crate::shortcut::ShortcutBinding;

const RUNTIME_EVENT: &str = "voice-flow://runtime";
const COMPLETION_STATUS_DURATION: Duration = Duration::from_millis(150);
const DICTATION_MIN_HEIGHT: u32 = 94;
const DICTATION_MAX_HEIGHT: u32 = 280;
const DICTATION_BOTTOM_MARGIN: f64 = 92.0;
#[cfg(target_os = "linux")]
const LINUX_FOCUS_RETURN_DELAY: Duration = Duration::from_millis(120);

pub struct AppState {
    pub config: Mutex<AppConfig>,
    shortcut: Mutex<ShortcutBinding>,
    session: Mutex<Option<ActiveSession>>,
    runtime: Mutex<RuntimeSnapshot>,
    shortcut_down: AtomicBool,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            config: Mutex::new(AppConfig::default()),
            shortcut: Mutex::new(
                ShortcutBinding::parse(crate::config::DEFAULT_SHORTCUT)
                    .expect("default shortcut must be valid"),
            ),
            session: Mutex::new(None),
            runtime: Mutex::new(RuntimeSnapshot::idle()),
            shortcut_down: AtomicBool::new(false),
        }
    }
}

struct ActiveSession {
    session_id: String,
    stop_sender: Option<oneshot::Sender<()>>,
    diagnostics: Option<DiagnosticSession>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeSnapshot {
    pub phase: String,
    pub transcript: String,
    pub segments: Vec<TranscriptSegment>,
    pub message: String,
}

impl RuntimeSnapshot {
    fn idle() -> Self {
        Self {
            phase: "idle".to_owned(),
            transcript: String::new(),
            segments: Vec::new(),
            message: "Ready".to_owned(),
        }
    }
}

impl AppState {
    pub fn current_config(&self) -> AppConfig {
        lock(&self.config).clone()
    }

    pub fn replace_config(&self, config: AppConfig) {
        *lock(&self.shortcut) = ShortcutBinding::parse(&config.shortcut)
            .expect("validated config must contain a valid shortcut");
        *lock(&self.config) = config;
        self.shortcut_down.store(false, Ordering::Release);
    }

    pub fn shortcut_binding(&self) -> ShortcutBinding {
        lock(&self.shortcut).clone()
    }

    pub fn runtime_snapshot(&self) -> RuntimeSnapshot {
        lock(&self.runtime).clone()
    }

    pub fn interaction_mode(&self) -> InteractionMode {
        lock(&self.config).interaction_mode
    }

    pub fn is_active(&self) -> bool {
        lock(&self.session).is_some()
    }

    pub fn mark_shortcut_pressed(&self) -> bool {
        !self.shortcut_down.swap(true, Ordering::AcqRel)
    }

    pub fn mark_shortcut_released(&self) {
        self.shortcut_down.store(false, Ordering::Release);
    }
}

pub fn begin(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let config = state.current_config();
    config
        .validate_for_dictation()
        .map_err(|error| error.to_string())?;
    if state.is_active() {
        debug!("ignored duplicate dictation start request");
        return Ok(());
    }

    let session_id = Uuid::new_v4().to_string();
    let session_span = info_span!("dictation_session", session_id = %session_id);
    let session_guard = session_span.enter();
    let diagnostics =
        match DiagnosticSession::start(app, &session_id, &config, asr::CAPTURE_TAIL_MS) {
            Ok(diagnostics) => Some(diagnostics),
            Err(error) => {
                warn!(%error, "failed to create diagnostic session; dictation will continue");
                None
            }
        };

    info!(
        mode = ?config.interaction_mode,
        microphone = if config.microphone.is_empty() { "system-default" } else { &config.microphone },
        auto_insert = config.auto_insert,
        "dictation session starting"
    );

    let (stop_sender, stop_receiver) = oneshot::channel();
    {
        let mut session = lock(&state.session);
        if session.is_some() {
            if let Some(diagnostics) = &diagnostics {
                diagnostics.fail("duplicate dictation session start");
            }
            debug!("ignored duplicate dictation start request");
            return Ok(());
        }
        *session = Some(ActiveSession {
            session_id: session_id.clone(),
            stop_sender: Some(stop_sender),
            diagnostics: diagnostics.clone(),
        });
    }

    let (audio_sender, audio_receiver) = tokio::sync::mpsc::unbounded_channel();
    let diagnostic_audio = diagnostics.as_ref().map(DiagnosticSession::audio_sink);
    let capture = match AudioCapture::start(&config.microphone, audio_sender, diagnostic_audio) {
        Ok(capture) => capture,
        Err(error) => {
            *lock(&state.session) = None;
            if let Some(diagnostics) = &diagnostics {
                diagnostics.fail(&error.to_string());
            }
            error!(%error, "failed to start microphone capture");
            return Err(error.to_string());
        }
    };

    if let Err(error) = show_dictation_window(app) {
        *lock(&state.session) = None;
        if let Some(diagnostics) = &diagnostics {
            diagnostics.fail(&error);
        }
        error!(%error, "failed to show dictation window");
        return Err(error);
    }

    publish_runtime(
        app,
        RuntimeSnapshot {
            phase: "connecting".to_owned(),
            transcript: String::new(),
            segments: Vec::new(),
            message: microphone_message(&config),
        },
    );

    let app_handle = app.clone();
    let panic_diagnostics = diagnostics.clone();
    drop(session_guard);
    tauri::async_runtime::spawn(
        async move {
            if let Err(panic) = AssertUnwindSafe(run_session(
                app_handle.clone(),
                session_id,
                config,
                stop_receiver,
                capture,
                audio_receiver,
                diagnostics,
            ))
            .catch_unwind()
            .await
            {
                recover_from_session_panic(&app_handle, panic.as_ref(), panic_diagnostics.as_ref());
            }
        }
        .instrument(session_span),
    );
    Ok(())
}

pub fn stop(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let (sender, session_id, diagnostics) = {
        let mut session = lock(&state.session);
        let Some(active) = session.as_mut() else {
            debug!("ignored dictation stop request because no session is active");
            return Ok(());
        };
        (
            active.stop_sender.take(),
            active.session_id.clone(),
            active.diagnostics.clone(),
        )
    };

    if let Some(sender) = sender {
        if let Some(diagnostics) = &diagnostics {
            diagnostics.mark_released();
        }
        info!(%session_id, "dictation stop signal sent");
        let _ = sender.send(());
        let current = state.runtime_snapshot();
        publish_runtime(
            app,
            RuntimeSnapshot {
                phase: "finalizing".to_owned(),
                transcript: current.transcript,
                segments: current.segments,
                message: "Finishing the transcript…".to_owned(),
            },
        );
    }
    Ok(())
}

pub fn toggle(app: &AppHandle) -> Result<(), String> {
    if app.state::<AppState>().is_active() {
        stop(app)
    } else {
        begin(app)
    }
}

pub fn report_shortcut_error(app: &AppHandle, error: impl Into<String>) {
    let message = error.into();
    let _ = show_dictation_window(app);
    publish_runtime(
        app,
        RuntimeSnapshot {
            phase: "error".to_owned(),
            transcript: String::new(),
            segments: Vec::new(),
            message,
        },
    );
    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(3)).await;
        hide_dictation_window(&app_handle);
        publish_runtime(&app_handle, RuntimeSnapshot::idle());
    });
}

async fn run_session(
    app: AppHandle,
    session_id: String,
    config: AppConfig,
    stop_receiver: oneshot::Receiver<()>,
    capture: AudioCapture,
    audio_receiver: tokio::sync::mpsc::UnboundedReceiver<AudioEvent>,
    diagnostics: Option<DiagnosticSession>,
) {
    let (event_sender, mut event_receiver) = tokio::sync::mpsc::unbounded_channel();
    let recognition = asr::recognize(
        config.clone(),
        session_id,
        stop_receiver,
        event_sender,
        capture,
        audio_receiver,
    );
    tokio::pin!(recognition);

    let result = loop {
        tokio::select! {
            result = &mut recognition => break result,
            event = event_receiver.recv() => {
                if let Some(event) = event {
                    handle_stream_event(&app, event, diagnostics.as_ref());
                }
            }
        }
    };

    match result {
        Ok(text) if !text.trim().is_empty() => {
            let text = text.trim().to_owned();
            if let Some(diagnostics) = &diagnostics {
                diagnostics.mark_final(&text);
            }
            info!(
                characters = text.chars().count(),
                "final ASR transcript received"
            );
            publish_runtime(
                &app,
                RuntimeSnapshot {
                    phase: "inserting".to_owned(),
                    transcript: text.clone(),
                    segments: final_segments(&text),
                    message: if config.auto_insert {
                        "Inserting at the active cursor…".to_owned()
                    } else {
                        "Copying the transcript…".to_owned()
                    },
                },
            );

            #[cfg(target_os = "linux")]
            if config.auto_insert {
                // GNOME Wayland may focus a newly shown overlay even after it
                // was marked non-focusable. Hide it before injecting paste so
                // the compositor can return focus to the user's target field.
                debug!(
                    delay_ms = LINUX_FOCUS_RETURN_DELAY.as_millis(),
                    "hiding Linux overlay before cursor insertion"
                );
                hide_dictation_window(&app);
                tokio::time::sleep(LINUX_FOCUS_RETURN_DELAY).await;
            }

            let insert_text = text.clone();
            let auto_insert = config.auto_insert;
            let insertion = tauri::async_runtime::spawn_blocking(move || {
                if auto_insert {
                    platform::insert_at_active_cursor(&insert_text)
                } else {
                    platform::copy_to_clipboard(&insert_text)
                }
            })
            .await;

            let (message, insertion_status, insertion_error) = match insertion {
                Ok(Ok(())) if config.auto_insert => {
                    info!("transcript inserted at active cursor");
                    ("Inserted at the active cursor".to_owned(), "inserted", None)
                }
                Ok(Ok(())) => {
                    info!("transcript copied to clipboard");
                    ("Copied to the clipboard".to_owned(), "copied", None)
                }
                Ok(Err(error)) => {
                    warn!(%error, "transcript insertion failed after ASR completed");
                    let detail = error.to_string();
                    (detail.clone(), "failed", Some(detail))
                }
                Err(error) => {
                    error!(%error, "text insertion worker failed");
                    let detail = format!("text insertion task failed: {error}");
                    (detail.clone(), "failed", Some(detail))
                }
            };
            if let Some(diagnostics) = &diagnostics {
                diagnostics.complete(insertion_status, insertion_error.as_deref());
            }
            publish_runtime(
                &app,
                RuntimeSnapshot {
                    phase: "complete".to_owned(),
                    segments: final_segments(&text),
                    transcript: text,
                    message,
                },
            );
            finish_session_after(&app, COMPLETION_STATUS_DURATION).await;
        }
        Ok(_) => {
            if let Some(diagnostics) = &diagnostics {
                diagnostics.mark_final("");
                diagnostics.complete("not_attempted", None);
            }
            info!("ASR session completed without detected speech");
            publish_runtime(
                &app,
                RuntimeSnapshot {
                    phase: "complete".to_owned(),
                    transcript: String::new(),
                    segments: Vec::new(),
                    message: "No speech detected".to_owned(),
                },
            );
            finish_session_after(&app, COMPLETION_STATUS_DURATION).await;
        }
        Err(error) => {
            if let Some(diagnostics) = &diagnostics {
                diagnostics.fail(&error.to_string());
            }
            error!(%error, "dictation session failed");
            publish_runtime(
                &app,
                RuntimeSnapshot {
                    phase: "error".to_owned(),
                    transcript: String::new(),
                    segments: Vec::new(),
                    message: error.to_string(),
                },
            );
            finish_session_after(&app, Duration::from_secs(3)).await;
        }
    }
}

fn handle_stream_event(
    app: &AppHandle,
    event: StreamEvent,
    diagnostics: Option<&DiagnosticSession>,
) {
    let state = app.state::<AppState>();
    match event {
        StreamEvent::Connected => {
            if let Some(diagnostics) = diagnostics {
                diagnostics.mark_connected();
            }
            info!("ASR stream connected");
            let current = state.runtime_snapshot();
            let finalizing = current.phase == "finalizing";
            publish_runtime(
                app,
                RuntimeSnapshot {
                    phase: stream_phase(&current.phase).to_owned(),
                    transcript: current.transcript,
                    segments: current.segments,
                    message: if finalizing {
                        "Finishing the transcript…".to_owned()
                    } else {
                        "Listening · release or press again to insert".to_owned()
                    },
                },
            );
        }
        StreamEvent::Transcript(update) => {
            if let Some(diagnostics) = diagnostics {
                diagnostics.record_transcript(&update.text);
            }
            debug!(
                characters = update.text.chars().count(),
                segments = update.segments.len(),
                "partial ASR transcript updated"
            );
            let current = state.runtime_snapshot();
            let finalizing = current.phase == "finalizing";
            publish_runtime(
                app,
                RuntimeSnapshot {
                    phase: stream_phase(&current.phase).to_owned(),
                    transcript: update.text,
                    segments: update.segments,
                    message: if finalizing {
                        "Finishing the transcript…".to_owned()
                    } else {
                        "Live transcription".to_owned()
                    },
                },
            );
        }
    }
}

async fn finish_session_after(app: &AppHandle, delay: Duration) {
    tokio::time::sleep(delay).await;
    debug!("dictation session returned to idle");
    *lock(&app.state::<AppState>().session) = None;
    hide_dictation_window(app);
    publish_runtime(app, RuntimeSnapshot::idle());
}

fn publish_runtime(app: &AppHandle, snapshot: RuntimeSnapshot) {
    *lock(&app.state::<AppState>().runtime) = snapshot.clone();
    let _ = app.emit(RUNTIME_EVENT, snapshot);
}

fn final_segments(text: &str) -> Vec<TranscriptSegment> {
    vec![TranscriptSegment {
        text: text.to_owned(),
        definite: true,
    }]
}

fn stream_phase(current_phase: &str) -> &'static str {
    if current_phase == "finalizing" {
        "finalizing"
    } else {
        "listening"
    }
}

fn recover_from_session_panic(
    app: &AppHandle,
    panic: &(dyn Any + Send),
    diagnostics: Option<&DiagnosticSession>,
) {
    let detail = if let Some(message) = panic.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = panic.downcast_ref::<&str>() {
        (*message).to_owned()
    } else {
        "unknown panic payload".to_owned()
    };
    if let Some(diagnostics) = diagnostics {
        diagnostics.fail(&format!("dictation worker panicked: {detail}"));
    }
    error!(%detail, "dictation worker panicked; session state recovered");
    *lock(&app.state::<AppState>().session) = None;
    publish_runtime(
        app,
        RuntimeSnapshot {
            phase: "error".to_owned(),
            transcript: String::new(),
            segments: Vec::new(),
            message: format!("Dictation worker failed: {detail}"),
        },
    );
    hide_dictation_window(app);
}

fn show_dictation_window(app: &AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window("dictation")
        .ok_or_else(|| "dictation window is unavailable".to_owned())?;

    if let Err(error) = window.set_focusable(false) {
        #[cfg(target_os = "linux")]
        warn!(%error, "the Linux window manager cannot mark the dictation overlay as non-focusable");
        #[cfg(not(target_os = "linux"))]
        return Err(format!(
            "failed to keep the dictation window unfocused: {error}"
        ));
    }
    #[cfg(not(target_os = "linux"))]
    window
        .set_ignore_cursor_events(true)
        .map_err(|error| format!("failed to make the dictation window click-through: {error}"))?;

    let target_monitor = platform::focused_window_center()
        .and_then(|(x, y)| window.monitor_from_point(x, y).ok().flatten())
        .map(|monitor| (monitor, "focused-window"))
        .or_else(|| {
            window
                .cursor_position()
                .ok()
                .and_then(|position| {
                    window
                        .monitor_from_point(position.x, position.y)
                        .ok()
                        .flatten()
                })
                .map(|monitor| (monitor, "cursor"))
        })
        .or_else(|| {
            window
                .current_monitor()
                .ok()
                .flatten()
                .map(|monitor| (monitor, "overlay"))
        });

    if let Some((monitor, source)) = target_monitor {
        debug!(
            source,
            monitor = ?monitor.name(),
            position = ?monitor.position(),
            "positioning dictation overlay"
        );
        let monitor_position = monitor.position();
        let monitor_size = monitor.size();
        if let Ok(window_size) = window.outer_size() {
            let x = monitor_position.x
                + ((monitor_size.width.saturating_sub(window_size.width)) / 2) as i32;
            let bottom_margin = (DICTATION_BOTTOM_MARGIN * monitor.scale_factor()) as i32;
            let y = monitor_position.y + monitor_size.height as i32
                - window_size.height as i32
                - bottom_margin;
            let _ = window.set_position(PhysicalPosition::new(x, y));
        }
    }

    window
        .show()
        .map_err(|error| format!("failed to show the dictation window: {error}"))?;

    // Tao's Linux backend requires a realized GTK window before changing its
    // input shape. Queuing this before `show` panics inside the event loop.
    #[cfg(target_os = "linux")]
    if let Err(error) = window.set_ignore_cursor_events(true) {
        warn!(%error, "the Linux window manager cannot make the dictation overlay click-through");
    }
    Ok(())
}

pub fn resize_dictation_overlay(app: &AppHandle, height: u32) -> Result<(), String> {
    let window = app
        .get_webview_window("dictation")
        .ok_or_else(|| "dictation window is unavailable".to_owned())?;
    let scale_factor = window
        .scale_factor()
        .map_err(|error| format!("failed to read the dictation window scale: {error}"))?;
    let current_size = window
        .outer_size()
        .map_err(|error| format!("failed to read the dictation window size: {error}"))?;
    let logical_height = height.clamp(DICTATION_MIN_HEIGHT, DICTATION_MAX_HEIGHT);
    let physical_height = (f64::from(logical_height) * scale_factor).round() as u32;

    window
        .set_size(PhysicalSize::new(current_size.width, physical_height))
        .map_err(|error| format!("failed to resize the dictation window: {error}"))?;

    if let Some(monitor) = window.current_monitor().unwrap_or_else(|error| {
        warn!(%error, "the window manager did not report the dictation overlay monitor");
        None
    }) {
        let monitor_position = monitor.position();
        let monitor_size = monitor.size();
        let x = monitor_position.x
            + ((monitor_size.width.saturating_sub(current_size.width)) / 2) as i32;
        let bottom_margin = (DICTATION_BOTTOM_MARGIN * monitor.scale_factor()) as i32;
        let y = monitor_position.y + monitor_size.height as i32
            - physical_height as i32
            - bottom_margin;
        if let Err(error) = window.set_position(PhysicalPosition::new(x, y)) {
            warn!(%error, "the window manager cannot reposition the resized dictation overlay");
        }
    }

    Ok(())
}

fn hide_dictation_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("dictation") {
        let _ = window.hide();
    }
}

fn microphone_message(config: &AppConfig) -> String {
    if config.microphone.is_empty() {
        "Opening the system microphone…".to_owned()
    } else {
        format!("Opening {}…", config.microphone)
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::stream_phase;

    #[test]
    fn late_stream_events_do_not_leave_finalizing_phase() {
        assert_eq!(stream_phase("finalizing"), "finalizing");
        assert_eq!(stream_phase("listening"), "listening");
        assert_eq!(stream_phase("connecting"), "listening");
    }
}
