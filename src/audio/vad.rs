use std::collections::VecDeque;
use std::fmt;
#[cfg(any(test, feature = "bench"))]
use std::time::Duration;

use anyhow::{Context, Result};
use earshot::{VoiceActivityDetector, VoiceActivityProfile};

use crate::config::{FastVadConfig, FastVadProfileConfig};

const SAMPLE_RATE_HZ: u32 = 16_000;
const FRAME_MS: u32 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FastVadProfile {
    Quality,
    LowBitrate,
    Aggressive,
    VeryAggressive,
}

impl FastVadProfile {
    fn rank(self) -> u8 {
        match self {
            FastVadProfile::Quality => 0,
            FastVadProfile::LowBitrate => 1,
            FastVadProfile::Aggressive => 2,
            FastVadProfile::VeryAggressive => 3,
        }
    }

    fn more_aggressive(self) -> Option<Self> {
        match self {
            FastVadProfile::Quality => Some(FastVadProfile::LowBitrate),
            FastVadProfile::LowBitrate => Some(FastVadProfile::Aggressive),
            FastVadProfile::Aggressive => Some(FastVadProfile::VeryAggressive),
            FastVadProfile::VeryAggressive => None,
        }
    }

    fn less_aggressive(self) -> Option<Self> {
        match self {
            FastVadProfile::Quality => None,
            FastVadProfile::LowBitrate => Some(FastVadProfile::Quality),
            FastVadProfile::Aggressive => Some(FastVadProfile::LowBitrate),
            FastVadProfile::VeryAggressive => Some(FastVadProfile::Aggressive),
        }
    }
}

impl fmt::Display for FastVadProfile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let label = match self {
            FastVadProfile::Quality => "quality",
            FastVadProfile::LowBitrate => "low_bitrate",
            FastVadProfile::Aggressive => "aggressive",
            FastVadProfile::VeryAggressive => "very_aggressive",
        };
        write!(f, "{label}")
    }
}

impl From<FastVadProfileConfig> for FastVadProfile {
    fn from(value: FastVadProfileConfig) -> Self {
        match value {
            FastVadProfileConfig::Quality => FastVadProfile::Quality,
            FastVadProfileConfig::LowBitrate => FastVadProfile::LowBitrate,
            FastVadProfileConfig::Aggressive => FastVadProfile::Aggressive,
            FastVadProfileConfig::VeryAggressive => FastVadProfile::VeryAggressive,
        }
    }
}

impl From<FastVadProfile> for VoiceActivityProfile {
    fn from(value: FastVadProfile) -> Self {
        match value {
            FastVadProfile::Quality => VoiceActivityProfile::QUALITY,
            FastVadProfile::LowBitrate => VoiceActivityProfile::LBR,
            FastVadProfile::Aggressive => VoiceActivityProfile::AGGRESSIVE,
            FastVadProfile::VeryAggressive => VoiceActivityProfile::VERY_AGGRESSIVE,
        }
    }
}

#[derive(Debug, Clone)]
pub struct FastVadSettings {
    pub base_profile: FastVadProfile,
    pub min_speech_frames: usize,
    pub silence_timeout_frames: usize,
    pub pre_roll_frames: usize,
    pub post_roll_frames: usize,
    pub volatility_window: usize,
    pub volatility_increase_threshold: f32,
    pub volatility_decrease_threshold: f32,
}

impl FastVadSettings {
    pub fn from_config(config: &FastVadConfig) -> Self {
        let ms_to_frames = |ms: u32| -> usize {
            if ms == 0 {
                return 0;
            }
            ((ms + FRAME_MS - 1) / FRAME_MS) as usize
        };

        let min_speech_frames = ms_to_frames(config.min_speech_ms).max(1);
        let silence_timeout_frames = ms_to_frames(config.silence_timeout_ms).max(1);
        let pre_roll_frames = ms_to_frames(config.pre_roll_ms);
        let post_roll_frames = ms_to_frames(config.post_roll_ms);
        let volatility_window = config.volatility_window.max(2) as usize;

        Self {
            base_profile: FastVadProfile::from(config.profile),
            min_speech_frames,
            silence_timeout_frames,
            pre_roll_frames,
            post_roll_frames,
            volatility_window,
            volatility_increase_threshold: config.volatility_increase_threshold,
            volatility_decrease_threshold: config.volatility_decrease_threshold,
        }
    }
}

