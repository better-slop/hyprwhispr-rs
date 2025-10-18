use anyhow::{Context, Result};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use crate::audio::{capture::RecordingSession, AudioCapture, AudioFeedback, CapturedAudio};
use crate::config::{Config, ConfigManager};
use crate::input::TextInjector;
use crate::status::StatusWriter;
use crate::transcription::TranscriptionBackend;
use crate::whisper::WhisperVadOptions;

#[cfg(feature = "fast-vad")]
use crate::audio::trim_buffer;

/// Test version of the app that doesn't use global shortcuts
pub struct HyprwhsprAppTest {
    config_manager: ConfigManager,
    audio_capture: AudioCapture,
    audio_feedback: AudioFeedback,
    transcriber: TranscriptionBackend,
    text_injector: Arc<Mutex<TextInjector>>,
    status_writer: StatusWriter,
    current_config: Config,
    recording_session: Option<RecordingSession>,
    is_processing: bool,
}

impl HyprwhsprAppTest {
    pub fn new(config_manager: ConfigManager) -> Result<Self> {
        let config = config_manager.get();

        let audio_capture = AudioCapture::new().context("Failed to initialize audio capture")?;

        let assets_dir = config_manager.get_assets_dir();
        let audio_feedback = AudioFeedback::new(
            config.audio_feedback,
            assets_dir,
            config.start_sound_path.clone(),
            config.stop_sound_path.clone(),
            config.start_sound_volume,
            config.stop_sound_volume,
        );

        let vad_options = build_vad_options(&config_manager, &config);

        let transcriber = TranscriptionBackend::build(&config_manager, &config, vad_options)
            .context("Failed to configure transcription backend")?;

        transcriber
            .initialize()
            .context("Failed to initialize transcription backend")?;

        info!(
            "🎯 Active transcription backend: {}",
            transcriber.provider().label()
        );

        let text_injector = TextInjector::new(
            config.shift_paste,
            config.paste_hints.shift.clone(),
            config.word_overrides.clone(),
            config.auto_copy_clipboard,
        )?;

        let status_writer = StatusWriter::new()?;
        status_writer.set_recording(false)?;

        Ok(Self {
            config_manager,
            audio_capture,
            audio_feedback,
            transcriber,
            text_injector: Arc::new(Mutex::new(text_injector)),
            status_writer,
            current_config: config,
            recording_session: None,
            is_processing: false,
        })
    }

    pub fn apply_config_update(&mut self, new_config: Config) -> Result<()> {
        tracing::debug!(?new_config, "Apply config update requested (test mode)");
        if new_config == self.current_config {
            tracing::debug!("Config unchanged; ignoring update (test mode)");
            return Ok(());
        }

        if self.recording_session.is_some() || self.is_processing {
            warn!("Skipping config refresh while busy");
            return Ok(());
        }

        let assets_dir = self.config_manager.get_assets_dir();
        let audio_feedback = AudioFeedback::new(
            new_config.audio_feedback,
            assets_dir,
            new_config.start_sound_path.clone(),
            new_config.stop_sound_path.clone(),
            new_config.start_sound_volume,
            new_config.stop_sound_volume,
        );

        let text_injector = TextInjector::new(
            new_config.shift_paste,
            new_config.paste_hints.shift.clone(),
            new_config.word_overrides.clone(),
            new_config.auto_copy_clipboard,
        )?;

        let transcriber_changed =
            TranscriptionBackend::needs_refresh(&self.current_config, &new_config);

        if transcriber_changed {
            let vad_options = build_vad_options(&self.config_manager, &new_config);
            let backend =
                TranscriptionBackend::build(&self.config_manager, &new_config, vad_options)
                    .context("Failed to reconfigure transcription backend")?;
            backend
                .initialize()
                .context("Failed to initialize updated transcription backend")?;
            info!(
                "🎯 Active transcription backend: {}",
                backend.provider().label()
            );
            self.transcriber = backend;
        }

        self.text_injector = Arc::new(Mutex::new(text_injector));
        self.audio_feedback = audio_feedback;
        self.current_config = new_config;

        info!("Configuration updated");
        tracing::debug!(?self.current_config, "Config state after update (test mode)");
        Ok(())
    }

    pub async fn toggle_recording(&mut self) -> Result<()> {
        if self.is_processing {
            warn!("Still processing previous recording, please wait");
            return Ok(());
        }

        if self.recording_session.is_some() {
            self.stop_recording().await?;
        } else {
            self.start_recording().await?;
        }

        Ok(())
    }

