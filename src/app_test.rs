use anyhow::{Context, Result};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

use crate::audio::{capture::RecordingSession, AudioCapture, AudioFeedback};
use crate::config::{Config, ConfigManager, SttBackend};
use crate::input::TextInjector;
use crate::status::StatusWriter;
use crate::whisper::{Transcriber, WhisperVadOptions};
use crate::HyprwhsprApp;

/// Test version of the app that doesn't use global shortcuts
pub struct HyprwhsprAppTest {
    config_manager: ConfigManager,
    audio_capture: AudioCapture,
    audio_feedback: AudioFeedback,
    transcriber: Box<dyn Transcriber>,
    text_injector: Arc<Mutex<TextInjector>>,
    status_writer: StatusWriter,
    current_config: Config,
    recording_session: Option<RecordingSession>,
    is_processing: bool,
    backend: SttBackend,
    backend_override: Option<SttBackend>,
}

impl HyprwhsprAppTest {
    pub fn new(
        config_manager: ConfigManager,
        backend_override: Option<SttBackend>,
    ) -> Result<Self> {
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

        let backend = backend_override.unwrap_or(config.stt_backend);
        let transcriber = HyprwhsprApp::create_transcriber(&config_manager, &config, backend)?;

        let text_injector = TextInjector::new(
            config.shift_paste,
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
            backend,
            backend_override,
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
            new_config.word_overrides.clone(),
            new_config.auto_copy_clipboard,
        )?;

        let desired_backend = self.backend_override.unwrap_or(new_config.stt_backend);

        if desired_backend != self.backend {
            self.transcriber = HyprwhsprApp::create_transcriber(
                &self.config_manager,
                &new_config,
                desired_backend,
            )?;
            self.backend = desired_backend;
        } else if matches!(self.backend, SttBackend::Local) {
            let whisper_changed = self.current_config.model != new_config.model
                || self.current_config.whisper_prompt != new_config.whisper_prompt
                || self.current_config.threads != new_config.threads
                || self.current_config.gpu_layers != new_config.gpu_layers
                || self.current_config.vad != new_config.vad
                || (self.current_config.no_speech_threshold - new_config.no_speech_threshold).abs()
                    > f32::EPSILON;

            if whisper_changed {
                self.transcriber = HyprwhsprApp::create_transcriber(
                    &self.config_manager,
                    &new_config,
                    desired_backend,
                )?;
            }
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

        let audio_data = session.stop().context("Failed to stop recording")?;

        if !audio_data.is_empty() {
            self.is_processing = true;
            info!("🧠 Processing audio...");
            if let Err(e) = self.process_audio(audio_data).await {
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

    async fn process_audio(&mut self, audio_data: Vec<f32>) -> Result<()> {
        let transcription = self
            .transcriber
            .transcribe(audio_data, self.audio_capture.sample_rate())
            .await?;

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
