use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use chrono::{DateTime, Local, SecondsFormat, Utc};
use serde::Serialize;
use tauri::{AppHandle, Manager};
use tracing::{info, warn};

use crate::asr_options::{AUDIO_EDGE_GUARD_MS, RESOURCE_ID, VAD_END_WINDOW_MS};
use crate::audio::TARGET_SAMPLE_RATE;
use crate::config::AppConfig;

const SCHEMA_VERSION: u32 = 1;
const SESSION_LIMIT: usize = 100;
const DIAGNOSTICS_DIRECTORY: &str = "diagnostics";
const SESSIONS_DIRECTORY: &str = "sessions";
const SESSION_FILE_NAME: &str = "session.json";
const AUDIO_FILE_NAME: &str = "audio.wav";

#[derive(Clone)]
pub struct DiagnosticSession {
    inner: Arc<Mutex<DiagnosticInner>>,
}

#[derive(Clone)]
pub struct DiagnosticAudioSink {
    inner: Arc<Mutex<DiagnosticInner>>,
}

struct DiagnosticInner {
    directory: PathBuf,
    sessions_directory: PathBuf,
    started: Instant,
    record: SessionRecord,
    wave: Option<WaveWriter>,
}

#[derive(Serialize)]
struct SessionRecord {
    schema_version: u32,
    session_id: String,
    status: String,
    started_at: String,
    started_at_local: String,
    ended_at: Option<String>,
    elapsed_ms: Option<u64>,
    app_version: String,
    context: SessionContext,
    audio: AudioRecord,
    asr: AsrRecord,
    transcript_updates: Vec<TranscriptUpdate>,
    final_text: String,
    timings: SessionTimings,
    insertion_status: Option<String>,
    error: Option<String>,
}

#[derive(Serialize)]
struct SessionContext {
    microphone: String,
    interaction_mode: String,
    auto_insert: bool,
}

#[derive(Serialize)]
struct AudioRecord {
    file: &'static str,
    sample_rate: u32,
    channels: u16,
    bits_per_sample: u16,
    samples: u64,
    duration_ms: u64,
    peak_sample: i32,
    clipped_samples: u64,
    source_device: Option<String>,
    source_sample_format: Option<String>,
    source_sample_rate: Option<u32>,
    source_channels: Option<u16>,
    write_error: Option<String>,
}

#[derive(Serialize)]
struct AsrRecord {
    resource_id: &'static str,
    edge_guard_ms: usize,
    vad_end_window_ms: u64,
    tail_capture_ms: u64,
}

#[derive(Serialize)]
struct TranscriptUpdate {
    elapsed_ms: u64,
    kind: &'static str,
    text: String,
}

#[derive(Default, Serialize)]
struct SessionTimings {
    microphone_opened_ms: Option<u64>,
    connected_ms: Option<u64>,
    first_text_ms: Option<u64>,
    released_ms: Option<u64>,
    final_result_ms: Option<u64>,
    completed_ms: Option<u64>,
}

