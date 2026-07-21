mod asr;
mod asr_options;
mod audio;
mod config;
mod controller;
mod diagnostics;
mod logging;
mod platform;
mod shortcut;

#[cfg(target_os = "linux")]
pub fn run_linux_overlay_helper() -> Result<(), String> {
    platform::run_overlay_helper().map_err(|error| error.to_string())
}

#[cfg(target_os = "macos")]
use tauri::{
    ActivationPolicy,
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
};
use tauri::{AppHandle, Manager, State};
use tracing::{debug, error, info, warn};

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

#[tauri::command]
fn resize_dictation_overlay(app: AppHandle, height: u32) -> Result<(), String> {
    controller::resize_dictation_overlay(&app, height)
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

fn show_settings(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn setup_settings_window(app: &mut tauri::App) -> tauri::Result<()> {
    let mut config = app
        .config()
        .app
        .windows
        .iter()
        .find(|config| config.label == "main")
        .cloned()
        .ok_or(tauri::Error::WindowNotFound)?;

    // tao 0.35 does not correctly initialize Wayland window controls when a
    // window is created hidden and shown later. Create the Linux settings
    // window visible from the start while preserving the macOS background-app
    // behavior.
    config.visible = !cfg!(target_os = "macos");
    let window = tauri::WebviewWindowBuilder::from_config(app.handle(), &config)?.build()?;
    platform::initialize_settings_window(&window);
    Ok(())
}

#[cfg(target_os = "macos")]
fn setup_status_item(app: &mut tauri::App) -> tauri::Result<()> {
    app.set_activation_policy(ActivationPolicy::Accessory);

    let open_settings =
        MenuItem::with_id(app, "open-settings", "Open Settings", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Voice Flow", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open_settings, &quit])?;
    let tray_icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray-icon.png"))?;

    TrayIconBuilder::with_id("voice-flow")
        .icon(tray_icon)
        .tooltip("Voice Flow")
        .icon_as_template(true)
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| {
            if event.id() == "open-settings" {
                show_settings(app);
            } else if event.id() == "quit" {
                app.exit(0);
            }
        })
        .build(app)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let graphics_workaround = platform::prepare_runtime();
    tauri::Builder::default()
        // Keep this as the first plugin so a second launch exits before it can
        // start another microphone capture and global shortcut monitor.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            info!("additional Voice Flow launch redirected to the running instance");
            show_settings(app);
        }))
        .manage(AppState::default())
        .on_window_event(|_window, _event| {
            #[cfg(target_os = "macos")]
            if _window.label() == "main"
                && let tauri::WindowEvent::CloseRequested { api, .. } = _event
            {
                api.prevent_close();
                let _ = _window.hide();
            }
        })
        .setup(move |app| {
            #[cfg(target_os = "macos")]
            setup_status_item(app)?;

            let log_path = logging::init(app.handle())?;
            if let Some(workaround) = graphics_workaround {
                info!(workaround, "applied Linux WebKitGTK graphics workaround");
            }
            setup_settings_window(app)?;
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
            if let Err(error) = platform::initialize() {
                warn!(%error, "automatic cursor insertion is unavailable");
            }
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
            resize_dictation_overlay,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run Voice Flow");
}
