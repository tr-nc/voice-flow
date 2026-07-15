use std::any::Any;
use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

use futures_util::FutureExt;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition};
use tokio::sync::oneshot;
use tracing::{debug, error, info, warn};

use crate::asr::{self, StreamEvent};
use crate::config::{AppConfig, InteractionMode};
use crate::platform;
use crate::shortcut::ShortcutBinding;

const RUNTIME_EVENT: &str = "voice-flow://runtime";

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
    stop_sender: Option<oneshot::Sender<()>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeSnapshot {
    pub phase: String,
    pub transcript: String,
    pub message: String,
}

impl RuntimeSnapshot {
    fn idle() -> Self {
        Self {
            phase: "idle".to_owned(),
            transcript: String::new(),
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
            debug!("ignored duplicate dictation start request");
            return Ok(());
        }
        *session = Some(ActiveSession {
            stop_sender: Some(stop_sender),
        });
    }

    if let Err(error) = show_dictation_window(app) {
        *lock(&state.session) = None;
        error!(%error, "failed to show dictation window");
        return Err(error);
    }

    publish_runtime(
        app,
        RuntimeSnapshot {
            phase: "connecting".to_owned(),
            transcript: String::new(),
            message: microphone_message(&config),
        },
    );

    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(panic) = AssertUnwindSafe(run_session(app_handle.clone(), config, stop_receiver))
            .catch_unwind()
            .await
        {
            recover_from_session_panic(&app_handle, panic.as_ref());
        }
    });
    Ok(())
}

pub fn stop(app: &AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    let sender = {
        let mut session = lock(&state.session);
        let Some(active) = session.as_mut() else {
            debug!("ignored dictation stop request because no session is active");
            return Ok(());
        };
        active.stop_sender.take()
    };

    if let Some(sender) = sender {
        info!("dictation stop signal sent");
        let _ = sender.send(());
        let current = state.runtime_snapshot();
        publish_runtime(
            app,
            RuntimeSnapshot {
                phase: "finalizing".to_owned(),
                transcript: current.transcript,
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

async fn run_session(app: AppHandle, config: AppConfig, stop_receiver: oneshot::Receiver<()>) {
    let (event_sender, mut event_receiver) = tokio::sync::mpsc::unbounded_channel();
    let recognition = asr::recognize(config.clone(), stop_receiver, event_sender);
    tokio::pin!(recognition);

    let result = loop {
        tokio::select! {
            result = &mut recognition => break result,
            event = event_receiver.recv() => {
                if let Some(event) = event {
                    handle_stream_event(&app, event);
                }
            }
        }
    };

    match result {
        Ok(text) if !text.trim().is_empty() => {
            let text = text.trim().to_owned();
            info!(
                characters = text.chars().count(),
                "final ASR transcript received"
            );
            publish_runtime(
                &app,
                RuntimeSnapshot {
                    phase: "inserting".to_owned(),
                    transcript: text.clone(),
                    message: if config.auto_insert {
                        "Inserting at the active cursor…".to_owned()
                    } else {
                        "Copying the transcript…".to_owned()
                    },
                },
            );

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

            let message = match insertion {
                Ok(Ok(())) if config.auto_insert => {
                    info!("transcript inserted at active cursor");
                    "Inserted at the active cursor".to_owned()
                }
                Ok(Ok(())) => {
                    info!("transcript copied to clipboard");
                    "Copied to the clipboard".to_owned()
                }
                Ok(Err(error)) => {
                    warn!(%error, "transcript insertion failed after ASR completed");
                    error.to_string()
                }
                Err(error) => {
                    error!(%error, "text insertion worker failed");
                    format!("text insertion task failed: {error}")
                }
            };
            publish_runtime(
                &app,
                RuntimeSnapshot {
                    phase: "complete".to_owned(),
                    transcript: text,
                    message,
                },
            );
            finish_session_after(&app, Duration::from_millis(1_100)).await;
        }
        Ok(_) => {
            info!("ASR session completed without detected speech");
            publish_runtime(
                &app,
                RuntimeSnapshot {
                    phase: "complete".to_owned(),
                    transcript: String::new(),
                    message: "No speech detected".to_owned(),
                },
            );
            finish_session_after(&app, Duration::from_millis(1_100)).await;
        }
        Err(error) => {
            error!(%error, "dictation session failed");
            publish_runtime(
                &app,
                RuntimeSnapshot {
                    phase: "error".to_owned(),
                    transcript: String::new(),
                    message: error.to_string(),
                },
            );
            finish_session_after(&app, Duration::from_secs(3)).await;
        }
    }
}

fn handle_stream_event(app: &AppHandle, event: StreamEvent) {
    let state = app.state::<AppState>();
    match event {
        StreamEvent::Connected => {
            info!("ASR stream connected");
            let current = state.runtime_snapshot();
            publish_runtime(
                app,
                RuntimeSnapshot {
                    phase: "listening".to_owned(),
                    transcript: current.transcript,
                    message: "Listening · release or press again to insert".to_owned(),
                },
            );
        }
        StreamEvent::Transcript(transcript) => {
            debug!(
                characters = transcript.chars().count(),
                "partial ASR transcript updated"
            );
            publish_runtime(
                app,
                RuntimeSnapshot {
                    phase: "listening".to_owned(),
                    transcript,
                    message: "Live transcription".to_owned(),
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

fn recover_from_session_panic(app: &AppHandle, panic: &(dyn Any + Send)) {
    let detail = if let Some(message) = panic.downcast_ref::<String>() {
        message.clone()
    } else if let Some(message) = panic.downcast_ref::<&str>() {
        (*message).to_owned()
    } else {
        "unknown panic payload".to_owned()
    };
    error!(%detail, "dictation worker panicked; session state recovered");
    *lock(&app.state::<AppState>().session) = None;
    publish_runtime(
        app,
        RuntimeSnapshot {
            phase: "error".to_owned(),
            transcript: String::new(),
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

    if let Ok(Some(monitor)) = window.current_monitor() {
        let monitor_position = monitor.position();
        let monitor_size = monitor.size();
        if let Ok(window_size) = window.outer_size() {
            let x = monitor_position.x
                + ((monitor_size.width.saturating_sub(window_size.width)) / 2) as i32;
            let bottom_margin = (92.0 * monitor.scale_factor()) as i32;
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
