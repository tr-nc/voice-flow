use std::sync::mpsc::Sender as StopSender;
use std::thread::{self, JoinHandle};

use anyhow::{Context, Result, bail};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};
use serde::Serialize;
use tokio::sync::mpsc::UnboundedSender;
use tracing::{Span, error, info};

use crate::diagnostics::DiagnosticAudioSink;

pub const TARGET_SAMPLE_RATE: u32 = 16_000;

#[derive(Debug, Clone, Serialize)]
pub struct Microphone {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

pub fn list_microphones() -> Result<Vec<Microphone>> {
    let host = cpal::default_host();
    let default_name = host
        .default_input_device()
        .and_then(|device| device.name().ok());
    let devices = host
        .input_devices()
        .context("failed to enumerate microphones")?
        .filter_map(|device| device.name().ok())
        .map(|name| Microphone {
            id: name.clone(),
            is_default: default_name.as_deref() == Some(name.as_str()),
            name,
        })
        .collect();
    Ok(devices)
}

#[derive(Debug)]
pub struct AudioChunk {
    pub samples: Vec<i16>,
}

#[derive(Debug)]
pub enum AudioEvent {
    Data(AudioChunk),
    Error(String),
}

#[derive(Clone)]
struct AudioOutput {
    sender: UnboundedSender<AudioEvent>,
    diagnostics: Option<DiagnosticAudioSink>,
}

impl AudioOutput {
    fn send_samples(&self, samples: Vec<i16>) {
        if let Some(diagnostics) = &self.diagnostics {
            diagnostics.write_samples(&samples);
        }
        let _ = self.sender.send(AudioEvent::Data(AudioChunk { samples }));
    }

    fn send_error(&self, error: impl ToString) {
        let _ = self.sender.send(AudioEvent::Error(error.to_string()));
    }
}

pub struct AudioCapture {
    stop_sender: Option<StopSender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl AudioCapture {
    pub fn start(
        selected_microphone: &str,
        sender: UnboundedSender<AudioEvent>,
        diagnostics: Option<DiagnosticAudioSink>,
    ) -> Result<Self> {
        let selected_microphone = selected_microphone.to_owned();
        let output = AudioOutput {
            sender,
            diagnostics,
        };
        let capture_span = Span::current();
        let (stop_sender, stop_receiver) = std::sync::mpsc::channel();
        info!(
            microphone = if selected_microphone.is_empty() {
                "system-default"
            } else {
                &selected_microphone
            },
            "starting microphone capture thread"
        );
        let thread = thread::Builder::new()
            .name("voice-flow-microphone".to_owned())
            .spawn(move || {
                let _capture_guard = capture_span.enter();
                let result = open_stream(&selected_microphone, output.clone());
                match result {
                    Ok(stream) => {
                        let _ = stop_receiver.recv();
                        drop(stream);
                    }
                    Err(error) => {
                        error!(%error, "microphone thread failed");
                        output.send_error(error);
                    }
                }
            })
            .context("failed to start the microphone thread")?;
        Ok(Self {
            stop_sender: Some(stop_sender),
            thread: Some(thread),
        })
    }
}

impl Drop for AudioCapture {
    fn drop(&mut self) {
        if let Some(sender) = self.stop_sender.take() {
            let _ = sender.send(());
        }
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            error!("microphone thread panicked while stopping");
        }
    }
}

fn open_stream(selected_microphone: &str, output: AudioOutput) -> Result<Stream> {
    let host = cpal::default_host();
    let device = if selected_microphone.is_empty() {
        host.default_input_device()
            .context("no default microphone is available")?
    } else {
        host.input_devices()
            .context("failed to enumerate microphones")?
            .find(|device| device.name().is_ok_and(|name| name == selected_microphone))
            .with_context(|| {
                format!("selected microphone is not available: {selected_microphone}")
            })?
    };
    let device_name = device
        .name()
        .unwrap_or_else(|_| "unknown microphone".to_owned());
    let supported = device
        .default_input_config()
        .context("failed to read the selected microphone format")?;
    let sample_format = supported.sample_format();
    let source_rate = supported.sample_rate().0;
    let config: StreamConfig = supported.into();
    let channels = usize::from(config.channels);

    if channels == 0 || source_rate == 0 {
        bail!("the selected microphone reported an invalid audio format");
    }
    info!(
        microphone = %device_name,
        ?sample_format,
        source_rate,
        channels,
        target_rate = TARGET_SAMPLE_RATE,
        "microphone opened"
    );
    let diagnostics = output.diagnostics.clone();
    if let Some(diagnostics) = &diagnostics {
        diagnostics.configure_source(
            &device_name,
            &format!("{sample_format:?}"),
            source_rate,
            config.channels,
        );
    }

    let stream = match sample_format {
        SampleFormat::F32 => build_f32_stream(&device, &config, channels, source_rate, output)?,
        SampleFormat::I16 => build_i16_stream(&device, &config, channels, source_rate, output)?,
        SampleFormat::U16 => build_u16_stream(&device, &config, channels, source_rate, output)?,
        format => bail!("unsupported microphone sample format: {format:?}"),
    };
    stream
        .play()
        .context("failed to start microphone capture")?;
    if let Some(diagnostics) = &diagnostics {
        diagnostics.mark_opened();
    }
    Ok(stream)
}

struct LinearResampler {
    step: f64,
    next_output_position: f64,
    source_index: u64,
    previous: Option<f32>,
}

impl LinearResampler {
    fn new(source_rate: u32) -> Self {
        Self {
            step: f64::from(source_rate) / f64::from(TARGET_SAMPLE_RATE),
            next_output_position: 0.0,
            source_index: 0,
            previous: None,
        }
    }