#[derive(Debug)]
pub struct FastVad {
    settings: FastVadSettings,
    detector: VoiceActivityDetector,
    current_profile: FastVadProfile,
    decision_history: VecDeque<bool>,
    profile_switches: usize,
    frame_samples: usize,
}

impl FastVad {
    pub fn maybe_new(config: &FastVadConfig) -> Result<Option<Self>> {
        if !config.enabled {
            return Ok(None);
        }

        let settings = FastVadSettings::from_config(config);
        Ok(Some(Self::with_settings(settings)))
    }

    pub fn with_settings(settings: FastVadSettings) -> Self {
        let frame_samples = ((SAMPLE_RATE_HZ as usize) * (FRAME_MS as usize)) / 1000;
        let detector = VoiceActivityDetector::new(settings.base_profile.into());

        Self {
            settings,
            detector,
            current_profile: settings.base_profile,
            decision_history: VecDeque::new(),
            profile_switches: 0,
            frame_samples,
        }
    }

    pub fn trim(&mut self, audio: &[f32]) -> Result<FastVadOutcome> {
        if audio.is_empty() {
            return Ok(FastVadOutcome {
                trimmed_audio: Vec::new(),
                segments: 0,
                evaluated_frames: 0,
                profile_switches: 0,
                final_profile: self.settings.base_profile,
                dropped_samples: 0,
            });
        }

        self.current_profile = self.settings.base_profile;
        self.detector = VoiceActivityDetector::new(self.current_profile.into());
        self.detector.reset();
        self.decision_history.clear();
        self.profile_switches = 0;

        let mut trimmed = Vec::with_capacity(audio.len());
        let mut active_segment = Vec::new();
        let mut pre_roll: VecDeque<Vec<f32>> =
            VecDeque::with_capacity(self.settings.pre_roll_frames.max(1));
        let mut pending_silence: VecDeque<(Vec<f32>, bool)> = VecDeque::new();
        let mut in_speech = false;
        let mut silence_frames = 0usize;
        let mut evaluated_frames = 0usize;
        let mut segments = 0usize;

        for chunk in audio.chunks(self.frame_samples) {
            let frame: Vec<f32> = chunk.to_vec();
            let pcm_frame = Self::convert_frame(&frame, self.frame_samples);
            let is_speech = self
                .detector
                .predict_16khz(&pcm_frame)
                .context("Earshot VAD failed to evaluate 16 kHz frame")?;
            evaluated_frames += 1;
            let volatility = self.push_decision(is_speech);
            self.adjust_profile(volatility);

            if !in_speech {
                if is_speech {
                    in_speech = true;
                    self.flush_pre_roll(&mut pre_roll, &mut active_segment);
                    if !pending_silence.is_empty() {
                        for (silence_frame, appended) in pending_silence.drain(..) {
                            if !appended {
                                active_segment.extend_from_slice(&silence_frame);
                            }
                        }
                    }
                    active_segment.extend_from_slice(&frame);
                    silence_frames = 0;
                } else {
                    self.push_pre_roll(&mut pre_roll, &frame);
                }
                continue;
            }

            if is_speech {
                if !pending_silence.is_empty() {
                    for (silence_frame, appended) in pending_silence.drain(..) {
                        if !appended {
                            active_segment.extend_from_slice(&silence_frame);
                        }
                    }
                }
                active_segment.extend_from_slice(&frame);
                silence_frames = 0;
                continue;
            }

            silence_frames += 1;
            let appended = if silence_frames <= self.settings.post_roll_frames {
                active_segment.extend_from_slice(&frame);
                true
            } else {
                false
            };
            pending_silence.push_back((frame.clone(), appended));

            if silence_frames >= self.settings.silence_timeout_frames {
                if !active_segment.is_empty() && active_segment.len() >= self.min_speech_samples() {
                    trimmed.extend_from_slice(&active_segment);
                    segments += 1;
                }
                active_segment.clear();

                if !pending_silence.is_empty() {
                    self.reseed_pre_roll(&mut pre_roll, &pending_silence);
                    pending_silence.clear();
                }

                in_speech = false;
                silence_frames = 0;
            }
        }

        if in_speech {
            if !pending_silence.is_empty() {
                for (silence_frame, appended) in pending_silence.drain(..) {
                    if !appended {
                        active_segment.extend_from_slice(&silence_frame);
                    }
                }
            }
            if !active_segment.is_empty() && active_segment.len() >= self.min_speech_samples() {
                trimmed.extend_from_slice(&active_segment);
                segments += 1;
            }
        }

        let dropped_samples = audio.len().saturating_sub(trimmed.len());

        Ok(FastVadOutcome {
            trimmed_audio: trimmed,
            segments,
            evaluated_frames,
            profile_switches: self.profile_switches,
            final_profile: self.current_profile,
            dropped_samples,
        })
    }

