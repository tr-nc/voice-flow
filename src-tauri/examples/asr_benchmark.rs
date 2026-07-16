#[path = "../src/asr_options.rs"]
mod asr_options;
#[path = "../src/asr/protocol.rs"]
mod protocol;

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use tokio::time::{sleep_until, timeout};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::Message;
use uuid::Uuid;

const SAMPLE_RATE: usize = 16_000;
const PACKET_SAMPLES: usize = 3_200;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const TRIM_WINDOW_SAMPLES: usize = SAMPLE_RATE / 50;
const TRIM_RELATIVE_RMS: f64 = 0.10;
const TRIM_MINIMUM_RMS: f64 = 50.0;

#[derive(Debug, Deserialize)]
struct BenchmarkCase {
    schema_version: u32,
    id: String,
    audio: PathBuf,
    language: String,
    expected_text: String,
}

#[derive(Debug, Clone, Copy)]
enum RecognitionMode {
    Current,
    Nostream,
}

impl RecognitionMode {
    fn all() -> [Self; 2] {
        [Self::Current, Self::Nostream]
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "current" => Ok(Self::Current),
            "nostream" => Ok(Self::Nostream),
            _ => bail!("unknown mode {value:?}; expected current, nostream, or all"),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Current => "current",
            Self::Nostream => "nostream",
        }
    }

    fn endpoint(self) -> &'static str {
        match self {
            Self::Current => asr_options::CURRENT_ENDPOINT,
            Self::Nostream => "wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_nostream",
        }
    }

    fn enable_nonstream(self) -> bool {
        matches!(self, Self::Current)
    }
}

struct RecognitionResult {
    text: String,
    connect_ms: u128,
    stream_ms: u128,
    total_ms: u128,
    packets: usize,
    first_text_ms: Option<u128>,
    first_text_lag_ms: Option<u128>,
    live_update_times_ms: Vec<u128>,
    live_lags_ms: Vec<u128>,
    stable_segments: Vec<StableSegment>,
    vad_end_window_ms: Option<u64>,
    log_id: String,
}

struct ExchangeResult {
    text: String,
    stream_ms: u128,
    first_text_ms: Option<u128>,
    first_text_lag_ms: Option<u128>,
    live_update_times_ms: Vec<u128>,
    live_lags_ms: Vec<u128>,
    stable_segments: Vec<StableSegment>,
}

#[derive(Clone)]
struct UtteranceObservation {
    start_ms: u64,
    end_ms: u64,
    text: String,
    definite: bool,
}

struct StableSegment {
    start_ms: u64,
    end_ms: u64,
    text: String,
    arrived_ms: u128,
    before_final_audio: bool,
}

struct ErrorBreakdown {
    substitutions: usize,
    insertions: usize,
    deletions: usize,
}

impl ErrorBreakdown {
    fn distance(&self) -> usize {
        self.substitutions + self.insertions + self.deletions
    }
}

struct Cli {
    input: PathBuf,
    mode: Option<RecognitionMode>,
    hotwords: Vec<String>,
    end_window_ms: Option<u64>,
    force_to_speech_ms: Option<u64>,
}

#[tokio::main]
async fn main() -> Result<()> {
    install_tls_provider()?;
    let cli = parse_args()?;
    let (case, audio_path) = load_case(&cli.input)?;
    let speech_pcm = trim_boundary_silence(decode_audio(&audio_path)?)?;
    let speech_duration_ms = speech_pcm.len() * 1_000 / SAMPLE_RATE;
    let pcm = add_edge_guards(speech_pcm);
    let duration_ms = pcm.len() * 1_000 / SAMPLE_RATE;
    let secret_key = load_secret_key()?;

    println!("benchmark: {}", case.id);
    println!("source: {}", audio_path.display());
    println!("language: {}", case.language);
    println!(
        "audio: {speech_duration_ms}ms edge-trimmed speech, {duration_ms}ms with {}ms production guards",
        asr_options::AUDIO_EDGE_GUARD_MS
    );
    println!("stream: {} samples at 16kHz mono s16le", pcm.len());
    println!("expected: {}", case.expected_text);
    println!("hotwords: {}", cli.hotwords.len());
    println!(
        "current_end_window_ms: {}",
        cli.end_window_ms.map_or_else(
            || format!("production-default({})", asr_options::VAD_END_WINDOW_MS),
            |value| value.to_string(),
        )
    );
    println!(
        "force_to_speech_ms: {}",
        cli.force_to_speech_ms
            .map_or_else(|| "provider-default".to_owned(), |value| value.to_string())
    );

    let modes = cli
        .mode
        .map_or_else(|| RecognitionMode::all().to_vec(), |mode| vec![mode]);
    for mode in modes {
        let result = recognize_pcm(
            &pcm,
            &secret_key,
            mode,
            &case.language,
            &cli.hotwords,
            cli.end_window_ms,
            cli.force_to_speech_ms,
        )
        .await?;
        print_result(mode, &case.expected_text, duration_ms, &result);
    }

    Ok(())
}

