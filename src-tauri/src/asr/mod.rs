mod protocol;

use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio::sync::{mpsc, oneshot};
use tokio::time::timeout;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::protocol::Message;
use tracing::{debug, info, warn};

use crate::asr_options::{AUDIO_EDGE_GUARD_MS, CURRENT_ENDPOINT, RESOURCE_ID, VAD_END_WINDOW_MS};
use crate::audio::{AudioCapture, AudioEvent, TARGET_SAMPLE_RATE};
use crate::config::AppConfig;

pub use protocol::{TranscriptSegment, TranscriptUpdate};

const AUDIO_PACKET_SAMPLES: usize = 3_200;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const SOCKET_WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const FINAL_RESPONSE_TIMEOUT: Duration = Duration::from_secs(6);
pub const CAPTURE_TAIL_MS: u64 = 250;
const CAPTURE_TAIL_DURATION: Duration = Duration::from_millis(CAPTURE_TAIL_MS);

#[derive(Debug)]
pub enum StreamEvent {
    Connected,
    Transcript(TranscriptUpdate),
}

pub fn install_tls_provider() -> Result<()> {
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        rustls::crypto::ring::default_provider()
            .install_default()
            .map_err(|_| anyhow::anyhow!("failed to install the rustls ring crypto provider"))?;
    }
    info!(provider = "ring", "TLS crypto provider ready");
    Ok(())
}

pub async fn recognize(
    config: AppConfig,
    session_id: String,
    stop_receiver: oneshot::Receiver<()>,
    events: mpsc::UnboundedSender<StreamEvent>,
    capture: AudioCapture,
    mut audio_receiver: mpsc::UnboundedReceiver<AudioEvent>,
) -> Result<String> {
    config.validate_for_dictation()?;
    info!(
        endpoint = CURRENT_ENDPOINT,
        resource_id = RESOURCE_ID,
        "preparing streaming ASR session"
    );

    let mut stop_receiver = stop_receiver;
    let request_id = session_id;
    let mut request = CURRENT_ENDPOINT
        .into_client_request()
        .context("failed to create the VolcEngine WebSocket request")?;
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
        HeaderValue::from_str(&config.secret_key).context("Secret Key is not a valid header")?,
    );

    let mut capture = Some(capture);
    let mut stop_requested = false;
    let connect_started = Instant::now();
    let connection = timeout(CONNECT_TIMEOUT, connect_async(request));
    tokio::pin!(connection);
    let connection = loop {
        tokio::select! {
            result = &mut connection => break result,
            _ = &mut stop_receiver, if !stop_requested => {
                stop_requested = true;
                info!("dictation stopped while ASR was connecting; preserving captured audio");
                finish_capture(&mut capture).await;
            }
        }
    };
    let (websocket, response) = connection
        .context("timed out while connecting to VolcEngine ASR")?
        .context("failed to connect to VolcEngine ASR")?;
    info!(
        request_id,
        elapsed_ms = connect_started.elapsed().as_millis(),
        status = %response.status(),
        "ASR WebSocket connected"
    );
    let (mut writer, mut reader) = websocket.split();

    let request_payload = request_payload();
    timeout(
        SOCKET_WRITE_TIMEOUT,
        writer.send(Message::Binary(
            protocol::full_request(1, &request_payload)?.into(),
        )),
    )
    .await
    .context("timed out sending the initial ASR request")?
    .context("failed to send the initial ASR request")?;
    debug!(sequence = 1, "initial ASR request sent");
    let _ = events.send(StreamEvent::Connected);

    let edge_guard_samples = TARGET_SAMPLE_RATE as usize * AUDIO_EDGE_GUARD_MS / 1_000;
    let mut sequence = 2;
    let mut pending_samples = vec![0; edge_guard_samples];
    let mut latest_transcript = TranscriptUpdate::default();

    if !stop_requested {
        loop {
            tokio::select! {
                _ = &mut stop_receiver => {
                    stop_requested = true;
                    break;
                }
                audio_event = audio_receiver.recv() => {
                    match audio_event {
                        Some(AudioEvent::Data(chunk)) => {
                            pending_samples.extend_from_slice(&chunk.samples);
                            while pending_samples.len() >= AUDIO_PACKET_SAMPLES {
                                let samples: Vec<i16> = pending_samples.drain(..AUDIO_PACKET_SAMPLES).collect();
                                send_audio(&mut writer, sequence, &samples, false).await?;
                                sequence += 1;
                            }
                        }
                        Some(AudioEvent::Error(error)) => bail!("microphone capture failed: {error}"),
                        None => bail!("microphone capture stopped unexpectedly"),
                    }
                }
                message = reader.next() => {
                    if handle_message(message, &mut writer, &events, &mut latest_transcript).await? {
                        return Ok(latest_transcript.text);
                    }
                }
            }
        }
    }

    if stop_requested {
        finish_capture(&mut capture).await;
    } else {
        drop(capture.take());
    }
    while let Ok(audio_event) = audio_receiver.try_recv() {
        match audio_event {
            AudioEvent::Data(chunk) => pending_samples.extend_from_slice(&chunk.samples),
            AudioEvent::Error(error) => bail!("microphone capture failed: {error}"),
        }
    }
    pending_samples.resize(pending_samples.len() + edge_guard_samples, 0);

    while pending_samples.len() > AUDIO_PACKET_SAMPLES {
        let samples: Vec<i16> = pending_samples.drain(..AUDIO_PACKET_SAMPLES).collect();
        send_audio(&mut writer, sequence, &samples, false).await?;
        sequence += 1;
    }
    send_audio(&mut writer, sequence, &pending_samples, true).await?;

    let final_wait_started = Instant::now();
    let final_response = timeout(FINAL_RESPONSE_TIMEOUT, async {
        loop {
            let message = reader.next().await;
            if handle_message(message, &mut writer, &events, &mut latest_transcript).await? {
                return Ok::<(), anyhow::Error>(());
            }
        }
    })
    .await;
    match final_response {
        Ok(result) => result?,
        Err(_) if !latest_transcript.text.is_empty() => {
            warn!(
                wait_ms = final_wait_started.elapsed().as_millis(),
                "final ASR marker timed out; using the latest transcript"
            );
        }
        Err(_) => bail!("timed out waiting for the final ASR response"),
    }
    info!(
        final_wait_ms = final_wait_started.elapsed().as_millis(),
        characters = latest_transcript.text.chars().count(),
        "ASR session finalized"
    );

    Ok(latest_transcript.text)
}

