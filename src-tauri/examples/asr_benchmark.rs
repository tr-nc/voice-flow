#[path = "../src/asr/protocol.rs"]
mod protocol;

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

const RESOURCE_ID: &str = "volc.seedasr.sauc.duration";
const SAMPLE_RATE: usize = 16_000;
const PACKET_SAMPLES: usize = 3_200;
const PACKET_INTERVAL: Duration = Duration::from_millis(200);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const WRITE_TIMEOUT: Duration = Duration::from_secs(5);

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
    Legacy,
    Current,
    Nostream,
}

impl RecognitionMode {
    fn all() -> [Self; 3] {
        [Self::Legacy, Self::Current, Self::Nostream]
    }

    fn parse(value: &str) -> Result<Self> {
        match value {
            "legacy" => Ok(Self::Legacy),
            "current" | "optimized" => Ok(Self::Current),
            "nostream" => Ok(Self::Nostream),
            _ => bail!("unknown mode {value:?}; expected legacy, current, nostream, or all"),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Legacy => "legacy",
            Self::Current => "current",
            Self::Nostream => "nostream",
        }
    }

    fn endpoint(self) -> &'static str {
        match self {
            Self::Legacy => "wss://openspeech.bytedance.com/api/v3/sauc/bigmodel",
            Self::Current => "wss://openspeech.bytedance.com/api/v3/sauc/bigmodel_async",
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
    total_ms: u128,
    packets: usize,
    saw_definite_segment: bool,
    log_id: String,
}

struct Cli {
    input: PathBuf,
    mode: Option<RecognitionMode>,
    hotwords: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    install_tls_provider()?;
    let cli = parse_args()?;
    let (case, audio_path) = load_case(&cli.input)?;
    let pcm = decode_audio(&audio_path)?;
    let duration_ms = pcm.len() * 1_000 / SAMPLE_RATE;
    let secret_key = load_secret_key()?;

    println!("benchmark: {}", case.id);
    println!("source: {}", audio_path.display());
    println!("language: {}", case.language);
    println!(
        "audio: {duration_ms}ms, {} samples at 16kHz mono s16le",
        pcm.len()
    );
    println!("expected: {}", case.expected_text);
    println!("hotwords: {}", cli.hotwords.len());

    let modes = cli
        .mode
        .map_or_else(|| RecognitionMode::all().to_vec(), |mode| vec![mode]);
    for mode in modes {
        let result = recognize_pcm(&pcm, &secret_key, mode, &case.language, &cli.hotwords).await?;
        let (distance, reference_len, cer) =
            character_error_rate(&case.expected_text, &result.text);
        println!();
        println!("mode: {}", mode.name());
        println!("recognized: {}", result.text);
        println!("cer: {cer:.2}% ({distance}/{reference_len})");
        println!(
            "timing: connect={}ms total={}ms packets={}",
            result.connect_ms, result.total_ms, result.packets
        );
        println!("definite_segment: {}", result.saw_definite_segment);
        if !result.log_id.is_empty() {
            println!("provider_log_id: {}", result.log_id);
        }
    }

    Ok(())
}

fn parse_args() -> Result<Cli> {
    let mut args = std::env::args_os().skip(1);
    let mut input = None;
    let mut mode = None;
    let mut hotwords = Vec::new();

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
        if input.replace(PathBuf::from(argument)).is_some() {
            bail!("only one benchmark directory or audio file may be provided");
        }
    }

    let input = input.context("missing benchmark directory or audio file; use --help for usage")?;
    Ok(Cli {
        input,
        mode,
        hotwords,
    })
}

