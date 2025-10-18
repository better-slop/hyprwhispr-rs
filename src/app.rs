use anyhow::{Context, Result};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread::{self, JoinHandle};
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, error, info, warn};

use crate::audio::{
    capture::RecordingSession, AudioCapture, AudioFeedback, CapturedAudio, FastVad, FastVadOutcome,
};
use crate::config::{Config, ConfigManager, ShortcutsConfig};
use crate::input::{GlobalShortcuts, ShortcutEvent, ShortcutKind, ShortcutPhase, TextInjector};
use crate::status::StatusWriter;
use crate::transcription::TranscriptionBackend;
use crate::whisper::WhisperVadOptions;

struct ShortcutListener {
    stop_flag: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
    shortcut: String,
    kind: ShortcutKind,
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

impl ShortcutListener {
    fn spawn(
        shortcut: String,
        kind: ShortcutKind,
        tx: mpsc::Sender<ShortcutEvent>,
    ) -> Result<Self> {
        let stop_flag = Arc::new(AtomicBool::new(false));
        let runner_flag = Arc::clone(&stop_flag);
        let runner_tx = tx.clone();
        let shortcut_name = shortcut.clone();

        let handle = thread::spawn(move || match GlobalShortcuts::new(&shortcut, kind) {
            Ok(shortcuts) => {
                if let Err(e) = shortcuts.run(runner_tx, runner_flag) {
                    error!("Global shortcuts error: {}", e);
                }
            }
            Err(e) => {
                error!("Failed to initialize global shortcuts: {}", e);
            }
        });

        Ok(Self {
            stop_flag,
            handle: Some(handle),
            shortcut: shortcut_name,
            kind,
        })
    }

    fn restart(
        &mut self,
        shortcut: String,
        kind: ShortcutKind,
        tx: mpsc::Sender<ShortcutEvent>,
    ) -> Result<()> {
        self.stop();
        *self = Self::spawn(shortcut, kind, tx)?;
        Ok(())
    }

    fn stop(&mut self) {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            if let Err(err) = handle.join() {
                error!("Shortcut listener thread panicked: {:?}", err);
            }
        }
    }

    fn matches(&self, shortcut: &str, kind: ShortcutKind) -> bool {
        self.shortcut == shortcut && self.kind == kind
    }
}