impl DiagnosticSession {
    pub fn start(
        app: &AppHandle,
        session_id: &str,
        config: &AppConfig,
        tail_capture_ms: u64,
    ) -> Result<Self> {
        let sessions_directory = app
            .path()
            .app_data_dir()
            .context("failed to resolve the application data directory")?
            .join(DIAGNOSTICS_DIRECTORY)
            .join(SESSIONS_DIRECTORY);
        create_private_directory(&sessions_directory)?;

        let now = SystemTime::now();
        let unix_ms = now
            .duration_since(UNIX_EPOCH)
            .context("system clock is before the Unix epoch")?
            .as_millis();
        let directory = sessions_directory.join(format!("{unix_ms:013}_{session_id}"));
        create_private_directory(&directory)?;

        let utc: DateTime<Utc> = now.into();
        let local: DateTime<Local> = now.into();
        let wave = WaveWriter::create(&directory.join(AUDIO_FILE_NAME))?;
        let record = SessionRecord {
            schema_version: SCHEMA_VERSION,
            session_id: session_id.to_owned(),
            status: "recording".to_owned(),
            started_at: utc.to_rfc3339_opts(SecondsFormat::Millis, true),
            started_at_local: local.to_rfc3339_opts(SecondsFormat::Millis, false),
            ended_at: None,
            elapsed_ms: None,
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            context: SessionContext {
                microphone: if config.microphone.is_empty() {
                    "system-default".to_owned()
                } else {
                    config.microphone.clone()
                },
                interaction_mode: format!("{:?}", config.interaction_mode).to_lowercase(),
                auto_insert: config.auto_insert,
            },
            audio: AudioRecord {
                file: AUDIO_FILE_NAME,
                sample_rate: TARGET_SAMPLE_RATE,
                channels: 1,
                bits_per_sample: 16,
                samples: 0,
                duration_ms: 0,
                peak_sample: 0,
                clipped_samples: 0,
                source_device: None,
                source_sample_format: None,
                source_sample_rate: None,
                source_channels: None,
                write_error: None,
            },
            asr: AsrRecord {
                resource_id: RESOURCE_ID,
                edge_guard_ms: AUDIO_EDGE_GUARD_MS,
                vad_end_window_ms: VAD_END_WINDOW_MS,
                tail_capture_ms,
            },
            transcript_updates: Vec::new(),
            final_text: String::new(),
            timings: SessionTimings::default(),
            insertion_status: None,
            error: None,
        };
        let mut inner = DiagnosticInner {
            directory: directory.clone(),
            sessions_directory: sessions_directory.clone(),
            started: Instant::now(),
            record,
            wave: Some(wave),
        };
        inner.persist()?;
        prune_old_sessions(&sessions_directory);

        info!(
            session_id,
            directory = %directory.display(),
            retained_sessions = SESSION_LIMIT,
            "diagnostic session created"
        );
        Ok(Self {
            inner: Arc::new(Mutex::new(inner)),
        })
    }

    pub fn audio_sink(&self) -> DiagnosticAudioSink {
        DiagnosticAudioSink {
            inner: self.inner.clone(),
        }
    }

    pub fn mark_released(&self) {
        self.update("record dictation release", |inner| {
            inner
                .record
                .timings
                .released_ms
                .get_or_insert(inner.elapsed_ms());
        });
    }

    pub fn mark_connected(&self) {
        self.update("record ASR connection", |inner| {
            inner
                .record
                .timings
                .connected_ms
                .get_or_insert(inner.elapsed_ms());
        });
    }

    pub fn record_transcript(&self, text: &str) {
        self.update("record partial transcript", |inner| {
            let is_duplicate = inner
                .record
                .transcript_updates
                .last()
                .is_some_and(|update| update.text == text);
            if is_duplicate {
                return;
            }
            let elapsed_ms = inner.elapsed_ms();
            inner.record.timings.first_text_ms.get_or_insert(elapsed_ms);
            inner.record.transcript_updates.push(TranscriptUpdate {
                elapsed_ms,
                kind: "partial",
                text: text.to_owned(),
            });
        });
    }

    pub fn mark_final(&self, text: &str) {
        self.update("record final transcript", |inner| {
            let elapsed_ms = inner.elapsed_ms();
            inner.record.timings.final_result_ms = Some(elapsed_ms);
            inner.record.final_text = text.to_owned();
            inner.record.transcript_updates.push(TranscriptUpdate {
                elapsed_ms,
                kind: "final",
                text: text.to_owned(),
            });
        });
    }

    pub fn complete(&self, insertion_status: &str, error: Option<&str>) {
        self.finish(
            "completed",
            Some(insertion_status.to_owned()),
            error.map(str::to_owned),
        );
    }

    pub fn fail(&self, error: &str) {
        self.finish("failed", None, Some(error.to_owned()));
    }

    fn finish(&self, status: &str, insertion_status: Option<String>, error: Option<String>) {
        let sessions_directory = {
            let mut inner = lock(&self.inner);
            if inner.record.status != "recording" {
                return;
            }
            let elapsed_ms = inner.elapsed_ms();
            inner.record.status = status.to_owned();
            inner.record.ended_at = Some(Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true));
            inner.record.elapsed_ms = Some(elapsed_ms);
            inner.record.timings.completed_ms = Some(elapsed_ms);
            inner.record.insertion_status = insertion_status;
            inner.record.error = error;
            inner.finish_audio();
            if let Err(error) = inner.persist() {
                warn!(
                    session_id = %inner.record.session_id,
                    %error,
                    "failed to finalize diagnostic session metadata"
                );
            }
            inner.sessions_directory.clone()
        };
        prune_old_sessions(&sessions_directory);
    }

    fn update(&self, action: &'static str, update: impl FnOnce(&mut DiagnosticInner)) {
        let mut inner = lock(&self.inner);
        if inner.record.status != "recording" {
            return;
        }
        update(&mut inner);
        if let Err(error) = inner.persist() {
            warn!(
                session_id = %inner.record.session_id,
                %error,
                action,
                "failed to update diagnostic session"
            );
        }
    }
}