fn print_result(
    mode: RecognitionMode,
    expected_text: &str,
    audio_duration_ms: usize,
    result: &RecognitionResult,
) {
    let (errors, reference_len, cer) = character_error_rate(expected_text, &result.text);
    let accuracy_score = (100.0 - cer).clamp(0.0, 100.0);
    let live_gap_ms = result
        .live_update_times_ms
        .windows(2)
        .map(|times| times[1].saturating_sub(times[0]))
        .collect::<Vec<_>>();
    let live_lag_p50 = percentile(&result.live_lags_ms, 50);
    let live_lag_p95 = percentile(&result.live_lags_ms, 95);
    let live_score = match (result.first_text_lag_ms, live_lag_p95) {
        (Some(first), Some(p95)) => {
            Some((latency_score(first, 500, 2_500) + latency_score(p95, 500, 2_500)) / 2.0)
        }
        (Some(first), None) => Some(latency_score(first, 500, 2_500)),
        _ => None,
    };

    let stable_lags_ms = result
        .stable_segments
        .iter()
        .map(|segment| {
            segment
                .arrived_ms
                .saturating_sub(u128::from(stable_speech_end_ms(
                    segment,
                    result.vad_end_window_ms,
                )))
        })
        .collect::<Vec<_>>();
    let stable_lag_p50 = percentile(&stable_lags_ms, 50);
    let stable_lag_p95 = percentile(&stable_lags_ms, 95);
    let before_final_count = result
        .stable_segments
        .iter()
        .filter(|segment| segment.before_final_audio)
        .count();
    let stable_character_count = result
        .stable_segments
        .iter()
        .map(|segment| normalize_for_cer(&segment.text).len())
        .sum::<usize>();
    let before_final_character_count = result
        .stable_segments
        .iter()
        .filter(|segment| segment.before_final_audio)
        .map(|segment| normalize_for_cer(&segment.text).len())
        .sum::<usize>();
    let stable_coverage = if stable_character_count == 0 {
        0.0
    } else {
        (before_final_character_count as f64 / stable_character_count as f64 * 100.0)
            .clamp(0.0, 100.0)
    };
    let final_tail_ms = result.stream_ms.saturating_sub(audio_duration_ms as u128);
    let stable_score = stable_lag_p95.map(|p95| {
        latency_score(p95, 1_200, 4_000) * 0.5
            + stable_coverage * 0.3
            + latency_score(final_tail_ms, 500, 3_000) * 0.2
    });

    println!();
    println!("mode: {}", mode.name());
    println!("recognized: {}", result.text);
    println!("accuracy_score: {accuracy_score:.2}/100");
    println!(
        "accuracy: cer={cer:.2}% distance={}/{} substitutions={} insertions={} deletions={}",
        errors.distance(),
        reference_len,
        errors.substitutions,
        errors.insertions,
        errors.deletions
    );
    if matches!(mode, RecognitionMode::Current) {
        match live_score {
            Some(score) => println!("live_responsiveness_score: {score:.2}/100"),
            None => println!("live_responsiveness_score: n/a"),
        }
        println!(
            "live: first_text={} first_lag={} updates={} lag_p50={} lag_p95={} gap_p50={} gap_p95={}",
            display_ms(result.first_text_ms),
            display_ms(result.first_text_lag_ms),
            result.live_update_times_ms.len(),
            display_ms(live_lag_p50),
            display_ms(live_lag_p95),
            display_ms(percentile(&live_gap_ms, 50)),
            display_ms(percentile(&live_gap_ms, 95))
        );
    } else {
        println!("live_responsiveness_score: n/a (mode does not provide first-pass text)");
    }
    match stable_score {
        Some(score) => println!("stable_follow_score: {score:.2}/100"),
        None => println!("stable_follow_score: n/a"),
    }
    println!(
        "stable: segments={} before_final={} coverage={stable_coverage:.2}% lag_p50={} lag_p95={} final_tail={final_tail_ms}ms",
        result.stable_segments.len(),
        before_final_count,
        display_ms(stable_lag_p50),
        display_ms(stable_lag_p95)
    );
    println!(
        "timing: connect={}ms stream={}ms total={}ms packets={}",
        result.connect_ms, result.stream_ms, result.total_ms, result.packets
    );
    for (index, segment) in result.stable_segments.iter().enumerate() {
        println!(
            "stable_segment_{}: source={}..{}ms provider_end={}ms arrival={}ms lag={}ms before_final={} chars={}",
            index + 1,
            segment.start_ms,
            stable_speech_end_ms(segment, result.vad_end_window_ms),
            segment.end_ms,
            segment.arrived_ms,
            segment
                .arrived_ms
                .saturating_sub(u128::from(stable_speech_end_ms(
                    segment,
                    result.vad_end_window_ms
                ))),
            segment.before_final_audio,
            normalize_for_cer(&segment.text).len()
        );
    }
    if !result.log_id.is_empty() {
        println!("provider_log_id: {}", result.log_id);
    }
}