    async fn start_recording(&mut self) -> Result<()> {
        info!("🎤 Starting recording - speak now!");

        self.audio_feedback.play_start_sound()?;

        let session = self
            .audio_capture
            .start_recording()
            .context("Failed to start recording")?;

        self.recording_session = Some(session);

        self.status_writer.set_recording(true)?;

        info!("⏺️  Recording... (press Enter to stop)");

        Ok(())
    }

    async fn stop_recording(&mut self) -> Result<()> {
        info!("🛑 Stopping recording...");

        let session = self
            .recording_session
            .take()
            .context("No active recording session")?;

        self.audio_feedback.play_stop_sound()?;

        self.status_writer.set_recording(false)?;

        let captured = session.stop().context("Failed to stop recording")?;

        if !captured.samples.is_empty() {
            self.is_processing = true;
            info!("🧠 Processing audio...");
            if let Err(e) = self.process_audio(captured).await {
                error!("Error processing audio: {}", e);
            }
            self.is_processing = false;
            info!("");
            info!("✅ Ready for next recording (press Enter)");
        } else {
            warn!("No audio data captured - try speaking louder");
        }

        Ok(())
    }

    async fn process_audio(&mut self, captured: CapturedAudio) -> Result<()> {
        let audio_data = self.prepare_audio(captured)?;

        if audio_data.is_empty() {
            warn!("Recording contained only silence after trimming; skipping transcription");
            return Ok(());
        }

        let transcription = self.transcriber.transcribe(audio_data).await?;

        if transcription.trim().is_empty() {
            warn!("Empty transcription - Whisper couldn't understand the audio");
            return Ok(());
        }

        info!("📝 Transcription: \"{}\"", transcription);

        let text_injector = Arc::clone(&self.text_injector);
        let mut injector = text_injector.lock().await;

        info!("⌨️  Injecting text into active application...");
        injector.inject_text(&transcription).await?;
        info!("✅ Text injected successfully!");

        Ok(())
    }

    fn prepare_audio(&self, captured: CapturedAudio) -> Result<Vec<f32>> {
        const TARGET_SAMPLE_RATE: u32 = 16_000;

        let mut samples = captured.samples;
        if captured.sample_rate != TARGET_SAMPLE_RATE
            && captured.sample_rate > 0
            && !samples.is_empty()
        {
            debug!(
                "Resampling audio from {} Hz to {} Hz",
                captured.sample_rate, TARGET_SAMPLE_RATE
            );
            samples = Self::resample_to_16khz(&samples, captured.sample_rate);
        }

        #[cfg(feature = "fast-vad")]
        {
            if self.current_config.fast_vad.enabled {
                return trim_buffer(&samples, TARGET_SAMPLE_RATE, &self.current_config.fast_vad);
            }
        }

        Ok(samples)
    }

    fn resample_to_16khz(samples: &[f32], source_rate: u32) -> Vec<f32> {
        const TARGET_SAMPLE_RATE: u32 = 16_000;

        if samples.is_empty() || source_rate == 0 || source_rate == TARGET_SAMPLE_RATE {
            return samples.to_vec();
        }

        let ratio = TARGET_SAMPLE_RATE as f64 / source_rate as f64;
        let new_len = ((samples.len() as f64) * ratio).round() as usize;
        if new_len == 0 {
            return Vec::new();
        }

        let mut output = Vec::with_capacity(new_len);
        for i in 0..new_len {
            let src_pos = i as f64 / ratio;
            let base = src_pos.floor() as usize;
            let frac = src_pos - base as f64;

            if base + 1 < samples.len() {
                let a = samples[base];
                let b = samples[base + 1];
                output.push(a + (b - a) * frac as f32);
            } else if let Some(&last) = samples.last() {
                output.push(last);
            }
        }

        output
    }

    pub async fn cleanup(&mut self) -> Result<()> {
        info!("🧹 Cleaning up...");

        if self.recording_session.is_some() {
            self.status_writer.set_recording(false)?;
            self.recording_session = None;
        }

        info!("✅ Cleanup completed");
        Ok(())
    }
}

fn build_vad_options(config_manager: &ConfigManager, config: &Config) -> WhisperVadOptions {
    WhisperVadOptions {
        enabled: config.vad.enabled,
        model_path: config_manager.get_vad_model_path(config),
        threshold: config.vad.threshold,
        min_speech_ms: config.vad.min_speech_ms,
        min_silence_ms: config.vad.min_silence_ms,
        max_speech_s: config.vad.max_speech_s,
        speech_pad_ms: config.vad.speech_pad_ms,
        samples_overlap: config.vad.samples_overlap,
    }
}