impl DiagnosticAudioSink {
    pub fn configure_source(
        &self,
        device: &str,
        sample_format: &str,
        sample_rate: u32,
        channels: u16,
    ) {
        let mut inner = lock(&self.inner);
        inner.record.audio.source_device = Some(device.to_owned());
        inner.record.audio.source_sample_format = Some(sample_format.to_owned());
        inner.record.audio.source_sample_rate = Some(sample_rate);
        inner.record.audio.source_channels = Some(channels);
    }

    pub fn mark_opened(&self) {
        let mut inner = lock(&self.inner);
        inner.record.timings.microphone_opened_ms = Some(inner.elapsed_ms());
        if let Err(error) = inner.persist() {
            warn!(
                session_id = %inner.record.session_id,
                %error,
                "failed to record microphone details"
            );
        }
    }

    pub fn write_samples(&self, samples: &[i16]) {
        let mut inner = lock(&self.inner);
        if inner.record.status != "recording" || inner.record.audio.write_error.is_some() {
            return;
        }

        let write_result = inner
            .wave
            .as_mut()
            .context("diagnostic wave writer is unavailable")
            .and_then(|wave| wave.write_samples(samples));
        if let Err(error) = write_result {
            inner.record.audio.write_error = Some(error.to_string());
            inner.wave.take();
            warn!(
                session_id = %inner.record.session_id,
                %error,
                "failed to save diagnostic audio"
            );
            return;
        }

        inner.record.audio.samples += samples.len() as u64;
        inner.record.audio.duration_ms =
            inner.record.audio.samples * 1_000 / u64::from(TARGET_SAMPLE_RATE);
        for sample in samples {
            let absolute = i32::from(*sample).abs();
            inner.record.audio.peak_sample = inner.record.audio.peak_sample.max(absolute);
            if *sample == i16::MIN || *sample == i16::MAX {
                inner.record.audio.clipped_samples += 1;
            }
        }
    }
}

impl DiagnosticInner {
    fn elapsed_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    fn persist(&mut self) -> Result<()> {
        if let Some(wave) = self.wave.as_mut()
            && let Err(error) = wave.checkpoint()
        {
            self.record.audio.write_error = Some(error.to_string());
            self.wave.take();
        }
        write_json_atomically(&self.directory.join(SESSION_FILE_NAME), &self.record)
    }

    fn finish_audio(&mut self) {
        let Some(mut wave) = self.wave.take() else {
            return;
        };
        if let Err(error) = wave.checkpoint() {
            self.record.audio.write_error = Some(error.to_string());
        }
    }
}

struct WaveWriter {
    writer: BufWriter<File>,
    samples: u64,
}

impl WaveWriter {
    fn create(path: &Path) -> Result<Self> {
        let file = open_private_file(path)?;
        let mut writer = BufWriter::new(file);
        write_wave_header(&mut writer, 0)?;
        let mut wave = Self { writer, samples: 0 };
        wave.checkpoint()?;
        Ok(wave)
    }

    fn write_samples(&mut self, samples: &[i16]) -> Result<()> {
        let mut bytes = Vec::with_capacity(samples.len() * 2);
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        self.writer
            .write_all(&bytes)
            .context("failed to write diagnostic PCM audio")?;
        self.samples += samples.len() as u64;
        Ok(())
    }

    fn checkpoint(&mut self) -> Result<()> {
        let data_bytes = self
            .samples
            .checked_mul(2)
            .and_then(|bytes| u32::try_from(bytes).ok())
            .context("diagnostic audio exceeds the WAV size limit")?;
        self.writer
            .flush()
            .context("failed to flush diagnostic audio")?;
        self.writer
            .seek(SeekFrom::Start(4))
            .context("failed to seek to the WAV RIFF size")?;
        self.writer
            .write_all(&(36_u32 + data_bytes).to_le_bytes())
            .context("failed to update the WAV RIFF size")?;
        self.writer
            .seek(SeekFrom::Start(40))
            .context("failed to seek to the WAV data size")?;
        self.writer
            .write_all(&data_bytes.to_le_bytes())
            .context("failed to update the WAV data size")?;
        self.writer
            .seek(SeekFrom::End(0))
            .context("failed to return to the end of the WAV file")?;
        self.writer
            .flush()
            .context("failed to finish the WAV checkpoint")?;
        Ok(())
    }
}