fn stable_speech_end_ms(segment: &StableSegment, vad_end_window_ms: Option<u64>) -> u64 {
    segment
        .end_ms
        .saturating_sub(vad_end_window_ms.unwrap_or(0))
}

fn display_ms(value: Option<u128>) -> String {
    value.map_or_else(|| "n/a".to_owned(), |value| format!("{value}ms"))
}

fn percentile(values: &[u128], percentile: usize) -> Option<u128> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let rank = (percentile * sorted.len()).div_ceil(100).saturating_sub(1);
    sorted.get(rank).copied()
}

fn latency_score(value_ms: u128, excellent_ms: u128, failing_ms: u128) -> f64 {
    if value_ms <= excellent_ms {
        return 100.0;
    }
    if value_ms >= failing_ms {
        return 0.0;
    }
    (failing_ms - value_ms) as f64 / (failing_ms - excellent_ms) as f64 * 100.0
}

fn parse_args() -> Result<Cli> {
    let mut args = std::env::args_os().skip(1);
    let mut input = None;
    let mut mode = None;
    let mut hotwords = Vec::new();
    let mut end_window_ms = None;
    let mut force_to_speech_ms = None;

    while let Some(argument) = args.next() {
        if argument == OsStr::new("--help") || argument == OsStr::new("-h") {
            print_help();
            std::process::exit(0);
        }
        if argument == OsStr::new("--mode") {
            let value = args.next().context("--mode requires a value")?;
            let value = value.to_str().context("--mode must be valid UTF-8")?;
            if value != "all" {
                mode = Some(RecognitionMode::parse(value)?);
            }
            continue;
        }
        if argument == OsStr::new("--hotword") {
            let value = args.next().context("--hotword requires a value")?;
            let value = value
                .to_str()
                .context("--hotword must be valid UTF-8")?
                .trim();
            if value.is_empty() {
                bail!("--hotword must not be empty");
            }
            hotwords.push(value.to_owned());
            continue;
        }
        if argument == OsStr::new("--end-window-ms") {
            let value = args.next().context("--end-window-ms requires a value")?;
            let value = value
                .to_str()
                .context("--end-window-ms must be valid UTF-8")?
                .parse::<u64>()
                .context("--end-window-ms must be an integer")?;
            if value < 200 {
                bail!("--end-window-ms must be at least 200");
            }
            end_window_ms = Some(value);
            continue;
        }
        if argument == OsStr::new("--force-to-speech-ms") {
            let value = args
                .next()
                .context("--force-to-speech-ms requires a value")?;
            let value = value
                .to_str()
                .context("--force-to-speech-ms must be valid UTF-8")?
                .parse::<u64>()
                .context("--force-to-speech-ms must be an integer")?;
            if value == 0 {
                bail!("--force-to-speech-ms must be at least 1");
            }
            force_to_speech_ms = Some(value);
            continue;
        }
        if input.replace(PathBuf::from(argument)).is_some() {
            bail!("only one benchmark directory or audio file may be provided");
        }
    }

    let input = input.context("missing benchmark directory or audio file; use --help for usage")?;
    if force_to_speech_ms.is_some() && end_window_ms.is_none() {
        bail!("--force-to-speech-ms requires --end-window-ms");
    }
    Ok(Cli {
        input,
        mode,
        hotwords,
        end_window_ms,
        force_to_speech_ms,
    })
}

