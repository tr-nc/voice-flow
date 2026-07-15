use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use anyhow::{Context, Result};
use tauri::{AppHandle, Manager};
use tracing::info;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::MakeWriter;

pub const LOG_FILE_NAME: &str = "voice-flow.log";
const MAX_LOG_BYTES: u64 = 5 * 1024 * 1024;

#[derive(Clone)]
struct TeeMakeWriter {
    targets: Arc<Mutex<TeeTargets>>,
}

struct TeeWriter {
    targets: Arc<Mutex<TeeTargets>>,
}

struct TeeTargets {
    file: File,
}

impl<'writer> MakeWriter<'writer> for TeeMakeWriter {
    type Writer = TeeWriter;

    fn make_writer(&'writer self) -> Self::Writer {
        TeeWriter {
            targets: self.targets.clone(),
        }
    }
}

impl Write for TeeWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        let mut targets = lock(&self.targets);
        io::stdout().write_all(buffer)?;
        targets.file.write_all(buffer)?;
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut targets = lock(&self.targets);
        io::stdout().flush()?;
        targets.file.flush()
    }
}

pub fn init(app: &AppHandle) -> Result<PathBuf> {
    let directory = app
        .path()
        .app_log_dir()
        .context("failed to resolve the Voice Flow log directory")?;
    fs::create_dir_all(&directory)
        .with_context(|| format!("failed to create log directory {}", directory.display()))?;
    let path = directory.join(LOG_FILE_NAME);

    let truncate = fs::metadata(&path)
        .map(|metadata| metadata.len() >= MAX_LOG_BYTES)
        .unwrap_or(false);
    let mut options = OpenOptions::new();
    options
        .create(true)
        .write(true)
        .append(!truncate)
        .truncate(truncate);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(&path)
        .with_context(|| format!("failed to open log file {}", path.display()))?;
    #[cfg(unix)]
    fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o600))
        .with_context(|| format!("failed to protect log file {}", path.display()))?;

    let writer = TeeMakeWriter {
        targets: Arc::new(Mutex::new(TeeTargets { file })),
    };
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,voice_flow_lib=debug,voice_flow=debug"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(true)
        .try_init()
        .map_err(|error| anyhow::anyhow!("failed to initialize logging: {error}"))?;

    std::panic::set_hook(Box::new(|panic| {
        tracing::error!(panic = %panic, "process panic");
    }));

    info!(
        log_path = %path.display(),
        max_log_bytes = MAX_LOG_BYTES,
        truncated = truncate,
        "logging initialized"
    );
    Ok(path)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
