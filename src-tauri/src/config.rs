use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

pub const DEFAULT_ENDPOINT: &str = "wss://openspeech.bytedance.com/api/v3/sauc/bigmodel";
pub const DEFAULT_RESOURCE_ID: &str = "volc.seedasr.sauc.duration";
pub const DEFAULT_SHORTCUT: &str = "Command+LShift+Space";

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InteractionMode {
    #[default]
    Hold,
    Toggle,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct AppConfig {
    pub secret_key: String,
    pub shortcut: String,
    pub interaction_mode: InteractionMode,
    pub microphone: String,
    pub auto_insert: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            secret_key: String::new(),
            shortcut: DEFAULT_SHORTCUT.to_owned(),
            interaction_mode: InteractionMode::Hold,
            microphone: String::new(),
            auto_insert: true,
        }
    }
}

impl AppConfig {
    pub fn normalized(mut self) -> Self {
        self.secret_key = self.secret_key.trim().to_owned();
        self.shortcut = self.shortcut.trim().to_owned();
        if let Ok(binding) = crate::shortcut::ShortcutBinding::parse(&self.shortcut) {
            self.shortcut = binding.to_string();
        }
        self.microphone = self.microphone.trim().to_owned();
        self
    }

    pub fn validate_settings(&self) -> Result<()> {
        crate::shortcut::ShortcutBinding::parse(&self.shortcut)?;
        Ok(())
    }

    pub fn validate_for_dictation(&self) -> Result<()> {
        self.validate_settings()?;
        if self.secret_key.is_empty() {
            bail!("Secret Key is required");
        }
        Ok(())
    }
}

fn settings_path(app: &AppHandle) -> Result<PathBuf> {
    Ok(app
        .path()
        .app_config_dir()
        .context("failed to resolve the app config directory")?
        .join("settings.json"))
}

pub fn load(app: &AppHandle) -> Result<AppConfig> {
    let path = settings_path(app)?;
    if !path.exists() {
        return Ok(AppConfig::default());
    }

    let contents = fs::read_to_string(&path)
        .with_context(|| format!("failed to read settings from {}", path.display()))?;
    let value: serde_json::Value = serde_json::from_str(&contents)
        .with_context(|| format!("failed to parse settings from {}", path.display()))?;
    let needs_migration = ["app_id", "endpoint", "resource_id", "polish"]
        .iter()
        .any(|field| value.get(field).is_some());
    let config: AppConfig = serde_json::from_value(value)
        .with_context(|| format!("failed to parse settings from {}", path.display()))?;
    let config = config.normalized();
    if needs_migration {
        save(app, &config)?;
    }
    Ok(config)
}

pub fn save(app: &AppHandle, config: &AppConfig) -> Result<()> {
    let path = settings_path(app)?;
    let parent = path
        .parent()
        .context("settings path does not have a parent directory")?;
    fs::create_dir_all(parent)
        .with_context(|| format!("failed to create settings directory {}", parent.display()))?;

    let temporary = path.with_extension("json.tmp");
    let payload = serde_json::to_vec_pretty(config).context("failed to serialize settings")?;

    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;

        let mut file = fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)
            .with_context(|| format!("failed to create {}", temporary.display()))?;
        file.write_all(&payload)
            .with_context(|| format!("failed to write {}", temporary.display()))?;
        file.write_all(b"\n")
            .with_context(|| format!("failed to finish {}", temporary.display()))?;
    }

    #[cfg(not(unix))]
    fs::write(&temporary, [payload.as_slice(), b"\n"].concat())
        .with_context(|| format!("failed to write {}", temporary.display()))?;

    fs::rename(&temporary, &path)
        .with_context(|| format!("failed to replace settings at {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_settings_round_trip() {
        let expected = AppConfig {
            secret_key: "local-secret".to_owned(),
            shortcut: "RCommand".to_owned(),
            interaction_mode: InteractionMode::Toggle,
            microphone: "External microphone".to_owned(),
            auto_insert: false,
        };

        let encoded = serde_json::to_vec(&expected).unwrap();
        let decoded: AppConfig = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, expected);
    }

    #[test]
    fn legacy_connection_fields_are_not_serialized() {
        let legacy = serde_json::json!({
            "app_id": "legacy-app",
            "endpoint": "wss://legacy.example",
            "resource_id": "legacy-resource",
            "secret_key": "local-secret",
            "shortcut": "Command",
            "interaction_mode": "hold",
            "microphone": "",
            "auto_insert": true
        });
        let config: AppConfig = serde_json::from_value(legacy).unwrap();
        let current = serde_json::to_value(config).unwrap();

        assert!(current.get("app_id").is_none());
        assert!(current.get("endpoint").is_none());
        assert!(current.get("resource_id").is_none());
    }
}
