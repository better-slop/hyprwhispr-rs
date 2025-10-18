use std::collections::VecDeque;

use anyhow::{Context, Result};
use earshot::vad::{VoiceActivityDetector, VoiceActivityProfile};

use crate::config::{FastVadConfig, FastVadProfile};

const FRAME_DURATION_MS: u32 = 30;

fn clamp_sample(value: f32) -> i16 {
    let scaled = (value.clamp(-1.0, 1.0) * i16::MAX as f32).round();
    scaled as i16
}

fn to_voice_profile(profile: &FastVadProfile) -> VoiceActivityProfile {
    match profile {
        FastVadProfile::Quality => VoiceActivityProfile::QUALITY,
        FastVadProfile::LowBitrate => VoiceActivityProfile::LOW_BITRATE,
        FastVadProfile::Aggressive => VoiceActivityProfile::AGGRESSIVE,
        FastVadProfile::VeryAggressive => VoiceActivityProfile::VERY_AGGRESSIVE,
    }
}

pub trait FramePredictor {
    fn predict(&mut self, frame: &[i16]) -> Result<bool>;
    fn set_profile(&mut self, profile: VoiceActivityProfile);
    fn current_profile(&self) -> VoiceActivityProfile;
    fn reset(&mut self);
}

pub struct EarshotDetector {
    detector: VoiceActivityDetector,
    profile: VoiceActivityProfile,
}

impl EarshotDetector {
    pub fn new(profile: VoiceActivityProfile) -> Self {
        Self {
            detector: VoiceActivityDetector::new(profile),
            profile,
        }
    }
}

impl FramePredictor for EarshotDetector {
    fn predict(&mut self, frame: &[i16]) -> Result<bool> {
        self.detector
            .predict_16khz(frame)
            .with_context(|| "earshot VAD prediction failed")
    }

    fn set_profile(&mut self, profile: VoiceActivityProfile) {
        if self.profile != profile {
            self.detector = VoiceActivityDetector::new(profile);
            self.profile = profile;
        }
    }

    fn current_profile(&self) -> VoiceActivityProfile {
        self.profile
    }

    fn reset(&mut self) {
        self.detector.reset();
    }
}

#[derive(Debug, Clone)]
pub struct FastVadTrimmingConfig {
    pub enabled: bool,
    pub profile: FastVadProfile,
    pub min_speech_ms: u32,
    pub silence_timeout_ms: u32,
    pub pre_roll_ms: u32,
    pub post_roll_ms: u32,
    pub volatility_window: usize,
    pub volatility_increase_threshold: f32,
    pub volatility_decrease_threshold: f32,
}

impl Default for FastVadTrimmingConfig {
    fn default() -> Self {
        let config = FastVadConfig::default();
        FastVadTrimmingConfig::from(&config)
    }
}

