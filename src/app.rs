use anyhow::{Context, Result};
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tracing::{error, info, warn};

use crate::audio::{AudioCapture, AudioFeedback, capture::RecordingSession};
use crate::config::ConfigManager;
use crate::input::{GlobalShortcuts, ShortcutEvent, TextInjector};
use crate::status::StatusWriter;
use crate::whisper::WhisperManager;

pub struct HyprwhsprApp {
    config_manager: ConfigManager,
    audio_capture: AudioCapture,
    audio_feedback: AudioFeedback,
    whisper_manager: WhisperManager,
    text_injector: Arc<Mutex<TextInjector>>,
    status_writer: StatusWriter,
    
    // State
    recording_session: Option<RecordingSession>,
    is_processing: bool,
}

impl HyprwhsprApp {
    pub fn new(config_manager: ConfigManager) -> Result<Self> {
        let config = config_manager.get();

        // Initialize audio capture
        let audio_capture = AudioCapture::new()
            .context("Failed to initialize audio capture")?;

        // Initialize audio feedback
        let assets_dir = config_manager.get_assets_dir();
        let audio_feedback = AudioFeedback::new(
            config.audio_feedback,
            assets_dir,
            config.start_sound_path.clone(),
            config.stop_sound_path.clone(),
            config.start_sound_volume,
            config.stop_sound_volume,
        );

        // Initialize whisper manager
        let whisper_manager = WhisperManager::new(
            config_manager.get_model_path(),
            config_manager.get_whisper_binary_path(),
            config.threads,
            config.whisper_prompt.clone(),
            config_manager.get_temp_dir(),
            config.gpu_layers,
        )?;

        whisper_manager.initialize()
            .context("Failed to initialize Whisper")?;

        // Initialize text injector
        let text_injector = TextInjector::new(
            config.shift_paste,
            config.word_overrides.clone(),
            config.auto_copy_clipboard,
        )?;

        // Initialize status writer
        let status_writer = StatusWriter::new()?;
        status_writer.set_recording(false)?;

        Ok(Self {
            config_manager,
            audio_capture,
            audio_feedback,
            whisper_manager,
            text_injector: Arc::new(Mutex::new(text_injector)),
            status_writer,
            recording_session: None,
            is_processing: false,
        })
    }

    pub async fn run(mut self) -> Result<()> {
        info!("🚀 hyprwhspr running!");

        // Set up channels
        let (shortcut_tx, mut shortcut_rx) = mpsc::channel(10);

        // Spawn global shortcuts listener in separate thread
        let config = self.config_manager.get();
        let shortcut = config.primary_shortcut.clone();

        std::thread::spawn(move || {
            match GlobalShortcuts::new(&shortcut) {
                Ok(shortcuts) => {
                    if let Err(e) = shortcuts.run(shortcut_tx) {
                        error!("Global shortcuts error: {}", e);
                    }
                }
                Err(e) => {
                    error!("Failed to initialize global shortcuts: {}", e);
                }
            }
        });

        // Main event loop
        loop {
            tokio::select! {
                Some(event) = shortcut_rx.recv() => {
                    if let Err(e) = self.handle_shortcut(event).await {
                        error!("Error handling shortcut: {}", e);
                    }
                }
            }
        }
    }

    async fn handle_shortcut(&mut self, _event: ShortcutEvent) -> Result<()> {
        if self.is_processing {
            warn!("Still processing previous recording, ignoring shortcut");
            return Ok(());
        }

        if self.recording_session.is_some() {
            // Stop recording
            self.stop_recording().await?;
        } else {
            // Start recording
            self.start_recording().await?;
        }

        Ok(())
    }

    async fn start_recording(&mut self) -> Result<()> {
        info!("🎤 Starting recording...");

        // Play start sound
        self.audio_feedback.play_start_sound()?;

        // Start audio capture
        let session = self.audio_capture.start_recording()
            .context("Failed to start recording")?;

        self.recording_session = Some(session);

        // Update status
        self.status_writer.set_recording(true)?;

        Ok(())
    }

    async fn stop_recording(&mut self) -> Result<()> {
        info!("🛑 Stopping recording...");

        // Take ownership of the recording session
        let session = self.recording_session.take()
            .context("No active recording session")?;

        // Play stop sound
        self.audio_feedback.play_stop_sound()?;

        // Update status
        self.status_writer.set_recording(false)?;

        // Stop recording and get audio data
        let audio_data = session.stop()
            .context("Failed to stop recording")?;

        // Process the audio
        if !audio_data.is_empty() {
            self.is_processing = true;
            if let Err(e) = self.process_audio(audio_data).await {
                error!("Error processing audio: {}", e);
            }
            self.is_processing = false;
        } else {
            warn!("No audio data captured");
        }

        Ok(())
    }

    async fn process_audio(&mut self, audio_data: Vec<f32>) -> Result<()> {
        // Transcribe audio
        let transcription = self.whisper_manager.transcribe(audio_data).await?;

        if transcription.trim().is_empty() {
            warn!("Empty transcription, nothing to inject");
            return Ok(());
        }

        // Inject text
        let text_injector = Arc::clone(&self.text_injector);
        let mut injector = text_injector.lock().await;
        injector.inject_text(&transcription).await?;

        Ok(())
    }

    pub async fn cleanup(&mut self) -> Result<()> {
        info!("🧹 Cleaning up...");

        // Stop recording if active
        if self.recording_session.is_some() {
            self.status_writer.set_recording(false)?;
            self.recording_session = None;
        }

        // Don't save config on exit - only save when explicitly modified
        // self.config_manager.save()?;

        info!("✅ Cleanup completed");
        Ok(())
    }
}