    pub fn settings(&self) -> &FastVadSettings {
        &self.settings
    }

    fn push_pre_roll(&self, pre_roll: &mut VecDeque<Vec<f32>>, frame: &[f32]) {
        if self.settings.pre_roll_frames == 0 {
            return;
        }
        if pre_roll.len() == self.settings.pre_roll_frames {
            pre_roll.pop_front();
        }
        pre_roll.push_back(frame.to_vec());
    }

    fn flush_pre_roll(&self, pre_roll: &mut VecDeque<Vec<f32>>, active_segment: &mut Vec<f32>) {
        while let Some(frame) = pre_roll.pop_front() {
            active_segment.extend_from_slice(&frame);
        }
    }

    fn reseed_pre_roll(
        &self,
        pre_roll: &mut VecDeque<Vec<f32>>,
        pending: &VecDeque<(Vec<f32>, bool)>,
    ) {
        pre_roll.clear();
        if self.settings.pre_roll_frames == 0 || pending.is_empty() {
            return;
        }
        let count = pending.len().min(self.settings.pre_roll_frames);
        let skip = pending.len().saturating_sub(count);
        for (frame, _) in pending.iter().skip(skip) {
            pre_roll.push_back(frame.clone());
        }
    }

    fn push_decision(&mut self, decision: bool) -> f32 {
        self.decision_history.push_back(decision);
        if self.decision_history.len() > self.settings.volatility_window {
            self.decision_history.pop_front();
        }
        if self.decision_history.len() < 2 {
            return 0.0;
        }
        let mut transitions = 0usize;
        let mut iter = self.decision_history.iter();
        let mut prev = *iter.next().unwrap();
        for &value in iter {
            if value != prev {
                transitions += 1;
            }
            prev = value;
        }
        transitions as f32 / (self.decision_history.len() - 1) as f32
    }

    fn adjust_profile(&mut self, volatility: f32) {
        if volatility > self.settings.volatility_increase_threshold {
            if let Some(next) = self.current_profile.more_aggressive() {
                self.set_profile(next);
            }
        } else if volatility < self.settings.volatility_decrease_threshold {
            if let Some(prev) = self.current_profile.less_aggressive() {
                if prev.rank() >= self.settings.base_profile.rank() {
                    self.set_profile(prev);
                }
            }
        }
    }

    fn set_profile(&mut self, profile: FastVadProfile) {
        if profile == self.current_profile {
            return;
        }
        self.current_profile = profile;
        self.detector = VoiceActivityDetector::new(profile.into());
        self.detector.reset();
        self.decision_history.clear();
        self.profile_switches += 1;
    }

    fn min_speech_samples(&self) -> usize {
        self.settings.min_speech_frames * self.frame_samples
    }

    fn convert_frame(frame: &[f32], target_len: usize) -> Vec<i16> {
        let mut pcm = Vec::with_capacity(target_len);
        for &sample in frame.iter() {
            let scaled = (sample * i16::MAX as f32).round();
            let clamped = scaled.clamp(i16::MIN as f32, i16::MAX as f32);
            pcm.push(clamped as i16);
        }
        while pcm.len() < target_len {
            pcm.push(0);
        }
        pcm
    }
}

#[derive(Debug, Clone)]
pub struct FastVadOutcome {
    pub trimmed_audio: Vec<f32>,
    pub segments: usize,
    pub evaluated_frames: usize,
    pub profile_switches: usize,
    pub final_profile: FastVadProfile,
    pub dropped_samples: usize,
}

impl FastVadOutcome {
    pub fn is_empty(&self) -> bool {
        self.trimmed_audio.is_empty()
    }
}

#[cfg(any(test, feature = "bench"))]
#[derive(Debug, Clone)]
pub struct FastVadBenchmark {
    pub fast_duration: Duration,
    pub baseline_duration: Duration,
    pub original_samples: usize,
    pub trimmed_samples: usize,
    pub profile_switches: usize,
    pub segments: usize,
}