fn print_help() {
    println!(
        "Voice Flow ASR benchmark\n\n\
Usage:\n  cargo run --manifest-path src-tauri/Cargo.toml --example asr_benchmark -- \\\n    examples/benchmarks/code-switch-001-normal [--mode all|current|nostream] \\\n    [--hotword WORD]... [--end-window-ms MILLISECONDS] \\
    [--force-to-speech-ms MILLISECONDS]\n\n\
The input may be a benchmark directory containing benchmark.json, or a directly\n\
decodable audio file accompanied by a sibling benchmark.json. ffmpeg must be installed.\n\
The Secret Key is read from VOICE_FLOW_SECRET_KEY or the local Voice Flow settings file."
    );
}

fn load_case(input: &Path) -> Result<(BenchmarkCase, PathBuf)> {
    let (manifest_path, base_dir) = if input.is_dir() {
        (input.join("benchmark.json"), input.to_path_buf())
    } else {
        let parent = input
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        (parent.join("benchmark.json"), parent.to_path_buf())
    };
    let contents = fs::read_to_string(&manifest_path)
        .with_context(|| format!("failed to read {}", manifest_path.display()))?;
    let case: BenchmarkCase = serde_json::from_str(&contents)
        .with_context(|| format!("failed to parse {}", manifest_path.display()))?;
    if case.schema_version != 1 {
        bail!(
            "unsupported benchmark schema version {} in {}",
            case.schema_version,
            manifest_path.display()
        );
    }
    if case.expected_text.trim().is_empty() {
        bail!("benchmark expected_text must not be empty");
    }

    let audio_path = if input.is_file() {
        input.to_path_buf()
    } else {
        base_dir.join(&case.audio)
    };
    if !audio_path.is_file() {
        bail!("benchmark audio is missing: {}", audio_path.display());
    }
    Ok((case, audio_path))
}

fn decode_audio(path: &Path) -> Result<Vec<i16>> {
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-i"])
        .arg(path)
        .args([
            "-map",
            "0:a:0",
            "-vn",
            "-ac",
            "1",
            "-ar",
            "16000",
            "-c:a",
            "pcm_s16le",
            "-f",
            "s16le",
            "pipe:1",
        ])
        .output()
        .context("failed to run ffmpeg; install it before running audio benchmarks")?;
    if !output.status.success() {
        bail!(
            "ffmpeg could not decode {}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    if output.stdout.is_empty() {
        bail!("ffmpeg decoded no audio from {}", path.display());
    }
    if output.stdout.len() % 2 != 0 {
        bail!("ffmpeg returned an incomplete 16-bit PCM sample");
    }

    Ok(output
        .stdout
        .chunks_exact(2)
        .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]))
        .collect())
}

fn trim_boundary_silence(pcm: Vec<i16>) -> Result<Vec<i16>> {
    let rms_values = pcm
        .chunks(TRIM_WINDOW_SAMPLES)
        .map(|window| {
            let mean_square = window
                .iter()
                .map(|sample| f64::from(*sample).powi(2))
                .sum::<f64>()
                / window.len() as f64;
            mean_square.sqrt()
        })
        .collect::<Vec<_>>();
    let maximum_rms = rms_values.iter().copied().fold(0.0_f64, f64::max);
    let threshold = (maximum_rms * TRIM_RELATIVE_RMS).max(TRIM_MINIMUM_RMS);
    let first_active = rms_values
        .iter()
        .position(|rms| *rms >= threshold)
        .context("benchmark audio contains no detectable speech")?;
    let last_active = rms_values
        .iter()
        .rposition(|rms| *rms >= threshold)
        .expect("an active window was already found");
    let start = first_active * TRIM_WINDOW_SAMPLES;
    let end = ((last_active + 1) * TRIM_WINDOW_SAMPLES).min(pcm.len());
    Ok(pcm[start..end].to_vec())
}