impl Drop for ShortcutListener {
    fn drop(&mut self) {
        self.stop();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecordingTrigger {
    HoldShortcut,
    PressShortcut,
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

pub struct HyprwhsprApp {
    config_manager: ConfigManager,
    audio_capture: AudioCapture,
    audio_feedback: AudioFeedback,
    transcriber: TranscriptionBackend,
    fast_vad: Option<FastVad>,
    text_injector: Arc<Mutex<TextInjector>>,
    status_writer: StatusWriter,
    shortcut_tx: mpsc::Sender<ShortcutEvent>,
    shortcut_rx: Option<mpsc::Receiver<ShortcutEvent>>,
    press_listener: Option<ShortcutListener>,
    hold_listener: Option<ShortcutListener>,
    current_config: Config,
    recording_session: Option<RecordingSession>,
    recording_trigger: Option<RecordingTrigger>,
    is_processing: bool,
}

impl HyprwhsprApp {
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

        let (shortcut_tx, shortcut_rx) = mpsc::channel(10);

        let fast_vad = FastVad::maybe_new(&config.fast_vad, audio_capture.sample_rate_hint())
            .context("Failed to initialize fast VAD pipeline")?;

        if let Some(vad) = &fast_vad {
            info!(
                "⚡ Earshot fast VAD enabled (profile: {}, silence timeout: {} ms)",
                vad.settings().base_profile,
                config.fast_vad.silence_timeout_ms
            );
        }

        Ok(Self {
            config_manager,
            audio_capture,
            audio_feedback,
            transcriber,
            fast_vad,
            text_injector: Arc::new(Mutex::new(text_injector)),
            status_writer,
            shortcut_tx,
            shortcut_rx: Some(shortcut_rx),
            press_listener: None,
            hold_listener: None,
            current_config: config,
            recording_session: None,
            recording_trigger: None,
            is_processing: false,
        })
    }

    pub async fn run(mut self) -> Result<()> {
        info!("🚀 hyprwhspr running!");

        let mut shortcut_rx = self
            .shortcut_rx
            .take()
            .expect("shortcut receiver already consumed");
        self.ensure_shortcut_listeners(self.current_config.shortcuts.clone())?;
        self.log_shortcut_configuration(&self.current_config.shortcuts);

        let mut config_rx = self.config_manager.subscribe();

        loop {
            tokio::select! {
                event = shortcut_rx.recv() => {
                    match event {
                        Some(event) => {
                            if let Err(e) = self.handle_shortcut(event).await {
                                error!("Error handling shortcut: {}", e);
                            }
                        }
                        None => {
                            info!("Shortcut channel closed");
                            break;
                        }
                    }
                }
                result = config_rx.changed() => {
                    match result {
                        Ok(()) => {
                            let updated = config_rx.borrow().clone();
                            if let Err(err) = self.apply_config_update(updated) {
                                error!("Failed to apply config update: {}", err);
                            }
                        }
                        Err(_) => {
                            info!("Configuration watcher closed");
                            break;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    fn ensure_shortcut_listeners(&mut self, shortcuts: ShortcutsConfig) -> Result<()> {
        self.ensure_listener(ShortcutKind::Press, shortcuts.press.clone())?;
        self.ensure_listener(ShortcutKind::Hold, shortcuts.hold.clone())
    }

    fn ensure_listener(&mut self, kind: ShortcutKind, shortcut: Option<String>) -> Result<()> {
        let slot = match kind {
            ShortcutKind::Press => &mut self.press_listener,
            ShortcutKind::Hold => &mut self.hold_listener,
        };

        match shortcut {
            Some(ref target) => {
                if let Some(listener) = slot {
                    if listener.matches(target, kind) {
                        return Ok(());
                    }
                    listener.restart(target.clone(), kind, self.shortcut_tx.clone())?;
                } else {
                    let listener =
                        ShortcutListener::spawn(target.clone(), kind, self.shortcut_tx.clone())?;
                    *slot = Some(listener);
                }
            }
            None => {
                if let Some(listener) = slot.as_mut() {
                    listener.stop();
                }
                *slot = None;
            }
        }

        Ok(())
    }

    fn apply_config_update(&mut self, new_config: Config) -> Result<()> {
        tracing::debug!(?new_config, "Apply config update requested");
        if new_config == self.current_config {
            tracing::debug!("Config unchanged; ignoring update");
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

        let shortcuts_changed = new_config.shortcuts != self.current_config.shortcuts
            || self.press_listener.is_none()
            || (new_config.hold_shortcut().is_some() && self.hold_listener.is_none());

        if shortcuts_changed {
            self.ensure_shortcut_listeners(new_config.shortcuts.clone())?;
            self.log_shortcut_configuration(&new_config.shortcuts);
        }

        if self.current_config.fast_vad != new_config.fast_vad {
            self.fast_vad =
                FastVad::maybe_new(&new_config.fast_vad, self.audio_capture.sample_rate_hint())
                    .context("Failed to refresh fast VAD pipeline")?;
            if let Some(vad) = &self.fast_vad {
                info!(
                    "⚡ Earshot fast VAD enabled (profile: {}, silence timeout: {} ms)",
                    vad.settings().base_profile,
                    new_config.fast_vad.silence_timeout_ms
                );
            } else {
                info!("⚡ Earshot fast VAD disabled");
            }
        }

        self.text_injector = Arc::new(Mutex::new(text_injector));
        self.audio_feedback = audio_feedback;
        self.current_config = new_config;

        info!("Configuration updated");
        tracing::debug!(?self.current_config, "Config state after update");
        Ok(())
    }

    fn log_shortcut_configuration(&self, shortcuts: &ShortcutsConfig) {
        match shortcuts.press.as_deref() {
            Some(value) => info!("Press shortcut active: {}", value),
            None => info!("Press shortcut disabled"),
        }

        match shortcuts.hold.as_deref() {
            Some(value) => info!("Hold shortcut active: {}", value),
            None => info!("Hold shortcut disabled"),
        }
    }

    async fn handle_shortcut(&mut self, event: ShortcutEvent) -> Result<()> {
        match (event.kind, event.phase) {
            (ShortcutKind::Press, ShortcutPhase::Start) => {
                if self.is_processing {
                    warn!("Still processing previous recording, ignoring shortcut");
                    return Ok(());
                }

                if self.recording_session.is_some() {
                    self.stop_recording().await?;
                } else {
                    self.start_recording(RecordingTrigger::PressShortcut)
                        .await?;
                }
            }
            (ShortcutKind::Hold, ShortcutPhase::Start) => {
                if self.is_processing {
                    warn!("Still processing previous recording, ignoring hold shortcut");
                    return Ok(());
                }

                if self.recording_session.is_some() {
                    debug!("Hold shortcut ignored because recording is already active");
                } else {
                    self.start_recording(RecordingTrigger::HoldShortcut).await?;
                }
            }
            (ShortcutKind::Hold, ShortcutPhase::End) => {
                if matches!(self.recording_trigger, Some(RecordingTrigger::HoldShortcut))
                    && self.recording_session.is_some()
                {
                    self.stop_recording().await?;
                } else {
                    debug!("Hold release ignored (no active hold-triggered recording)");
                }
            }
            _ => {}
        }

        Ok(())
    }

    async fn start_recording(&mut self, trigger: RecordingTrigger) -> Result<()> {
        info!("🎤 Starting recording...");

        self.audio_feedback.play_start_sound()?;

        let session = self
            .audio_capture
            .start_recording()
            .context("Failed to start recording")?;

        self.recording_session = Some(session);
        self.recording_trigger = Some(trigger);

        self.status_writer.set_recording(true)?;

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
        self.recording_trigger = None;

        if !captured_audio.is_empty() {
            self.is_processing = true;
            if let Err(e) = self.process_audio(captured_audio).await {
                error!("❌ Error processing audio: {:#}", e);
                // Show user-friendly error notification
                warn!("Failed to process recording. Check logs for details.");
            }
            self.is_processing = false;
        } else {
            warn!("No audio data captured");
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
                    "🎚️ Input sample rate {} Hz unsupported by fast VAD; resampling to 16 kHz",
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

            let FastVadOutcome {
                trimmed_audio,
                segments,
                profile_switches,
                final_profile,
                dropped_samples,
                ..
            } = outcome;

            debug!(
                "Earshot fast VAD kept {}/{} samples across {} segments (profile={}, switches={}, dropped={})",
                trimmed_audio.len(),
                samples.len(),
                segments,
                final_profile,
                profile_switches,
                dropped_samples
            );

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
                "Resampling processed audio from {} Hz to 16 kHz for transcription backend",
                sample_rate
            );
            resample_audio(&samples, sample_rate, 16_000)
        };

        let transcription = self.transcriber.transcribe(audio_for_transcription).await?;

        if transcription.trim().is_empty() {
            warn!("Empty transcription, nothing to inject");
            return Ok(());
        }

        info!("📝 Transcription: \"{}\"", transcription);

        let text_injector = Arc::clone(&self.text_injector);
        let mut injector = text_injector.lock().await;

        debug!("⌨️  Injecting text into active application...");
        injector.inject_text(&transcription).await?;

        Ok(())
    }

    pub async fn cleanup(&mut self) -> Result<()> {
        info!("🧹 Cleaning up...");

        if self.recording_session.is_some() {
            self.status_writer.set_recording(false)?;
            self.recording_session = None;
        }

        if let Some(listener) = &mut self.press_listener {
            listener.stop();
        }
        self.press_listener = None;

        if let Some(listener) = &mut self.hold_listener {
            listener.stop();
        }
        self.hold_listener = None;
        self.recording_trigger = None;

        info!("✅ Cleanup completed");
        Ok(())
    }
}
