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
use uuid::Uuid;

use crate::audio::{AudioCapture, AudioEvent, TARGET_SAMPLE_RATE};
use crate::config::AppConfig;

const AUDIO_PACKET_SAMPLES: usize = 3_200;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const SOCKET_WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const FINAL_RESPONSE_TIMEOUT: Duration = Duration::from_secs(6);

#[derive(Debug)]
pub enum StreamEvent {
    Connected,
    Transcript(String),
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
    stop_receiver: oneshot::Receiver<()>,
    events: mpsc::UnboundedSender<StreamEvent>,
) -> Result<String> {
    config.validate()?;
    info!(
        endpoint = %config.endpoint.split(['?', '#']).next().unwrap_or("invalid-endpoint"),
        resource_id = %config.resource_id,
        auth_mode = if config.app_id.is_empty() { "api-key" } else { "legacy-app-id" },
        "preparing streaming ASR session"
    );

    let mut stop_receiver = stop_receiver;
    let request_id = Uuid::new_v4().to_string();
    let mut request = config
        .endpoint
        .as_str()
        .into_client_request()
        .context("failed to create the VolcEngine WebSocket request")?;
    let headers = request.headers_mut();
    headers.insert(
        "x-api-resource-id",
        HeaderValue::from_str(&config.resource_id).context("resource ID is not a valid header")?,
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

    if config.app_id.is_empty() {
        headers.insert(
            "x-api-key",
            HeaderValue::from_str(&config.secret_key)
                .context("Secret Key / API Key is not a valid header")?,
        );
    } else {
        headers.insert(
            "x-api-app-key",
            HeaderValue::from_str(&config.app_id).context("APP ID is not a valid header")?,
        );
        headers.insert(
            "x-api-access-key",
            HeaderValue::from_str(&config.secret_key)
                .context("Secret Key / Access Token is not a valid header")?,
        );
    }

    let connect_started = Instant::now();
    let connection = tokio::select! {
        _ = &mut stop_receiver => {
            info!("ASR session cancelled while connecting");
            return Ok(String::new());
        }
        result = timeout(CONNECT_TIMEOUT, connect_async(request)) => result,
    };
    let (websocket, response) = connection
        .context("timed out while connecting to VolcEngine ASR")?
        .context("failed to connect to VolcEngine ASR")?;
    info!(
        elapsed_ms = connect_started.elapsed().as_millis(),
        status = %response.status(),
        "ASR WebSocket connected"
    );
    let (mut writer, mut reader) = websocket.split();

    let request_payload = json!({
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
            "enable_itn": true,
            "enable_punc": true,
            "enable_ddc": false,
            "show_utterances": true,
            "result_type": "full"
        }
    });
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

    let (audio_sender, mut audio_receiver) = mpsc::unbounded_channel();
    let capture = AudioCapture::start(&config.microphone, audio_sender)?;
    let mut sequence = 2;
    let mut pending_samples = Vec::with_capacity(AUDIO_PACKET_SAMPLES * 2);
    let mut final_text = String::new();

    loop {
        tokio::select! {
            _ = &mut stop_receiver => {
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
                if handle_message(message, &mut writer, &events, &mut final_text).await? {
                    return Ok(final_text);
                }
            }
        }
    }

    drop(capture);
    while let Ok(audio_event) = audio_receiver.try_recv() {
        match audio_event {
            AudioEvent::Data(chunk) => pending_samples.extend_from_slice(&chunk.samples),
            AudioEvent::Error(error) => bail!("microphone capture failed: {error}"),
        }
    }

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
            if handle_message(message, &mut writer, &events, &mut final_text).await? {
                return Ok::<(), anyhow::Error>(());
            }
        }
    })
    .await;
    match final_response {
        Ok(result) => result?,
        Err(_) if !final_text.is_empty() => {
            warn!(
                wait_ms = final_wait_started.elapsed().as_millis(),
                "final ASR marker timed out; using the latest transcript"
            );
        }
        Err(_) => bail!("timed out waiting for the final ASR response"),
    }
    info!(
        final_wait_ms = final_wait_started.elapsed().as_millis(),
        characters = final_text.chars().count(),
        "ASR session finalized"
    );

    Ok(final_text)
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
    final_text: &mut String,
) -> Result<bool>
where
    S: futures_util::Sink<Message> + Unpin,
    S::Error: std::error::Error + Send + Sync + 'static,
{
    match message {
        Some(Ok(Message::Binary(data))) => {
            let frame = protocol::parse_server_frame(data.as_ref())?;
            debug!(is_last = frame.is_last, "ASR response frame received");
            if let Some(text) = protocol::extract_text(frame.payload.as_ref())
                && text != *final_text
            {
                final_text.clone_from(&text);
                let _ = events.send(StreamEvent::Transcript(text));
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

    #[tokio::test]
    async fn stop_cancels_a_session_while_it_is_connecting() {
        install_tls_provider().unwrap();
        let config = AppConfig {
            secret_key: "test-key".to_owned(),
            endpoint: "wss://192.0.2.1/voice-flow-test".to_owned(),
            ..AppConfig::default()
        };
        let (stop_sender, stop_receiver) = oneshot::channel();
        let (event_sender, _event_receiver) = mpsc::unbounded_channel();
        stop_sender.send(()).unwrap();

        let result = timeout(
            Duration::from_secs(1),
            recognize(config, stop_receiver, event_sender),
        )
        .await
        .expect("stop should not leave the connection pending")
        .unwrap();

        assert!(result.is_empty());
    }
}