fn add_edge_guards(pcm: Vec<i16>) -> Vec<i16> {
    let guard_samples = SAMPLE_RATE * asr_options::AUDIO_EDGE_GUARD_MS / 1_000;
    let mut guarded = Vec::with_capacity(pcm.len() + guard_samples * 2);
    guarded.resize(guard_samples, 0);
    guarded.extend(pcm);
    guarded.resize(guarded.len() + guard_samples, 0);
    guarded
}

fn load_secret_key() -> Result<String> {
    if let Ok(value) = std::env::var("VOICE_FLOW_SECRET_KEY") {
        let value = value.trim().to_owned();
        if !value.is_empty() {
            return Ok(value);
        }
    }

    let path = local_settings_path()?;
    let contents = fs::read_to_string(&path).with_context(|| {
        format!(
            "failed to read local Voice Flow settings from {}",
            path.display()
        )
    })?;
    let settings: serde_json::Value = serde_json::from_str(&contents).with_context(|| {
        format!(
            "failed to parse local Voice Flow settings from {}",
            path.display()
        )
    })?;
    let secret_key = settings
        .get("secret_key")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_owned();
    if secret_key.is_empty() {
        bail!("the local Voice Flow settings do not contain a Secret Key");
    }
    Ok(secret_key)
}

fn local_settings_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is unavailable")?;

    #[cfg(target_os = "macos")]
    return Ok(
        PathBuf::from(home).join("Library/Application Support/dev.voiceflow.desktop/settings.json")
    );

    #[cfg(target_os = "linux")]
    {
        let config_directory = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(home).join(".config"));
        Ok(config_directory.join("dev.voiceflow.desktop/settings.json"))
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    bail!("loading Voice Flow settings is not supported on this platform")
}

fn install_tls_provider() -> Result<()> {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        rustls::crypto::ring::default_provider()
            .install_default()
            .map_err(|_| anyhow::anyhow!("failed to install the rustls ring crypto provider"))?;
    }
    Ok(())
}