fn write_wave_header(writer: &mut impl Write, data_bytes: u32) -> Result<()> {
    writer.write_all(b"RIFF")?;
    writer.write_all(&(36_u32 + data_bytes).to_le_bytes())?;
    writer.write_all(b"WAVE")?;
    writer.write_all(b"fmt ")?;
    writer.write_all(&16_u32.to_le_bytes())?;
    writer.write_all(&1_u16.to_le_bytes())?;
    writer.write_all(&1_u16.to_le_bytes())?;
    writer.write_all(&TARGET_SAMPLE_RATE.to_le_bytes())?;
    writer.write_all(&(TARGET_SAMPLE_RATE * 2).to_le_bytes())?;
    writer.write_all(&2_u16.to_le_bytes())?;
    writer.write_all(&16_u16.to_le_bytes())?;
    writer.write_all(b"data")?;
    writer.write_all(&data_bytes.to_le_bytes())?;
    Ok(())
}

fn write_json_atomically(path: &Path, value: &impl Serialize) -> Result<()> {
    let temporary = path.with_extension("json.tmp");
    let mut file = open_private_file(&temporary)?;
    let payload = serde_json::to_vec_pretty(value).context("failed to serialize diagnostics")?;
    file.write_all(&payload)
        .with_context(|| format!("failed to write {}", temporary.display()))?;
    file.write_all(b"\n")
        .with_context(|| format!("failed to finish {}", temporary.display()))?;
    file.flush()
        .with_context(|| format!("failed to flush {}", temporary.display()))?;
    fs::rename(&temporary, path)
        .with_context(|| format!("failed to replace {}", path.display()))?;
    Ok(())
}

fn open_private_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options
        .open(path)
        .with_context(|| format!("failed to create {}", path.display()))
}

fn create_private_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path)
        .with_context(|| format!("failed to create diagnostics directory {}", path.display()))?;
    #[cfg(unix)]
    fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o700))
        .with_context(|| format!("failed to protect diagnostics directory {}", path.display()))?;
    Ok(())
}

fn prune_old_sessions(sessions_directory: &Path) {
    let entries = match fs::read_dir(sessions_directory) {
        Ok(entries) => entries,
        Err(error) => {
            warn!(
                directory = %sessions_directory.display(),
                %error,
                "failed to inspect diagnostic session retention"
            );
            return;
        }
    };
    let mut directories = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            entry
                .file_type()
                .ok()
                .filter(|kind| kind.is_dir())
                .map(|_| entry.path())
        })
        .collect::<Vec<_>>();
    directories.sort();

    let excess = directories.len().saturating_sub(SESSION_LIMIT);
    for directory in directories.into_iter().take(excess) {
        if let Err(error) = fs::remove_dir_all(&directory) {
            warn!(
                directory = %directory.display(),
                %error,
                "failed to remove an old diagnostic session"
            );
        } else {
            info!(directory = %directory.display(), "old diagnostic session removed");
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    #[test]
    fn wave_writer_produces_a_valid_pcm_header() {
        let directory =
            std::env::temp_dir().join(format!("voice-flow-wave-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("test.wav");
        let mut wave = WaveWriter::create(&path).unwrap();
        wave.write_samples(&[1, -2, 3]).unwrap();
        wave.checkpoint().unwrap();
        drop(wave);

        let mut bytes = Vec::new();
        File::open(&path).unwrap().read_to_end(&mut bytes).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 6);
        assert_eq!(bytes.len(), 50);

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn retention_keeps_only_the_newest_one_hundred_directories() {
        let directory =
            std::env::temp_dir().join(format!("voice-flow-retention-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        for index in 0..105 {
            fs::create_dir(directory.join(format!("{index:013}_session"))).unwrap();
        }

        prune_old_sessions(&directory);

        let mut names = fs::read_dir(&directory)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(names.len(), 100);
        assert_eq!(names.first().unwrap(), "0000000000005_session");

        fs::remove_dir_all(directory).unwrap();
    }
}
