mod asr;
mod audio;
mod config;
mod controller;
mod platform;
mod text;

use tauri::{AppHandle, Manager, State};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

use audio::Microphone;
use config::{AppConfig, InteractionMode};
use controller::{AppState, RuntimeSnapshot};

#[tauri::command]
fn get_config(state: State<'_, AppState>) -> AppConfig {
    state.current_config()
}

#[tauri::command]
fn get_runtime(state: State<'_, AppState>) -> RuntimeSnapshot {
    state.runtime_snapshot()
}

#[tauri::command]
fn list_microphones() -> Result<Vec<Microphone>, String> {
    audio::list_microphones().map_err(|error| error.to_string())
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
    config.validate().map_err(|error| error.to_string())?;
    let previous = state.current_config();

    app.global_shortcut()
        .unregister_all()
        .map_err(|error| format!("failed to release the previous shortcut: {error}"))?;
    if let Err(error) = app.global_shortcut().register(config.shortcut.as_str()) {
        let _ = app.global_shortcut().register(previous.shortcut.as_str());
        return Err(format!("failed to register {}: {error}", config.shortcut));
    }

    if let Err(error) = config::save(&app, &config) {
        let _ = app.global_shortcut().unregister_all();
        let _ = app.global_shortcut().register(previous.shortcut.as_str());
        return Err(error.to_string());
    }

    state.replace_config(config.clone());
    Ok(config)
}

#[tauri::command]
fn start_dictation(app: AppHandle) -> Result<(), String> {
    controller::begin(&app)
}

#[tauri::command]
fn stop_dictation(app: AppHandle) -> Result<(), String> {
    controller::stop(&app)
}

fn handle_shortcut(app: &AppHandle, shortcut_state: ShortcutState) {
    let state = app.state::<AppState>();
    let result = match shortcut_state {
        ShortcutState::Pressed if state.mark_shortcut_pressed() => match state.interaction_mode() {
            InteractionMode::Hold => controller::begin(app),
            InteractionMode::Toggle => controller::toggle(app),
        },
        ShortcutState::Pressed => Ok(()),
        ShortcutState::Released => {
            state.mark_shortcut_released();
            match state.interaction_mode() {
                InteractionMode::Hold => controller::stop(app),
                InteractionMode::Toggle => Ok(()),
            }
        }
    };

    if let Err(error) = result {
        controller::report_shortcut_error(app, error);
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState::default())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, _shortcut, event| {
                    handle_shortcut(app, event.state());
                })
                .build(),
        )
        .setup(|app| {
            let config = config::load(app.handle())?;
            app.global_shortcut().register(config.shortcut.as_str())?;
            app.state::<AppState>().replace_config(config);
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