async fn recognize_pcm(
    pcm: &[i16],
    secret_key: &str,
    mode: RecognitionMode,
    language: &str,
    hotwords: &[String],
    end_window_ms: Option<u64>,
    force_to_speech_ms: Option<u64>,
) -> Result<RecognitionResult> {
    let started = Instant::now();
    let request_id = Uuid::new_v4().to_string();
    let mut request = mode
        .endpoint()
        .into_client_request()
        .context("failed to create the ASR WebSocket request")?;
    let headers = request.headers_mut();
    headers.insert(
        "x-api-resource-id",
        HeaderValue::from_static(asr_options::RESOURCE_ID),
    );
    headers.insert(
        "x-api-connect-id",
        HeaderValue::from_str(&request_id).expect("UUID is a valid header"),
    );
    headers.insert(
        "x-api-request-id",
        HeaderValue::from_str(&request_id).expect("UUID is a valid header"),
    );
    headers.insert("x-api-sequence", HeaderValue::from_static("-1"));
    headers.insert(
        "x-api-key",
        HeaderValue::from_str(secret_key).context("Secret Key is not a valid header")?,
    );

    let connect_started = Instant::now();
    let (websocket, response) = timeout(CONNECT_TIMEOUT, connect_async(request))
        .await
        .context("timed out connecting to ASR")?
        .context("failed to connect to ASR")?;
    let connect_ms = connect_started.elapsed().as_millis();
    let log_id = response
        .headers()
        .get("x-tt-logid")
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
        .to_owned();
    let (mut writer, mut reader) = websocket.split();

    let end_window_ms = matches!(mode, RecognitionMode::Current)
        .then_some(end_window_ms.unwrap_or(asr_options::VAD_END_WINDOW_MS));
    let mut request_settings = json!({
        "model_name": "bigmodel",
        "enable_itn": true,
        "enable_punc": true,
        "enable_ddc": false,
        "show_utterances": true,
        "result_type": "full"
    });
    if mode.enable_nonstream() {
        request_settings["enable_nonstream"] = json!(true);
        if let Some(value) = end_window_ms {
            request_settings["end_window_size"] = json!(value);
        }
        if let Some(value) = force_to_speech_ms {
            request_settings["force_to_speech_time"] = json!(value);
        }
    }
    if !hotwords.is_empty() {
        let context = json!({
            "hotwords": hotwords
                .iter()
                .map(|word| json!({ "word": word }))
                .collect::<Vec<_>>()
        });
        request_settings["corpus"] = json!({ "context": serde_json::to_string(&context)? });
    }
    let mut audio_settings = json!({
        "format": "pcm",
        "codec": "raw",
        "rate": SAMPLE_RATE,
        "bits": 16,
        "channel": 1
    });
    if matches!(mode, RecognitionMode::Nostream) && !language.is_empty() {
        audio_settings["language"] = json!(language);
    }
    let request_payload = json!({
        "user": { "uid": "voice-flow-benchmark" },
        "audio": audio_settings,
        "request": request_settings
    });
    send_message(
        &mut writer,
        Message::Binary(protocol::full_request(1, &request_payload)?.into()),
    )
    .await?;

    let packet_count = pcm.len().div_ceil(PACKET_SAMPLES).max(1);
    let response_timeout = Duration::from_secs_f64((pcm.len() as f64 / SAMPLE_RATE as f64) + 20.0)
        .max(Duration::from_secs(30));
    let stream_started = Instant::now();
    let pacing_started = tokio::time::Instant::now();
    let exchange = async {
        let mut packet_index = 0;
        let mut next_send = pacing_started + sample_duration(PACKET_SAMPLES.min(pcm.len()));
        let mut final_audio_sent = false;
        let mut final_text = String::new();
        let mut first_text_ms = None;
        let mut first_text_lag_ms = None;
        let mut live_update_times_ms = Vec::new();
        let mut live_lags_ms = Vec::new();
        let mut stable_segments = BTreeMap::<(u64, u64), StableSegment>::new();

        loop {
            tokio::select! {
                _ = sleep_until(next_send), if packet_index < packet_count => {
                    let start = packet_index * PACKET_SAMPLES;
                    let end = ((packet_index + 1) * PACKET_SAMPLES).min(pcm.len());
                    let samples = pcm.get(start..end).unwrap_or_default();
                    let is_last = packet_index + 1 == packet_count;
                    send_audio(&mut writer, 2 + packet_index as i32, samples, is_last).await?;
                    packet_index += 1;
                    final_audio_sent = is_last;
                    if packet_index < packet_count {
                        let next_end = ((packet_index + 1) * PACKET_SAMPLES).min(pcm.len());
                        next_send = pacing_started + sample_duration(next_end);
                    }
                }
                message = reader.next() => {
                    match message {
                        Some(Ok(Message::Binary(data))) => {
                            let arrived_ms = stream_started.elapsed().as_millis();
                            let frame = protocol::parse_server_frame(data.as_ref())?;
                            if let Some(payload) = frame.payload.as_ref() {
                                let utterances = extract_utterances(payload);
                                if let Some(text) = protocol::extract_text(Some(payload))
                                    && text != final_text
                                {
                                    first_text_ms.get_or_insert(arrived_ms);
                                    let source_end_ms = utterances
                                        .iter()
                                        .map(|utterance| utterance.end_ms)
                                        .max()
                                        .map(u128::from);
                                    if first_text_lag_ms.is_none() {
                                        first_text_lag_ms = source_end_ms
                                            .map(|end_ms| arrived_ms.saturating_sub(end_ms));
                                    }
                                    let has_provisional = utterances.is_empty()
                                        || utterances.iter().any(|utterance| !utterance.definite);
                                    if has_provisional {
                                        live_update_times_ms.push(arrived_ms);
                                        if let Some(end_ms) = source_end_ms {
                                            live_lags_ms.push(arrived_ms.saturating_sub(end_ms));
                                        }
                                    }
                                    final_text = text;
                                }

                                for utterance in utterances
                                    .into_iter()
                                    .filter(|utterance| utterance.definite && !utterance.text.is_empty())
                                {
                                    stable_segments
                                        .entry((utterance.start_ms, utterance.end_ms))
                                        .and_modify(|segment| segment.text.clone_from(&utterance.text))
                                        .or_insert(StableSegment {
                                            start_ms: utterance.start_ms,
                                            end_ms: utterance.end_ms,
                                            text: utterance.text,
                                            arrived_ms,
                                            before_final_audio: !final_audio_sent,
                                        });
                                }
                            }
                            if frame.is_last {
                                if packet_index < packet_count {
                                    bail!("ASR ended before all benchmark audio was sent");
                                }
                                return Ok::<ExchangeResult, anyhow::Error>(ExchangeResult {
                                    text: final_text,
                                    stream_ms: stream_started.elapsed().as_millis(),
                                    first_text_ms,
                                    first_text_lag_ms,
                                    live_update_times_ms,
                                    live_lags_ms,
                                    stable_segments: stable_segments.into_values().collect(),
                                });
                            }
                        }
                        Some(Ok(Message::Ping(payload))) => {
                            send_message(&mut writer, Message::Pong(payload)).await?;
                        }
                        Some(Ok(Message::Close(frame))) => {
                            bail!("ASR WebSocket closed before the final result: {frame:?}");
                        }
                        Some(Ok(_)) => {}
                        Some(Err(error)) => return Err(error).context("ASR WebSocket receive failed"),
                        None => bail!("ASR WebSocket ended before the final result"),
                    }
                }
            }
        }
    };
    let exchange = timeout(response_timeout, exchange)
        .await
        .context("timed out waiting for the benchmark ASR result")??;

    Ok(RecognitionResult {
        text: exchange.text,
        connect_ms,
        stream_ms: exchange.stream_ms,
        total_ms: started.elapsed().as_millis(),
        packets: packet_count,
        first_text_ms: exchange.first_text_ms,
        first_text_lag_ms: exchange.first_text_lag_ms,
        live_update_times_ms: exchange.live_update_times_ms,
        live_lags_ms: exchange.live_lags_ms,
        stable_segments: exchange.stable_segments,
        vad_end_window_ms: end_window_ms,
        log_id,
    })
}