async fn finish_capture(capture: &mut Option<AudioCapture>) {
    if capture.is_some() {
        tokio::time::sleep(CAPTURE_TAIL_DURATION).await;
        drop(capture.take());
    }
}

fn request_payload() -> serde_json::Value {
    json!({
        "user": { "uid": "voice-flow" },
        "audio": {
            "format": "pcm",
            "codec": "raw",
            "rate": TARGET_SAMPLE_RATE,
            "bits": 16,
            "channel": 1
        },
        "request": {
            "model_name": "bigmodel",
            "enable_nonstream": true,
            "end_window_size": VAD_END_WINDOW_MS,
            "enable_itn": true,
            "enable_punc": true,
            "enable_ddc": false,
            "show_utterances": true,
            "result_type": "full"
        }
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
    debug!(
        sequence,
        samples = samples.len(),
        pcm_bytes = pcm.len(),
        is_last,
        "sending ASR audio packet"
    );
    timeout(
        SOCKET_WRITE_TIMEOUT,
        writer.send(Message::Binary(
            protocol::audio_request(sequence, &pcm, is_last)?.into(),
        )),
    )
    .await
    .context("timed out streaming microphone audio to VolcEngine")?
    .context("failed to stream microphone audio to VolcEngine")
}

async fn handle_message<S>(
    message: Option<Result<Message, tokio_tungstenite::tungstenite::Error>>,
    writer: &mut S,
    events: &mpsc::UnboundedSender<StreamEvent>,
    latest_transcript: &mut TranscriptUpdate,
) -> Result<bool>
where
    S: futures_util::Sink<Message> + Unpin,
    S::Error: std::error::Error + Send + Sync + 'static,
{
    match message {
        Some(Ok(Message::Binary(data))) => {
            let frame = protocol::parse_server_frame(data.as_ref())?;
            debug!(is_last = frame.is_last, "ASR response frame received");
            if let Some(update) = protocol::extract_transcript(frame.payload.as_ref()) {
                // A second-pass response can keep the exact same canonical text
                // while changing an utterance from provisional to definite.
                if update != *latest_transcript {
                    latest_transcript.clone_from(&update);
                    let _ = events.send(StreamEvent::Transcript(update));
                }
            }
            Ok(frame.is_last)
        }
        Some(Ok(Message::Ping(payload))) => {
            writer
                .send(Message::Pong(payload))
                .await
                .context("failed to answer the ASR WebSocket ping")?;
            Ok(false)
        }
        Some(Ok(Message::Close(frame))) => {
            bail!("ASR WebSocket closed before the final result: {frame:?}")
        }
        Some(Ok(_)) => Ok(false),
        Some(Err(error)) => Err(error).context("ASR WebSocket receive failed"),
        None => bail!("ASR WebSocket ended before the final result"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_request_enables_asr_second_pass() {
        let payload = request_payload();
        assert_eq!(
            payload.pointer("/request/enable_nonstream"),
            Some(&serde_json::Value::Bool(true))
        );
        assert_eq!(
            payload.pointer("/request/end_window_size"),
            Some(&serde_json::Value::from(VAD_END_WINDOW_MS))
        );
    }

    #[test]
    fn production_audio_has_an_edge_guard() {
        assert_eq!(AUDIO_EDGE_GUARD_MS, 200);
        assert_eq!(
            TARGET_SAMPLE_RATE as usize * AUDIO_EDGE_GUARD_MS / 1_000,
            3_200
        );
    }
}
