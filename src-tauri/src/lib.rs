mod asr;
mod asr_options;
mod audio;
mod config;
mod controller;
mod logging;
mod platform;
mod shortcut;

use tauri::{AppHandle, Manager, State};
use tracing::{debug, error, info};

use audio::Microphone;
use config::{AppConfig, InteractionMode};
use controller::{AppState, RuntimeSnapshot};

#[tauri::command]
fn get_config(state: State<'_, AppState>) -> AppConfig {
    debug!("configuration requested by UI");
    state.current_config()
}

#[tauri::command]
fn get_runtime(state: State<'_, AppState>) -> RuntimeSnapshot {
    state.runtime_snapshot()
}

#[tauri::command]
fn list_microphones() -> Result<Vec<Microphone>, String> {
    let microphones = audio::list_microphones().map_err(|error| error.to_string())?;
    info!(count = microphones.len(), "microphones enumerated");
    Ok(microphones)
}

#[tauri::command]
fn save_config(
    app: AppHandle,
    state: State<'_, AppState>,
    config: AppConfig,
) -> Result<AppConfig, String> {
    if state.is_active() {
        return Err("Stop dictation before changing settings".to_owned());
    }

    let config = config.normalized();
    config
        .validate_settings()
        .map_err(|error| error.to_string())?;
    config::save(&app, &config).map_err(|error| error.to_string())?;
    state.replace_config(config.clone());
    info!(
        shortcut = %config.shortcut,
        mode = ?config.interaction_mode,
        microphone = if config.microphone.is_empty() { "system-default" } else { &config.microphone },
        has_secret_key = !config.secret_key.is_empty(),
        auto_insert = config.auto_insert,
        "configuration saved"
    );
    Ok(config)
}

#[tauri::command]
fn start_dictation(app: AppHandle) -> Result<(), String> {
    info!(source = "ui", "dictation start requested");
    let result = controller::begin(&app);
    if let Err(error) = &result {
        error!(%error, "UI dictation start failed");
    }
    result
}

#[tauri::command]
fn stop_dictation(app: AppHandle) -> Result<(), String> {
    info!(source = "ui", "dictation stop requested");
    let result = controller::stop(&app);
    if let Err(error) = &result {
        error!(%error, "UI dictation stop failed");
    }
    result
}

fn handle_shortcut(app: &AppHandle, shortcut_event: shortcut::ShortcutEvent) {
    let state = app.state::<AppState>();
    debug!(event = ?shortcut_event, mode = ?state.interaction_mode(), "shortcut state changed");
    let result = match shortcut_event {
        shortcut::ShortcutEvent::Pressed if state.mark_shortcut_pressed() => {
            match state.interaction_mode() {
                InteractionMode::Hold => controller::begin(app),
                InteractionMode::Toggle => controller::toggle(app),
            }
        }
        shortcut::ShortcutEvent::Pressed => Ok(()),
        shortcut::ShortcutEvent::Released => {
            state.mark_shortcut_released();
            match state.interaction_mode() {
                InteractionMode::Hold => controller::stop(app),
                InteractionMode::Toggle => Ok(()),
            }
        }
    };

    if let Err(error) = result {
        error!(%error, "shortcut action failed");
        controller::report_shortcut_error(app, error);
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState::default())
        .setup(|app| {
            let log_path = logging::init(app.handle())?;
            asr::install_tls_provider()?;
            let config = config::load(app.handle())?;
            info!(
                version = env!("CARGO_PKG_VERSION"),
                log_path = %log_path.display(),
                shortcut = %config.shortcut,
                has_secret_key = !config.secret_key.is_empty(),
                "Voice Flow starting"
            );
            app.state::<AppState>().replace_config(config);
            shortcut::start_monitor(app.handle().clone(), handle_shortcut)?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_config,
            get_runtime,
            list_microphones,
            save_config,
            start_dictation,
            stop_dictation,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Voice Flow");
}
