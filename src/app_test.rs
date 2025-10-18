use anyhow::{Context, Result};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

use crate::audio::{capture::RecordingSession, AudioCapture, AudioFeedback};
#[cfg(feature = "fast-vad")]
use crate::audio::{FastVad, FastVadOutcome};
use crate::config::{Config, ConfigManager};
use crate::input::TextInjector;
use crate::status::StatusWriter;
use crate::transcription::TranscriptionBackend;
use crate::whisper::WhisperVadOptions;

/// Test version of the app that doesn't use global shortcuts
pub struct HyprwhsprAppTest {
    config_manager: ConfigManager,
    audio_capture: AudioCapture,
    audio_feedback: AudioFeedback,
    transcriber: TranscriptionBackend,
    #[cfg(feature = "fast-vad")]
    fast_vad: Option<FastVad>,
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

        #[cfg(feature = "fast-vad")]
        let fast_vad = FastVad::maybe_new(&config.fast_vad)
            .context("Failed to initialize fast VAD pipeline")?;

        Ok(Self {
            config_manager,
            audio_capture,
            audio_feedback,
            transcriber,
            #[cfg(feature = "fast-vad")]
            fast_vad,
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

        #[cfg(feature = "fast-vad")]
        if self.current_config.fast_vad != new_config.fast_vad {
            self.fast_vad = FastVad::maybe_new(&new_config.fast_vad)
                .context("Failed to refresh fast VAD pipeline")?;
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

    #[cfg(feature = "fast-vad")]
    fn preprocess_audio(&mut self, audio_data: Vec<f32>) -> Result<Option<Vec<f32>>> {
        if let Some(vad) = self.fast_vad.as_mut() {
            let outcome = vad.trim(&audio_data).context("Fast VAD trimming failed")?;
            if outcome.trimmed_audio.is_empty() {
                info!(
                    "🎧 Recording contained only silence after fast VAD trimming; skipping transcription"
                );
                return Ok(None);
            }

            let FastVadOutcome { trimmed_audio, .. } = outcome;

            return Ok(Some(trimmed_audio));
        }

        Ok(Some(audio_data))
    }

    #[cfg(not(feature = "fast-vad"))]
    fn preprocess_audio(&mut self, audio_data: Vec<f32>) -> Result<Option<Vec<f32>>> {
        Ok(Some(audio_data))
    }

    async fn process_audio(&mut self, audio_data: Vec<f32>) -> Result<()> {
        let maybe_audio = self.preprocess_audio(audio_data)?;

        let Some(audio_for_transcription) = maybe_audio else {
            return Ok(());
        };

        if audio_for_transcription.is_empty() {
            info!("🎧 No audio remaining after preprocessing; skipping transcription");
            return Ok(());
        }

        let transcription = self.transcriber.transcribe(audio_for_transcription).await?;

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
