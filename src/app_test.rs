use anyhow::{Context, Result};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

use crate::app::BackendKind;
use crate::audio::{capture::RecordingSession, AudioCapture, AudioFeedback};
use crate::config::{Config, ConfigManager};
use crate::input::TextInjector;
use crate::status::StatusWriter;
use crate::whisper::{GroqClient, LocalWhisper, Transcriber};

/// Test version of the app that doesn't use global shortcuts
pub struct HyprwhsprAppTest {
    config_manager: ConfigManager,
    audio_capture: AudioCapture,
    audio_feedback: AudioFeedback,
    transcriber: Box<dyn Transcriber>,
    backend_kind: BackendKind,
    backend_override: bool,
    text_injector: Arc<Mutex<TextInjector>>,
    status_writer: StatusWriter,
    current_config: Config,
    recording_session: Option<RecordingSession>,
    is_processing: bool,
}

impl HyprwhsprAppTest {
    pub fn new(
        config_manager: ConfigManager,
        transcriber: Box<dyn Transcriber>,
        backend_kind: BackendKind,
        backend_override: bool,
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
            backend_kind,
            backend_override,
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
            new_config.word_overrides.clone(),
            new_config.auto_copy_clipboard,
        )?;

        let desired_backend = if new_config.use_groq {
            BackendKind::Groq
        } else {
            BackendKind::Local
        };

        if !self.backend_override && desired_backend != self.backend_kind {
            self.switch_transcriber(desired_backend, &new_config)?;
        } else if matches!(self.backend_kind, BackendKind::Local) {
            let whisper_changed = self.current_config.model != new_config.model
                || self.current_config.whisper_prompt != new_config.whisper_prompt
                || self.current_config.threads != new_config.threads
                || self.current_config.gpu_layers != new_config.gpu_layers
                || self.current_config.vad != new_config.vad
                || (self.current_config.no_speech_threshold - new_config.no_speech_threshold).abs()
                    > f32::EPSILON;

            if whisper_changed {
                let local = self.build_local_transcriber(&new_config)?;
                self.transcriber = Box::new(local);
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
            let sample_rate_hz = self.audio_capture.sample_rate();
            if let Err(e) = self.process_audio(audio_data, sample_rate_hz).await {
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

    async fn process_audio(&mut self, audio_data: Vec<f32>, sample_rate_hz: u32) -> Result<()> {
        let transcription = self
            .transcriber
            .transcribe(audio_data, sample_rate_hz)
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

    fn build_local_transcriber(&self, config: &Config) -> Result<LocalWhisper> {
        let vad_options = self.config_manager.build_vad_options(config);
        let local = LocalWhisper::new(
            self.config_manager.get_model_path(),
            self.config_manager.get_whisper_binary_path(),
            config.threads,
            config.whisper_prompt.clone(),
            self.config_manager.get_temp_dir(),
            config.gpu_layers,
            vad_options,
            config.no_speech_threshold,
        )?;
        local.initialize()?;
        Ok(local)
    }

    fn switch_transcriber(&mut self, backend: BackendKind, config: &Config) -> Result<()> {
        match backend {
            BackendKind::Local => {
                let local = self.build_local_transcriber(config)?;
                self.transcriber = Box::new(local);
                info!("Backend switched to LocalWhisper");
            }
            BackendKind::Groq => {
                let groq = GroqClient::new().context("Failed to initialize Groq backend")?;
                self.transcriber = Box::new(groq);
                info!("Backend switched to Groq (model=whisper-large-v3)");
            }
        }

        self.backend_kind = backend;
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