fn sample_duration(samples: usize) -> Duration {
    Duration::from_secs_f64(samples as f64 / SAMPLE_RATE as f64)
}

fn extract_utterances(payload: &serde_json::Value) -> Vec<UtteranceObservation> {
    payload
        .pointer("/result/utterances")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .map(|utterance| UtteranceObservation {
            start_ms: utterance
                .get("start_time")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
            end_ms: utterance
                .get("end_time")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
            text: utterance
                .get("text")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            definite: utterance
                .get("definite")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false),
        })
        .collect()
}

async fn send_audio<S>(writer: &mut S, sequence: i32, samples: &[i16], is_last: bool) -> Result<()>
where
    S: futures_util::Sink<Message> + Unpin,
    S::Error: std::error::Error + Send + Sync + 'static,
{
    let pcm = samples
        .iter()
        .flat_map(|sample| sample.to_le_bytes())
        .collect::<Vec<_>>();
    send_message(
        writer,
        Message::Binary(protocol::audio_request(sequence, &pcm, is_last)?.into()),
    )
    .await
}

async fn send_message<S>(writer: &mut S, message: Message) -> Result<()>
where
    S: futures_util::Sink<Message> + Unpin,
    S::Error: std::error::Error + Send + Sync + 'static,
{
    timeout(WRITE_TIMEOUT, writer.send(message))
        .await
        .context("timed out writing to the ASR WebSocket")?
        .context("failed to write to the ASR WebSocket")
}

fn character_error_rate(expected: &str, actual: &str) -> (ErrorBreakdown, usize, f64) {
    let expected = normalize_for_cer(expected);
    let actual = normalize_for_cer(actual);
    let errors = error_breakdown(&expected, &actual);
    let reference_len = expected.len();
    let percentage = if reference_len == 0 {
        0.0
    } else {
        errors.distance() as f64 / reference_len as f64 * 100.0
    };
    (errors, reference_len, percentage)
}

fn normalize_for_cer(value: &str) -> Vec<char> {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|character| character.is_alphanumeric())
        .collect()
}