    fn push(&mut self, sample: f32, output: &mut Vec<i16>) {
        let sample = sample.clamp(-1.0, 1.0);
        let Some(previous) = self.previous else {
            output.push(float_to_pcm(sample));
            self.previous = Some(sample);
            self.next_output_position = self.step;
            return;
        };

        self.source_index += 1;
        let current_position = self.source_index as f64;
        let previous_position = current_position - 1.0;

        while self.next_output_position <= current_position {
            if self.next_output_position >= previous_position {
                let mix = (self.next_output_position - previous_position) as f32;
                let interpolated = previous + ((sample - previous) * mix);
                output.push(float_to_pcm(interpolated));
            }
            self.next_output_position += self.step;
        }

        self.previous = Some(sample);
    }
}

fn float_to_pcm(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16
}

fn process_input<T>(
    data: &[T],
    channels: usize,
    convert: impl Fn(T) -> f32,
    resampler: &mut LinearResampler,
    output: &AudioOutput,
) where
    T: Copy,
{
    let frame_count = data.len() / channels;
    if frame_count == 0 {
        return;
    }

    let expected_output = (frame_count * TARGET_SAMPLE_RATE as usize)
        .div_ceil((resampler.step * TARGET_SAMPLE_RATE as f64) as usize);
    let mut samples = Vec::with_capacity(expected_output.max(1));

    for frame in data.chunks_exact(channels) {
        let mono = frame.iter().map(|sample| convert(*sample)).sum::<f32>() / channels as f32;
        resampler.push(mono, &mut samples);
    }

    output.send_samples(samples);
}

fn build_f32_stream(
    device: &cpal::Device,
    config: &StreamConfig,
    channels: usize,
    source_rate: u32,
    output: AudioOutput,
) -> Result<Stream> {
    let error_output = output.clone();
    let mut resampler = LinearResampler::new(source_rate);
    device
        .build_input_stream(
            config,
            move |data: &[f32], _| {
                process_input(data, channels, |sample| sample, &mut resampler, &output);
            },
            move |error| {
                tracing::error!(%error, "f32 microphone stream error");
                error_output.send_error(error);
            },
            None,
        )
        .context("failed to open the microphone as f32 audio")
}

fn build_i16_stream(
    device: &cpal::Device,
    config: &StreamConfig,
    channels: usize,
    source_rate: u32,
    output: AudioOutput,
) -> Result<Stream> {
    let error_output = output.clone();
    let mut resampler = LinearResampler::new(source_rate);
    device
        .build_input_stream(
            config,
            move |data: &[i16], _| {
                process_input(
                    data,
                    channels,
                    |sample| f32::from(sample) / f32::from(i16::MAX),
                    &mut resampler,
                    &output,
                );
            },
            move |error| {
                tracing::error!(%error, "i16 microphone stream error");
                error_output.send_error(error);
            },
            None,
        )
        .context("failed to open the microphone as i16 audio")
}

fn build_u16_stream(
    device: &cpal::Device,
    config: &StreamConfig,
    channels: usize,
    source_rate: u32,
    output: AudioOutput,
) -> Result<Stream> {
    let error_output = output.clone();
    let mut resampler = LinearResampler::new(source_rate);
    device
        .build_input_stream(
            config,
            move |data: &[u16], _| {
                process_input(
                    data,
                    channels,
                    |sample| (f32::from(sample) / 32_767.5) - 1.0,
                    &mut resampler,
                    &output,
                );
            },
            move |error| {
                tracing::error!(%error, "u16 microphone stream error");
                error_output.send_error(error);
            },
            None,
        )
        .context("failed to open the microphone as u16 audio")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resamples_48khz_to_16khz() {
        let mut resampler = LinearResampler::new(48_000);
        let mut output = Vec::new();
        for _ in 0..4_800 {
            resampler.push(0.25, &mut output);
        }
        assert!((output.len() as isize - 1_600).abs() <= 1);
    }
}
