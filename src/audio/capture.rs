use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{BufferSize, InputCallbackInfo, SampleRate, StreamConfig};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tracing::{debug, error, info, warn};

pub struct AudioCapture {
    sample_rate: u32,
}

pub struct RecordingSession {
    stream: cpal::Stream,
    audio_data: Arc<Mutex<Vec<f32>>>,
    sample_rate_tracker: Arc<Mutex<SampleRateTracker>>,
    requested_sample_rate: u32,
}

#[derive(Debug, Clone)]
pub struct CapturedAudio {
    pub samples: Vec<f32>,
    pub sample_rate: u32,
}

impl CapturedAudio {
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }
}

#[derive(Debug)]
struct SampleRateTracker {
    requested: u32,
    channels: u16,
    last_capture: Option<cpal::StreamInstant>,
    accumulated_frames: u64,
    accumulated_duration: Duration,
    measured: Option<u32>,
}

impl SampleRateTracker {
    fn new(requested: u32, channels: u16) -> Self {
        Self {
            requested,
            channels,
            last_capture: None,
            accumulated_frames: 0,
            accumulated_duration: Duration::ZERO,
            measured: None,
        }
    }

    fn update(&mut self, data_len: usize, info: &InputCallbackInfo) {
        let capture = info.timestamp().capture;

        if let Some(prev) = self.last_capture {
            if let Some(delta) = capture.duration_since(&prev) {
                let channel_count = self.channels.max(1) as usize;
                let frames = data_len / channel_count;
                self.accumulated_frames += frames as u64;
                self.accumulated_duration += delta;

                if self.accumulated_duration.as_secs_f64() >= 0.05 {
                    let secs = self.accumulated_duration.as_secs_f64();
                    if secs > 0.0 && self.accumulated_frames > 0 {
                        let rate = (self.accumulated_frames as f64 / secs).round() as u32;
                        self.measured = Some(rate);
                    }
                    self.accumulated_frames = 0;
                    self.accumulated_duration = Duration::ZERO;
                }
            }
        }

        self.last_capture = Some(capture);
    }

    fn sample_rate(&self) -> u32 {
        self.measured.unwrap_or(self.requested)
    }
}

impl AudioCapture {
    pub fn new() -> Result<Self> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .context("No input device available")?;

        let device_name = device.name().unwrap_or_else(|_| "Unknown".to_string());

        info!("Using audio input device: {}", device_name);

        Ok(Self { sample_rate: 16000 })
    }

    pub fn sample_rate_hint(&self) -> u32 {
        self.sample_rate
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
        let sample_rate_tracker = Arc::new(Mutex::new(SampleRateTracker::new(
            config.sample_rate.0,
            config.channels,
        )));
        let tracker_clone = Arc::clone(&sample_rate_tracker);

        // Build input stream
        let stream = device
            .build_input_stream(
                &config,
                move |data: &[f32], info: &InputCallbackInfo| {
                    if let Ok(mut tracker) = tracker_clone.lock() {
                        tracker.update(data.len(), info);
                    }
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

        let device_name = device.name().unwrap_or_else(|_| "Unknown".to_string());
        info!("✅ Audio recording started on {}", device_name);

        Ok(RecordingSession {
            stream,
            audio_data,
            sample_rate_tracker,
            requested_sample_rate: config.sample_rate.0,
        })
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
    pub fn stop(self) -> Result<CapturedAudio> {
        // Drop the stream (stops recording)
        drop(self.stream);

        let measured_sample_rate = self
            .sample_rate_tracker
            .lock()
            .map(|tracker| tracker.sample_rate())
            .unwrap_or(self.requested_sample_rate);

        // Extract the recorded audio
        let audio_data = Arc::try_unwrap(self.audio_data)
            .map_err(|_| anyhow::anyhow!("Failed to unwrap audio data"))?
            .into_inner()
            .map_err(|_| anyhow::anyhow!("Failed to lock audio data"))?;

        let duration_secs = if measured_sample_rate > 0 {
            audio_data.len() as f32 / measured_sample_rate as f32
        } else {
            0.0
        };
        info!(
            "🛑 Audio recording stopped - captured {} samples ({:.2}s @ {} Hz)",
            audio_data.len(),
            duration_secs,
            measured_sample_rate
        );

        if audio_data.is_empty() {
            warn!("No audio data captured");
        }

        Ok(CapturedAudio {
            samples: audio_data,
            sample_rate: measured_sample_rate,
        })
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