fn error_breakdown(expected: &[char], actual: &[char]) -> ErrorBreakdown {
    let mut distances = vec![vec![0; actual.len() + 1]; expected.len() + 1];
    for (index, row) in distances.iter_mut().enumerate() {
        row[0] = index;
    }
    for (index, value) in distances[0].iter_mut().enumerate() {
        *value = index;
    }

    for expected_index in 1..=expected.len() {
        for actual_index in 1..=actual.len() {
            let substitution = distances[expected_index - 1][actual_index - 1]
                + usize::from(expected[expected_index - 1] != actual[actual_index - 1]);
            let deletion = distances[expected_index - 1][actual_index] + 1;
            let insertion = distances[expected_index][actual_index - 1] + 1;
            distances[expected_index][actual_index] = substitution.min(deletion).min(insertion);
        }
    }

    let mut expected_index = expected.len();
    let mut actual_index = actual.len();
    let mut errors = ErrorBreakdown {
        substitutions: 0,
        insertions: 0,
        deletions: 0,
    };
    while expected_index > 0 || actual_index > 0 {
        if expected_index > 0
            && actual_index > 0
            && expected[expected_index - 1] == actual[actual_index - 1]
        {
            expected_index -= 1;
            actual_index -= 1;
        } else if expected_index > 0
            && actual_index > 0
            && distances[expected_index][actual_index]
                == distances[expected_index - 1][actual_index - 1] + 1
        {
            errors.substitutions += 1;
            expected_index -= 1;
            actual_index -= 1;
        } else if expected_index > 0
            && distances[expected_index][actual_index]
                == distances[expected_index - 1][actual_index] + 1
        {
            errors.deletions += 1;
            expected_index -= 1;
        } else {
            errors.insertions += 1;
            actual_index -= 1;
        }
    }
    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cer_ignores_case_spacing_and_punctuation() {
        let (errors, reference_len, percentage) =
            character_error_rate("Voice Flow，测试。", "voiceflow测试");
        assert_eq!(errors.distance(), 0);
        assert_eq!(reference_len, 11);
        assert_eq!(percentage, 0.0);
    }

    #[test]
    fn edit_distance_counts_a_substitution() {
        let errors = error_breakdown(&['声', '音'], &['生', '音']);
        assert_eq!(errors.distance(), 1);
        assert_eq!(errors.substitutions, 1);
        assert_eq!(errors.insertions, 0);
        assert_eq!(errors.deletions, 0);
    }

    #[test]
    fn percentile_uses_nearest_rank() {
        assert_eq!(percentile(&[40, 10, 30, 20], 50), Some(20));
        assert_eq!(percentile(&[40, 10, 30, 20], 95), Some(40));
        assert_eq!(percentile(&[], 95), None);
    }

    #[test]
    fn latency_score_respects_thresholds() {
        assert_eq!(latency_score(400, 500, 2_500), 100.0);
        assert_eq!(latency_score(2_500, 500, 2_500), 0.0);
        assert_eq!(latency_score(1_500, 500, 2_500), 50.0);
    }

    #[test]
    fn boundary_trimming_removes_silence_without_touching_speech() {
        let mut pcm = vec![0; TRIM_WINDOW_SAMPLES * 2];
        pcm.extend(vec![1_000; TRIM_WINDOW_SAMPLES * 2]);
        pcm.extend(vec![0; TRIM_WINDOW_SAMPLES * 3]);

        assert_eq!(
            trim_boundary_silence(pcm).unwrap(),
            vec![1_000; TRIM_WINDOW_SAMPLES * 2]
        );
    }

    #[test]
    fn edge_guards_wrap_speech_without_changing_it() {
        let speech = vec![11, 22, 33];
        let guarded = add_edge_guards(speech.clone());
        let guard_samples = SAMPLE_RATE * asr_options::AUDIO_EDGE_GUARD_MS / 1_000;

        assert_eq!(
            &guarded[guard_samples..guard_samples + speech.len()],
            &speech
        );
        assert!(guarded[..guard_samples].iter().all(|sample| *sample == 0));
        assert!(
            guarded[guard_samples + speech.len()..]
                .iter()
                .all(|sample| *sample == 0)
        );
    }
}
