use anyhow::{Context, Result};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use crate::audio::{
    capture::RecordingSession, AudioCapture, AudioFeedback, CapturedAudio, FastVad, FastVadOutcome,
};
use crate::config::{Config, ConfigManager};
use crate::input::TextInjector;
use crate::status::StatusWriter;
use crate::transcription::{TranscriptionBackend, TranscriptionResult};
use crate::whisper::WhisperVadOptions;

/// Test version of the app that doesn't use global shortcuts
pub struct HyprwhsprAppTest {
    config_manager: ConfigManager,
    audio_capture: AudioCapture,
    audio_feedback: AudioFeedback,
    transcriber: TranscriptionBackend,
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

        let fast_vad = FastVad::maybe_new(&config.fast_vad, audio_capture.sample_rate_hint())
            .context("Failed to initialize fast VAD pipeline")?;

        Ok(Self {
            config_manager,
            audio_capture,
            audio_feedback,
            transcriber,
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

        if self.current_config.fast_vad != new_config.fast_vad {
            self.fast_vad =
                FastVad::maybe_new(&new_config.fast_vad, self.audio_capture.sample_rate_hint())
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

        let captured_audio = session.stop().context("Failed to stop recording")?;

        if !captured_audio.is_empty() {
            self.is_processing = true;
            info!("🧠 Processing audio...");
            if let Err(e) = self.process_audio(captured_audio).await {
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

    fn preprocess_audio(&mut self, audio_data: CapturedAudio) -> Result<Option<CapturedAudio>> {
        let CapturedAudio {
            mut samples,
            mut sample_rate,
        } = audio_data;

        if let Some(vad) = self.fast_vad.as_mut() {
            if !FastVad::supports_sample_rate(sample_rate) {
                warn!(
                    "🎚️ Input sample rate {} Hz unsupported by fast VAD; resampling to 16 kHz (test mode)",
                    sample_rate
                );
                samples = resample_audio(&samples, sample_rate, 16_000);
                sample_rate = 16_000;
            }

            if vad.sample_rate_hz() != sample_rate {
                vad.set_sample_rate(sample_rate)
                    .context("Failed to configure fast VAD sample rate")?;
            }

            let outcome = vad.trim(&samples).context("Fast VAD trimming failed")?;
            if outcome.trimmed_audio.is_empty() {
                info!(
                    "🎧 Recording contained only silence after fast VAD trimming; skipping transcription"
                );
                return Ok(None);
            }

            let FastVadOutcome { trimmed_audio, .. } = outcome;

            return Ok(Some(CapturedAudio {
                samples: trimmed_audio,
                sample_rate,
            }));
        }

        Ok(Some(CapturedAudio {
            samples,
            sample_rate,
        }))
    }

    async fn process_audio(&mut self, audio_data: CapturedAudio) -> Result<()> {
        let maybe_audio = self.preprocess_audio(audio_data)?;

        let Some(processed_audio) = maybe_audio else {
            return Ok(());
        };

        if processed_audio.is_empty() {
            info!("🎧 No audio remaining after preprocessing; skipping transcription");
            return Ok(());
        }

        let CapturedAudio {
            samples,
            sample_rate,
        } = processed_audio;

        let audio_for_transcription = if sample_rate == 16_000 {
            samples
        } else {
            debug!(
                "Resampling processed audio from {} Hz to 16 kHz for transcription backend (test mode)",
                sample_rate
            );
            resample_audio(&samples, sample_rate, 16_000)
        };

        let TranscriptionResult {
            text: transcription,
            ..
        } = self.transcriber.transcribe(audio_for_transcription).await?;

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

fn resample_audio(samples: &[f32], src_rate: u32, dst_rate: u32) -> Vec<f32> {
    if samples.is_empty() || src_rate == 0 || dst_rate == 0 {
        return Vec::new();
    }
    if src_rate == dst_rate {
        return samples.to_vec();
    }

    let src_len = samples.len();
    if src_len == 0 {
        return Vec::new();
    }

    let output_len = ((src_len as u64 * dst_rate as u64) + (src_rate as u64 / 2)) / src_rate as u64;
    if output_len == 0 {
        return Vec::new();
    }

    let mut output = Vec::with_capacity(output_len as usize);
    let rate_ratio = src_rate as f64 / dst_rate as f64;
    let last_index = src_len.saturating_sub(1);

    for n in 0..output_len as usize {
        let src_pos = n as f64 * rate_ratio;
        let idx = src_pos.floor() as usize;
        let frac = src_pos - idx as f64;
        let left = samples[idx.min(last_index)];
        let right = samples[(idx + 1).min(last_index)];
        let value = left + (right - left) * frac as f32;
        output.push(value);
    }

    output
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