#[cfg(any(test, feature = "bench"))]
pub fn benchmark_against_passthrough(
    audio: &[f32],
    settings: &FastVadSettings,
) -> Result<FastVadBenchmark> {
    use std::time::Instant;

    let mut fast_vad = FastVad::with_settings(settings.clone());
    let fast_start = Instant::now();
    let outcome = fast_vad.trim(audio)?;
    let fast_duration = fast_start.elapsed();

    let baseline_start = Instant::now();
    let baseline = audio.to_vec();
    let baseline_duration = baseline_start.elapsed();

    Ok(FastVadBenchmark {
        fast_duration,
        baseline_duration,
        original_samples: baseline.len(),
        trimmed_samples: outcome.trimmed_audio.len(),
        profile_switches: outcome.profile_switches,
        segments: outcome.segments,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::FastVadConfig;

    fn silence_ms(duration_ms: u32) -> Vec<f32> {
        let samples = (SAMPLE_RATE_HZ as u64 * duration_ms as u64 / 1000) as usize;
        vec![0.0; samples]
    }

    fn tone_ms(duration_ms: u32) -> Vec<f32> {
        let samples = (SAMPLE_RATE_HZ as u64 * duration_ms as u64 / 1000) as usize;
        let mut buffer = Vec::with_capacity(samples);
        for n in 0..samples {
            let phase = (n as f32 / SAMPLE_RATE_HZ as f32) * 2.0 * std::f32::consts::PI * 220.0;
            buffer.push((phase.sin() * 0.6).clamp(-1.0, 1.0));
        }
        buffer
    }

    #[test]
    fn silence_stream_is_removed() -> Result<()> {
        let mut config = FastVadConfig::default();
        config.enabled = true;
        let mut vad = FastVad::maybe_new(&config)?.expect("fast VAD enabled");
        let audio = silence_ms(2000);
        let outcome = vad.trim(&audio)?;
        assert!(outcome.trimmed_audio.is_empty());
        assert_eq!(outcome.segments, 0);
        Ok(())
    }

    #[test]
    fn speech_keeps_padding_and_drops_long_silence() -> Result<()> {
        let mut config = FastVadConfig::default();
        config.enabled = true;
        config.min_speech_ms = 90;
        let mut vad = FastVad::maybe_new(&config)?.expect("fast VAD enabled");

        let mut audio = Vec::new();
        audio.extend(silence_ms(300));
        audio.extend(tone_ms(600));
        audio.extend(silence_ms(700));
        audio.extend(tone_ms(400));
        audio.extend(silence_ms(300));

        let outcome = vad.trim(&audio)?;
        assert!(!outcome.trimmed_audio.is_empty());
        assert!(outcome.segments >= 1);

        let trimmed_ms = outcome.trimmed_audio.len() as u64 * 1000 / SAMPLE_RATE_HZ as u64;
        let original_ms = audio.len() as u64 * 1000 / SAMPLE_RATE_HZ as u64;

        assert!(trimmed_ms < original_ms);
        assert!(trimmed_ms >= 900);
        Ok(())
    }

    #[test]
    fn volatility_triggers_profile_adjustment() -> Result<()> {
        let mut config = FastVadConfig::default();
        config.enabled = true;
        config.volatility_window = 12;
        config.volatility_increase_threshold = 0.15;
        config.volatility_decrease_threshold = 0.05;
        let mut vad = FastVad::maybe_new(&config)?.expect("fast VAD enabled");

        let mut audio = Vec::new();
        for _ in 0..40 {
            audio.extend(tone_ms(30));
            audio.extend(silence_ms(30));
        }

        let outcome = vad.trim(&audio)?;
        assert!(outcome.profile_switches > 0);
        Ok(())
    }

    #[test]
    fn benchmark_hook_runs() -> Result<()> {
        let mut config = FastVadConfig::default();
        config.enabled = true;
        let settings = FastVadSettings::from_config(&config);
        let audio = tone_ms(500);
        let metrics = super::benchmark_against_passthrough(&audio, &settings)?;
        assert!(metrics.fast_duration > Duration::ZERO);
        assert_eq!(metrics.original_samples, audio.len());
        assert!(metrics.trimmed_samples <= metrics.original_samples);
        Ok(())
    }
}