impl From<&FastVadConfig> for FastVadTrimmingConfig {
    fn from(value: &FastVadConfig) -> Self {
        Self {
            enabled: value.enabled,
            profile: value.profile,
            min_speech_ms: value.min_speech_ms,
            silence_timeout_ms: value.silence_timeout_ms,
            pre_roll_ms: value.pre_roll_ms,
            post_roll_ms: value.post_roll_ms,
            volatility_window: value.volatility_window.max(2),
            volatility_increase_threshold: value.volatility_increase_threshold.clamp(0.0, 1.0),
            volatility_decrease_threshold: value.volatility_decrease_threshold.clamp(0.0, 1.0),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FastVadRuntimeConfig {
    pub enabled: bool,
    pub sample_rate: u32,
    pub frame_samples: usize,
    pub min_speech_frames: usize,
    pub silence_timeout_frames: usize,
    pub pre_roll_frames: usize,
    pub post_roll_frames: usize,
    pub volatility_window: usize,
    pub volatility_increase_threshold: f32,
    pub volatility_decrease_threshold: f32,
    pub initial_profile: VoiceActivityProfile,
}

impl FastVadRuntimeConfig {
    pub fn from_trimming(config: &FastVadTrimmingConfig, sample_rate: u32) -> Self {
        let frame_samples = ((sample_rate as u64 * FRAME_DURATION_MS as u64) / 1000) as usize;
        let frame_samples = frame_samples.max(1);
        let ms_to_frames = |ms: u32| -> usize {
            if ms == 0 {
                0
            } else {
                ((ms as u64 + FRAME_DURATION_MS as u64 - 1) / FRAME_DURATION_MS as u64) as usize
            }
        };

        let min_speech_frames = ms_to_frames(config.min_speech_ms).max(1);
        let silence_timeout_frames = ms_to_frames(config.silence_timeout_ms).max(1);
        let pre_roll_frames = ms_to_frames(config.pre_roll_ms);
        let post_roll_frames = ms_to_frames(config.post_roll_ms);

        let volatility_window = config.volatility_window.max(2);
        let inc = config.volatility_increase_threshold.max(0.0).min(1.0);
        let dec = config.volatility_decrease_threshold.max(0.0).min(1.0);

        Self {
            enabled: config.enabled,
            sample_rate,
            frame_samples,
            min_speech_frames,
            silence_timeout_frames,
            pre_roll_frames,
            post_roll_frames,
            volatility_window,
            volatility_increase_threshold: inc,
            volatility_decrease_threshold: dec.min(inc),
            initial_profile: to_voice_profile(&config.profile),
        }
    }
}

pub struct StreamingSilenceTrimmer<P: FramePredictor> {
    predictor: P,
    runtime: FastVadRuntimeConfig,
    base_profile: VoiceActivityProfile,
    current_profile: VoiceActivityProfile,
    history: VecDeque<bool>,
}

pub type EarshotStreamingTrimmer = StreamingSilenceTrimmer<EarshotDetector>;

impl StreamingSilenceTrimmer<EarshotDetector> {
    pub fn from_config(config: &FastVadConfig, sample_rate: u32) -> Self {
        let trimming = FastVadTrimmingConfig::from(config);
        let runtime = FastVadRuntimeConfig::from_trimming(&trimming, sample_rate);
        let predictor = EarshotDetector::new(runtime.initial_profile);
        Self::new(predictor, runtime)
    }
}

impl<P: FramePredictor> StreamingSilenceTrimmer<P> {
    pub fn new(predictor: P, runtime: FastVadRuntimeConfig) -> Self {
        let base = runtime.initial_profile;
        Self {
            predictor,
            runtime,
            base_profile: base,
            current_profile: base,
            history: VecDeque::with_capacity(runtime.volatility_window),
        }
    }

    pub fn trim(&mut self, audio: &[f32]) -> Result<Vec<f32>> {
        if !self.runtime.enabled || audio.is_empty() {
            return Ok(audio.to_vec());
        }

        self.history.clear();
        self.predictor.reset();
        self.current_profile = self.base_profile;
        self.predictor.set_profile(self.base_profile);

        let mut trimmed = Vec::with_capacity(audio.len());
        let mut pre_buffer: VecDeque<Vec<f32>> =
            VecDeque::with_capacity(self.runtime.pre_roll_frames);
        let mut candidate_frames: Vec<Vec<f32>> = Vec::new();
        let mut post_buffer: VecDeque<Vec<f32>> =
            VecDeque::with_capacity(self.runtime.post_roll_frames);

        let mut speech_active = false;
        let mut speech_frames = 0usize;
        let mut silence_frames = 0usize;

        let mut frame_i16 = vec![0i16; self.runtime.frame_samples];

        for chunk in audio.chunks(self.runtime.frame_samples) {
            let mut frame = vec![0f32; self.runtime.frame_samples];
            frame[..chunk.len()].copy_from_slice(chunk);

            for (idx, sample) in frame.iter().enumerate() {
                frame_i16[idx] = clamp_sample(*sample);
            }

            let decision = self.predictor.predict(&frame_i16)?;
            self.record_decision(decision);

            if decision {
                silence_frames = 0;
                if speech_active {
                    if !post_buffer.is_empty() {
                        for pending in post_buffer.drain(..) {
                            trimmed.extend_from_slice(&pending);
                        }
                    }
                    trimmed.extend_from_slice(&frame);
                } else {
                    speech_frames += 1;
                    candidate_frames.push(frame.clone());

                    if speech_frames >= self.runtime.min_speech_frames {
                        speech_active = true;
                        while let Some(pre) = pre_buffer.pop_front() {
                            trimmed.extend_from_slice(&pre);
                        }
                        for candidate in candidate_frames.drain(..) {
                            trimmed.extend_from_slice(&candidate);
                        }
                        post_buffer.clear();
                    }
                }
            } else if speech_active {
                silence_frames += 1;
                if self.runtime.post_roll_frames > 0 {
                    if post_buffer.len() == self.runtime.post_roll_frames {
                        post_buffer.pop_front();
                    }
                    post_buffer.push_back(frame.clone());
                }

                if silence_frames >= self.runtime.silence_timeout_frames {
                    while let Some(pending) = post_buffer.pop_front() {
                        trimmed.extend_from_slice(&pending);
                    }
                    speech_active = false;
                    speech_frames = 0;
                    candidate_frames.clear();
                    silence_frames = 0;
                    pre_buffer.clear();
                }
            } else {
                speech_frames = 0;
                candidate_frames.clear();
                if self.runtime.pre_roll_frames > 0 {
                    if pre_buffer.len() == self.runtime.pre_roll_frames {
                        pre_buffer.pop_front();
                    }
                    pre_buffer.push_back(frame.clone());
                }
            }
        }

        if speech_active {
            while let Some(pending) = post_buffer.pop_front() {
                trimmed.extend_from_slice(&pending);
            }
        }

        Ok(trimmed)
    }

    fn record_decision(&mut self, decision: bool) {
        if self.runtime.volatility_window == 0 {
            return;
        }

        if self.history.len() == self.runtime.volatility_window {
            self.history.pop_front();
        }
        self.history.push_back(decision);

        if self.history.len() < 2 {
            return;
        }

        let mut transitions = 0usize;
        let mut iter = self.history.iter();
        let mut prev = *iter.next().unwrap();
        for value in iter {
            if *value != prev {
                transitions += 1;
            }
            prev = *value;
        }

        if self.history.len() > 1 {
            let volatility = transitions as f32 / (self.history.len() - 1) as f32;
            self.adjust_profile(volatility);
        }
    }

    fn adjust_profile(&mut self, volatility: f32) {
        if volatility > self.runtime.volatility_increase_threshold {
            if let Some(next) = more_aggressive(self.current_profile) {
                self.predictor.set_profile(next);
                self.predictor.reset();
                self.current_profile = next;
            }
        } else if volatility < self.runtime.volatility_decrease_threshold {
            if let Some(prev) = less_aggressive(self.current_profile, self.base_profile) {
                self.predictor.set_profile(prev);
                self.predictor.reset();
                self.current_profile = prev;
            }
        }
    }
}

const PROFILE_LADDER: [VoiceActivityProfile; 4] = [
    VoiceActivityProfile::QUALITY,
    VoiceActivityProfile::LOW_BITRATE,
    VoiceActivityProfile::AGGRESSIVE,
    VoiceActivityProfile::VERY_AGGRESSIVE,
];

fn profile_index(profile: VoiceActivityProfile) -> usize {
    PROFILE_LADDER
        .iter()
        .position(|p| *p == profile)
        .unwrap_or(0)
}

fn more_aggressive(current: VoiceActivityProfile) -> Option<VoiceActivityProfile> {
    let idx = profile_index(current);
    PROFILE_LADDER.get(idx + 1).copied()
}

fn less_aggressive(
    current: VoiceActivityProfile,
    floor: VoiceActivityProfile,
) -> Option<VoiceActivityProfile> {
    let current_idx = profile_index(current);
    let floor_idx = profile_index(floor);
    if current_idx <= floor_idx {
        return None;
    }

    PROFILE_LADDER.get(current_idx.saturating_sub(1)).copied()
}

#[cfg(all(test, feature = "fast-vad"))]
mod tests {
    use super::*;

    struct MockDetector {
        decisions: Vec<bool>,
        index: usize,
        profile: VoiceActivityProfile,
    }

    impl MockDetector {
        fn new(decisions: Vec<bool>, profile: VoiceActivityProfile) -> Self {
            Self {
                decisions,
                index: 0,
                profile,
            }
        }
    }

    impl FramePredictor for MockDetector {
        fn predict(&mut self, _frame: &[i16]) -> Result<bool> {
            let decision = self.decisions.get(self.index).copied().unwrap_or(false);
            self.index += 1;
            Ok(decision)
        }

        fn set_profile(&mut self, profile: VoiceActivityProfile) {
            self.profile = profile;
        }

        fn current_profile(&self) -> VoiceActivityProfile {
            self.profile
        }

        fn reset(&mut self) {
            self.index = 0;
        }
    }

    fn runtime_config() -> FastVadRuntimeConfig {
        let trimming = FastVadTrimmingConfig {
            enabled: true,
            profile: FastVadProfile::Aggressive,
            min_speech_ms: 60,
            silence_timeout_ms: 90,
            pre_roll_ms: 30,
            post_roll_ms: 60,
            volatility_window: 8,
            volatility_increase_threshold: 0.4,
            volatility_decrease_threshold: 0.1,
        };
        FastVadRuntimeConfig::from_trimming(&trimming, 16_000)
    }

    fn frame(decision: bool, frame_samples: usize) -> Vec<f32> {
        if decision {
            vec![0.5; frame_samples]
        } else {
            vec![0.0; frame_samples]
        }
    }

    #[test]
    fn trims_leading_and_trailing_silence_with_padding() {
        let runtime = runtime_config();
        let mut decisions = Vec::new();
        decisions.extend(std::iter::repeat(false).take(5));
        decisions.extend(std::iter::repeat(true).take(6));
        decisions.extend(std::iter::repeat(false).take(10));

        let mut frames = Vec::new();
        for &decision in &decisions {
            frames.extend(frame(decision, runtime.frame_samples));
        }

        let detector = MockDetector::new(decisions, runtime.initial_profile);
        let mut trimmer = StreamingSilenceTrimmer::new(detector, runtime.clone());
        let trimmed = trimmer.trim(&frames).expect("trim succeeded");

        let expected_frames =
            runtime.pre_roll_frames + runtime.min_speech_frames + runtime.post_roll_frames;
        assert_eq!(trimmed.len(), expected_frames * runtime.frame_samples);
    }

    #[test]
    fn drops_short_speech_bursts() {
        let runtime = runtime_config();
        let mut decisions = Vec::new();
        decisions.extend(std::iter::repeat(false).take(3));
        decisions.extend(std::iter::repeat(true).take(1));
        decisions.extend(std::iter::repeat(false).take(6));

        let mut frames = Vec::new();
        for &decision in &decisions {
            frames.extend(frame(decision, runtime.frame_samples));
        }

        let detector = MockDetector::new(decisions, runtime.initial_profile);
        let mut trimmer = StreamingSilenceTrimmer::new(detector, runtime);
        let trimmed = trimmer.trim(&frames).expect("trim succeeded");
        assert!(trimmed.is_empty());
    }

    #[test]
    fn adaptive_profile_escalates_on_volatility() {
        let mut runtime = runtime_config();
        runtime.volatility_increase_threshold = 0.2;
        runtime.volatility_decrease_threshold = 0.05;

        let decisions = vec![true, false, true, false, true, false, true, false];
        let mut frames = Vec::new();
        for &decision in &decisions {
            frames.extend(frame(decision, runtime.frame_samples));
        }

        let detector = MockDetector::new(decisions, runtime.initial_profile);
        let mut trimmer = StreamingSilenceTrimmer::new(detector, runtime);
        let _ = trimmer.trim(&frames).expect("trim succeeded");
        assert_eq!(
            trimmer.current_profile,
            VoiceActivityProfile::VERY_AGGRESSIVE
        );
    }

    #[test]
    #[ignore]
    fn benchmark_earshot_vs_threshold() {
        use std::time::Instant;

        let runtime = runtime_config();
        let frames_count = 200;
        let mut frames = Vec::new();
        let mut decisions = Vec::new();
        for idx in 0..frames_count {
            let speech = idx % 4 == 0;
            decisions.push(speech);
            frames.extend(frame(speech, runtime.frame_samples));
        }

        let mut earshot_trimmer = StreamingSilenceTrimmer::new(
            MockDetector::new(decisions.clone(), runtime.initial_profile),
            runtime.clone(),
        );
        let start = Instant::now();
        let _ = earshot_trimmer.trim(&frames).unwrap();
        let earshot_elapsed = start.elapsed();

        let start = Instant::now();
        let _ = naive_energy_gate(&frames, runtime.frame_samples);
        let baseline_elapsed = start.elapsed();

        eprintln!(
            "earshot-inspired trimming: {:?}, naive energy gate: {:?}",
            earshot_elapsed, baseline_elapsed
        );
    }

    fn naive_energy_gate(audio: &[f32], frame_samples: usize) -> Vec<f32> {
        let mut output = Vec::new();
        for chunk in audio.chunks(frame_samples) {
            let energy: f32 = chunk.iter().map(|s| s * s).sum::<f32>() / chunk.len() as f32;
            if energy > 1e-4 {
                output.extend_from_slice(chunk);
            }
        }
        output
    }
}
