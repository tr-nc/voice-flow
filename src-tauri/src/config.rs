use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

pub const DEFAULT_ENDPOINT: &str = "wss://openspeech.bytedance.com/api/v3/sauc/bigmodel";
pub const DEFAULT_RESOURCE_ID: &str = "volc.seedasr.sauc.duration";
pub const DEFAULT_SHORTCUT: &str = "CommandOrControl+Shift+Space";

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InteractionMode {
    #[default]
    Hold,
    Toggle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub app_id: String,
    pub secret_key: String,
    pub shortcut: String,
    pub interaction_mode: InteractionMode,
    pub microphone: String,
    pub polish: bool,
    pub auto_insert: bool,
    pub endpoint: String,
    pub resource_id: String,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            app_id: String::new(),
            secret_key: String::new(),
            shortcut: DEFAULT_SHORTCUT.to_owned(),
            interaction_mode: InteractionMode::Hold,
            microphone: String::new(),
            polish: true,
            auto_insert: true,
            endpoint: DEFAULT_ENDPOINT.to_owned(),
            resource_id: DEFAULT_RESOURCE_ID.to_owned(),
        }
    }
}

impl AppConfig {
    pub fn normalized(mut self) -> Self {
        self.app_id = self.app_id.trim().to_owned();
        self.secret_key = self.secret_key.trim().to_owned();
        self.shortcut = self.shortcut.trim().to_owned();
        self.microphone = self.microphone.trim().to_owned();
        self.endpoint = self.endpoint.trim().to_owned();
        self.resource_id = self.resource_id.trim().to_owned();
        self
    }

    pub fn validate(&self) -> Result<()> {
        if self.secret_key.is_empty() {
            bail!("Secret Key / API Key is required");
        }
        if self.shortcut.is_empty() {
            bail!("A global shortcut is required");
        }
        if !self.endpoint.starts_with("wss://") {
            bail!("The ASR endpoint must start with wss://");
        }
        if self.resource_id.is_empty() {
            bail!("The VolcEngine resource ID is required");
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
    let config: AppConfig = serde_json::from_str(&contents)
        .with_context(|| format!("failed to parse settings from {}", path.display()))?;
    Ok(config.normalized())
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