fn print_help() {
    println!(
        "Voice Flow ASR benchmark\n\n\
Usage:\n  cargo run --manifest-path src-tauri/Cargo.toml --example asr_benchmark -- \\\n    examples/benchmarks/mandarin-basic-001 [--mode all|legacy|current|nostream] \\\n    [--hotword WORD]...\n\n\
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

fn load_secret_key() -> Result<String> {
    if let Ok(value) = std::env::var("VOICE_FLOW_SECRET_KEY") {
        let value = value.trim().to_owned();
        if !value.is_empty() {
            return Ok(value);
        }
    }

    let home = std::env::var_os("HOME").context("HOME is unavailable")?;
    let path =
        PathBuf::from(home).join("Library/Application Support/dev.voiceflow.desktop/settings.json");
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
) -> Result<RecognitionResult> {
    let started = Instant::now();
    let request_id = Uuid::new_v4().to_string();
    let mut request = mode
        .endpoint()
        .into_client_request()
        .context("failed to create the ASR WebSocket request")?;
    let headers = request.headers_mut();
    headers.insert("x-api-resource-id", HeaderValue::from_static(RESOURCE_ID));
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
    let exchange = async {
        let mut packet_index = 0;
        let mut next_send = tokio::time::Instant::now();
        let mut final_text = String::new();
        let mut saw_definite_segment = false;

        loop {
            tokio::select! {
                _ = sleep_until(next_send), if packet_index < packet_count => {
                    let start = packet_index * PACKET_SAMPLES;
                    let end = ((packet_index + 1) * PACKET_SAMPLES).min(pcm.len());
                    let samples = pcm.get(start..end).unwrap_or_default();
                    let is_last = packet_index + 1 == packet_count;
                    send_audio(&mut writer, 2 + packet_index as i32, samples, is_last).await?;
                    packet_index += 1;
                    next_send += PACKET_INTERVAL;
                }
                message = reader.next() => {
                    match message {
                        Some(Ok(Message::Binary(data))) => {
                            let frame = protocol::parse_server_frame(data.as_ref())?;
                            if let Some(payload) = frame.payload.as_ref() {
                                if let Some(text) = protocol::extract_text(Some(payload)) {
                                    final_text = text;
                                }
                                saw_definite_segment |= payload
                                    .pointer("/result/utterances")
                                    .and_then(serde_json::Value::as_array)
                                    .is_some_and(|utterances| {
                                        utterances.iter().any(|utterance| {
                                            utterance.get("definite")
                                                .and_then(serde_json::Value::as_bool)
                                                .unwrap_or(false)
                                        })
                                    });
                            }
                            if frame.is_last {
                                if packet_index < packet_count {
                                    bail!("ASR ended before all benchmark audio was sent");
                                }
                                return Ok::<(String, bool), anyhow::Error>((
                                    final_text,
                                    saw_definite_segment,
                                ));
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
    let (text, saw_definite_segment) = timeout(response_timeout, exchange)
        .await
        .context("timed out waiting for the benchmark ASR result")??;

    Ok(RecognitionResult {
        text,
        connect_ms,
        total_ms: started.elapsed().as_millis(),
        packets: packet_count,
        saw_definite_segment,
        log_id,
    })
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

fn character_error_rate(expected: &str, actual: &str) -> (usize, usize, f64) {
    let expected = normalize_for_cer(expected);
    let actual = normalize_for_cer(actual);
    let distance = edit_distance(&expected, &actual);
    let reference_len = expected.len();
    let percentage = if reference_len == 0 {
        0.0
    } else {
        distance as f64 / reference_len as f64 * 100.0
    };
    (distance, reference_len, percentage)
}

fn normalize_for_cer(value: &str) -> Vec<char> {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|character| character.is_alphanumeric())
        .collect()
}

fn edit_distance(left: &[char], right: &[char]) -> usize {
    let mut previous = (0..=right.len()).collect::<Vec<_>>();
    let mut current = vec![0; right.len() + 1];

    for (left_index, left_character) in left.iter().enumerate() {
        current[0] = left_index + 1;
        for (right_index, right_character) in right.iter().enumerate() {
            let substitution =
                previous[right_index] + usize::from(left_character != right_character);
            let insertion = current[right_index] + 1;
            let deletion = previous[right_index + 1] + 1;
            current[right_index + 1] = substitution.min(insertion).min(deletion);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cer_ignores_case_spacing_and_punctuation() {
        let (distance, reference_len, percentage) =
            character_error_rate("Voice Flow，测试。", "voiceflow测试");
        assert_eq!(distance, 0);
        assert_eq!(reference_len, 11);
        assert_eq!(percentage, 0.0);
    }

    #[test]
    fn edit_distance_counts_a_substitution() {
        assert_eq!(edit_distance(&['声', '音'], &['生', '音']), 1);
    }
}
