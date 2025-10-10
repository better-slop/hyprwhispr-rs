use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, SampleRate, StreamConfig};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

pub struct AudioCapture {
    sample_rate: u32,
    device_name: String,
}

pub struct RecordingSession {
    stream: cpal::Stream,
    audio_data: Arc<Mutex<Vec<f32>>>,
}

impl AudioCapture {
    pub fn new() -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .context("No input device available")?;

        let device_name = device.name().unwrap_or_else(|_| "Unknown".to_string());

        info!("Using audio input device: {}", device_name);

        Ok(Self {
            sample_rate: 16000,
            device_name,
        })
    }

    pub fn start_recording(&self) -> Result<RecordingSession> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .context("No input device available")?;

        // Configure for 16kHz mono (whisper.cpp prefers this)
        let config = StreamConfig {
            channels: 1,
            sample_rate: SampleRate(self.sample_rate),
            buffer_size: BufferSize::Default,
        };

        debug!("Starting audio capture at {}Hz mono", self.sample_rate);

        // Shared buffer for audio data
        let audio_data = Arc::new(Mutex::new(Vec::new()));
        let audio_data_clone = Arc::clone(&audio_data);

        // Build input stream
        let stream = device
            .build_input_stream(
                &config,
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    // Store audio samples
                    if let Ok(mut buffer) = audio_data_clone.lock() {
                        buffer.extend_from_slice(data);
                    }
                },
                move |err| {
                    error!("Audio stream error: {}", err);
                },
                None,
            )
            .context("Failed to build input stream")?;

        // Start the stream
        stream.play().context("Failed to start audio stream")?;

        info!("✅ Audio recording started");

        Ok(RecordingSession { stream, audio_data })
    }

    pub fn get_available_devices() -> Result<Vec<String>> {
        let host = cpal::default_host();
        let mut devices = Vec::new();

        for device in host.input_devices()? {
            if let Ok(name) = device.name() {
                devices.push(name);
            }
        }

        Ok(devices)
    }
}

impl RecordingSession {
    pub fn stop(self) -> Result<Vec<f32>> {
        // Drop the stream (stops recording)
        drop(self.stream);

        // Extract the recorded audio
        let audio_data = Arc::try_unwrap(self.audio_data)
            .map_err(|_| anyhow::anyhow!("Failed to unwrap audio data"))?
            .into_inner()
            .map_err(|_| anyhow::anyhow!("Failed to lock audio data"))?;

        let duration_secs = audio_data.len() as f32 / 16000.0;
        info!(
            "🛑 Audio recording stopped - captured {} samples ({:.2}s)",
            audio_data.len(),
            duration_secs
        );

        if audio_data.is_empty() {
            warn!("No audio data captured");
        }

        Ok(audio_data)
    }

    pub fn get_current_level(&self) -> f32 {
        if let Ok(data) = self.audio_data.lock() {
            if data.is_empty() {
                return 0.0;
            }

            // Calculate RMS level for last 1024 samples
            let start = data.len().saturating_sub(1024);
            let samples = &data[start..];

            let sum_squares: f32 = samples.iter().map(|s| s * s).sum();
            let rms = (sum_squares / samples.len() as f32).sqrt();

            // Scale for better visualization (0.0 to 1.0)
            (rms * 10.0).min(1.0)
        } else {
            0.0
        }
    }
}

impl Default for AudioCapture {
    fn default() -> Self {
        Self::new().expect("Failed to create AudioCapture")
    }
}
