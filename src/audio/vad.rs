use std::cmp;
use std::collections::VecDeque;

use anyhow::{anyhow, ensure, Result};
use earshot::{VoiceActivityDetector, VoiceActivityProfile};

use crate::config::{FastVadConfig, FastVadProfile};

const TARGET_SAMPLE_RATE: u32 = 16_000;
const FRAME_MS: u32 = 30;
const VOLATILITY_HIGH: f32 = 0.55;
const VOLATILITY_LOW: f32 = 0.15;
const COOLDOWN_DIVISOR: usize = 2;

const PROFILE_ORDER: [VoiceActivityProfile; 4] = [
    VoiceActivityProfile::QUALITY,
    VoiceActivityProfile::LBR,
    VoiceActivityProfile::AGGRESSIVE,
    VoiceActivityProfile::VERY_AGGRESSIVE,
];

pub struct FastVadTrimmer {
    config: FastVadConfig,
}

impl FastVadTrimmer {
    pub fn new(config: &FastVadConfig) -> Self {
        Self {
            config: config.clone(),
        }
    }

    pub fn trim(&self, audio: &[f32], sample_rate: u32) -> Result<Vec<f32>> {
        if audio.is_empty() {
            return Ok(Vec::new());
        }

        ensure!(
            sample_rate == TARGET_SAMPLE_RATE,
            "fast VAD requires 16 kHz mono PCM input (got {sample_rate} Hz)"
        );

        let params = FastVadParams::new(sample_rate, &self.config)?;
        let start_index = profile_index(self.config.profile);
        let mut engine = EarshotEngine::new(PROFILE_ORDER[start_index]);
        let mut controller = AdaptiveController::new(start_index, self.config.volatility_window);

        process_stream(audio, &params, &mut engine, &mut controller)
    }
}

pub fn trim_buffer(audio: &[f32], sample_rate: u32, config: &FastVadConfig) -> Result<Vec<f32>> {
    FastVadTrimmer::new(config).trim(audio, sample_rate)
}

struct FastVadParams {
    frame_samples: usize,
    min_speech_frames: usize,
    silence_timeout_frames: usize,
    pre_roll_frames: usize,
    post_roll_frames: usize,
}

impl FastVadParams {
    fn new(sample_rate: u32, config: &FastVadConfig) -> Result<Self> {
        if sample_rate != TARGET_SAMPLE_RATE {
            return Err(anyhow!(
                "fast VAD requires {TARGET_SAMPLE_RATE} Hz input, got {sample_rate} Hz"
            ));
        }

        let frame_samples = (sample_rate as usize * FRAME_MS as usize) / 1000;
        if frame_samples == 0 {
            return Err(anyhow!("invalid frame configuration"));
        }

        Ok(Self {
            frame_samples,
            min_speech_frames: ms_to_frames(config.min_speech_ms),
            silence_timeout_frames: ms_to_frames(config.silence_timeout_ms),
            pre_roll_frames: ms_to_frames(config.pre_roll_ms),
            post_roll_frames: ms_to_frames(config.post_roll_ms),
        })
    }
}

fn ms_to_frames(ms: u32) -> usize {
    cmp::max(1, ((ms + FRAME_MS - 1) / FRAME_MS) as usize)
}

struct EarshotEngine {
    detector: VoiceActivityDetector,
}

impl EarshotEngine {
    fn new(profile: VoiceActivityProfile) -> Self {
        Self {
            detector: VoiceActivityDetector::new(profile),
        }
    }

    fn classify(&mut self, frame: &[i16]) -> Result<bool> {
        self.detector
            .predict_16khz(frame)
            .map_err(|err| anyhow!(err))
    }

    fn set_profile(&mut self, profile: VoiceActivityProfile) {
        self.detector = VoiceActivityDetector::new(profile);
    }
}

struct AdaptiveController {
    window: usize,
    history: VecDeque<bool>,
    current_index: usize,
    cooldown: usize,
}

impl AdaptiveController {
    fn new(start_index: usize, window: usize) -> Self {
        let window = window.max(6);
        Self {
            window,
            history: VecDeque::with_capacity(window),
            current_index: cmp::min(start_index, PROFILE_ORDER.len() - 1),
            cooldown: 0,
        }
    }

    fn observe(&mut self, decision: bool) -> Option<VoiceActivityProfile> {
        if self.window < 2 {
            return None;
        }

        if self.history.len() == self.window {
            self.history.pop_front();
        }
        self.history.push_back(decision);

        if self.history.len() < self.window {
            return None;
        }

        if self.cooldown > 0 {
            self.cooldown -= 1;
            return None;
        }

        let transitions = self
            .history
            .iter()
            .zip(self.history.iter().skip(1))
            .filter(|(a, b)| a != b)
            .count();
        let denom = self.history.len().saturating_sub(1).max(1);
        let volatility = transitions as f32 / denom as f32;

        if volatility > VOLATILITY_HIGH && self.current_index > 0 {
            self.current_index -= 1;
            self.cooldown = cmp::max(1, self.window / COOLDOWN_DIVISOR);
            self.history.clear();
            return Some(PROFILE_ORDER[self.current_index]);
        }

        if volatility < VOLATILITY_LOW && self.current_index + 1 < PROFILE_ORDER.len() {
            self.current_index += 1;
            self.cooldown = cmp::max(1, self.window / COOLDOWN_DIVISOR);
            self.history.clear();
            return Some(PROFILE_ORDER[self.current_index]);
        }

        None
    }
}

#[cfg(test)]
impl AdaptiveController {
    fn current_index(&self) -> usize {
        self.current_index
    }
}

enum VadState {
    Silence,
    MaybeSpeech {
        frames: Vec<Vec<f32>>,
    },
    Speech {
        post_buffer: Vec<Vec<f32>>,
        trailing: usize,
    },
}

fn process_stream<E: FramePredictor>(
    audio: &[f32],
    params: &FastVadParams,
    engine: &mut E,
    controller: &mut AdaptiveController,
) -> Result<Vec<f32>> {
    let mut output = Vec::with_capacity(audio.len());
    let mut frame_buffer = vec![0.0f32; params.frame_samples];
    let mut frame_i16 = vec![0i16; params.frame_samples];
    let mut pre_roll = VecDeque::with_capacity(params.pre_roll_frames);
    let mut state = VadState::Silence;

    for chunk in audio.chunks(params.frame_samples) {
        frame_buffer.fill(0.0);
        frame_buffer[..chunk.len()].copy_from_slice(chunk);
        for (dst, sample) in frame_i16.iter_mut().zip(frame_buffer.iter()) {
            *dst = (*sample * 32767.0).clamp(-32768.0, 32767.0) as i16;
        }

        let decision = engine.predict(&frame_i16)?;
        state = handle_frame(
            decision,
            frame_buffer.clone(),
            &mut output,
            &mut pre_roll,
            params,
            state,
        );

        if let Some(profile) = controller.observe(decision) {
            engine.set_profile(profile);
        }
    }

    finalize_state(state, &mut output, params);

    Ok(output)
}

fn handle_frame(
    decision: bool,
    frame: Vec<f32>,
    output: &mut Vec<f32>,
    pre_roll: &mut VecDeque<Vec<f32>>,
    params: &FastVadParams,
    state: VadState,
) -> VadState {
    match state {
        VadState::Silence => {
            if decision {
                VadState::MaybeSpeech {
                    frames: vec![frame],
                }
            } else {
                push_pre_roll(pre_roll, frame, params.pre_roll_frames);
                VadState::Silence
            }
        }
        VadState::MaybeSpeech { mut frames } => {
            if decision {
                frames.push(frame);
                if frames.len() >= params.min_speech_frames {
                    while let Some(buffer) = pre_roll.pop_front() {
                        output.extend_from_slice(&buffer);
                    }
                    for segment in frames {
                        output.extend_from_slice(&segment);
                    }
                    VadState::Speech {
                        post_buffer: Vec::new(),
                        trailing: 0,
                    }
                } else {
                    VadState::MaybeSpeech { frames }
                }
            } else {
                for segment in frames {
                    push_pre_roll(pre_roll, segment, params.pre_roll_frames);
                }
                push_pre_roll(pre_roll, frame, params.pre_roll_frames);
                VadState::Silence
            }
        }
        VadState::Speech {
            mut post_buffer,
            mut trailing,
        } => {
            if decision {
                if !post_buffer.is_empty() {
                    for buffer in post_buffer.drain(..) {
                        output.extend_from_slice(&buffer);
                    }
                    trailing = 0;
                }
                output.extend_from_slice(&frame);
                VadState::Speech {
                    post_buffer,
                    trailing,
                }
            } else {
                post_buffer.push(frame);
                trailing += 1;
                if trailing >= params.silence_timeout_frames {
                    let keep = cmp::min(params.post_roll_frames, post_buffer.len());
                    for buffer in post_buffer.into_iter().take(keep) {
                        output.extend_from_slice(&buffer);
                    }
                    pre_roll.clear();
                    VadState::Silence
                } else {
                    VadState::Speech {
                        post_buffer,
                        trailing,
                    }
                }
            }
        }
    }
}

fn finalize_state(state: VadState, output: &mut Vec<f32>, params: &FastVadParams) {
    if let VadState::Speech { post_buffer, .. } = state {
        let keep = cmp::min(params.post_roll_frames, post_buffer.len());
        for buffer in post_buffer.into_iter().take(keep) {
            output.extend_from_slice(&buffer);
        }
    }
}

fn push_pre_roll(pre_roll: &mut VecDeque<Vec<f32>>, frame: Vec<f32>, capacity: usize) {
    if capacity == 0 {
        return;
    }

    if pre_roll.len() == capacity {
        pre_roll.pop_front();
    }
    pre_roll.push_back(frame);
}

fn profile_index(profile: FastVadProfile) -> usize {
    match profile {
        FastVadProfile::Quality => 0,
        FastVadProfile::Lbr => 1,
        FastVadProfile::Aggressive => 2,
        FastVadProfile::VeryAggressive => 3,
    }
}

trait FramePredictor {
    fn predict(&mut self, frame: &[i16]) -> Result<bool>;
    fn set_profile(&mut self, profile: VoiceActivityProfile);
}

impl FramePredictor for EarshotEngine {
    fn predict(&mut self, frame: &[i16]) -> Result<bool> {
        self.classify(frame)
    }

    fn set_profile(&mut self, profile: VoiceActivityProfile) {
        EarshotEngine::set_profile(self, profile);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StubEngine {
        decisions: Vec<bool>,
        idx: usize,
    }

    impl StubEngine {
        fn new(decisions: Vec<bool>) -> Self {
            Self { decisions, idx: 0 }
        }
    }

    impl FramePredictor for StubEngine {
        fn predict(&mut self, _frame: &[i16]) -> Result<bool> {
            let decision = self.decisions.get(self.idx).copied().unwrap_or(false);
            self.idx += 1;
            Ok(decision)
        }

        fn set_profile(&mut self, _profile: VoiceActivityProfile) {}
    }

    fn synthetic_audio(total_frames: usize, speech_frames: &[usize]) -> Vec<f32> {
        let mut audio = Vec::new();
        for frame_idx in 0..total_frames {
            let mut frame = vec![0.0f32; (TARGET_SAMPLE_RATE as usize * FRAME_MS as usize) / 1000];
            if speech_frames.contains(&frame_idx) {
                for (i, sample) in frame.iter_mut().enumerate() {
                    *sample = (i as f32 / 10.0).sin() * 0.2;
                }
            }
            audio.extend_from_slice(&frame);
        }
        audio
    }

    #[test]
    fn trims_long_silence_and_keeps_padding() {
        let mut config = FastVadConfig::default();
        config.enabled = true;
        config.pre_roll_ms = 120;
        config.post_roll_ms = 150;
        config.min_speech_ms = 90;
        config.silence_timeout_ms = 600;

        let params = FastVadParams::new(TARGET_SAMPLE_RATE, &config).unwrap();
        let total_frames = 40;
        let speech_range: Vec<usize> = (10..20).collect();
        let audio = synthetic_audio(total_frames, &speech_range);
        let decisions: Vec<bool> = (0..total_frames)
            .map(|frame| speech_range.contains(&frame))
            .collect();

        let mut engine = StubEngine::new(decisions);
        let start_index = profile_index(config.profile);
        let mut controller = AdaptiveController::new(start_index, config.volatility_window);

        let output = process_stream(&audio, &params, &mut engine, &mut controller).unwrap();

        let expected_frames = speech_range.len()
            + params.pre_roll_frames
            + cmp::min(params.post_roll_frames, params.silence_timeout_frames);
        assert_eq!(output.len(), expected_frames * params.frame_samples);
    }

    #[test]
    fn drops_short_speech_bursts() {
        let mut config = FastVadConfig::default();
        config.enabled = true;
        config.min_speech_ms = 120;
        config.silence_timeout_ms = 500;

        let params = FastVadParams::new(TARGET_SAMPLE_RATE, &config).unwrap();
        let total_frames = 8;
        let speech_frames = vec![2];
        let audio = synthetic_audio(total_frames, &speech_frames);
        let decisions: Vec<bool> = (0..total_frames)
            .map(|frame| speech_frames.contains(&frame))
            .collect();

        let mut engine = StubEngine::new(decisions);
        let start_index = profile_index(config.profile);
        let mut controller = AdaptiveController::new(start_index, config.volatility_window);

        let output = process_stream(&audio, &params, &mut engine, &mut controller).unwrap();
        assert!(output.is_empty());
    }

    #[test]
    fn adaptive_controller_relaxes_on_chatter() {
        let mut controller = AdaptiveController::new(profile_index(FastVadProfile::Aggressive), 12);
        let decisions: Vec<bool> = vec![true, false].into_iter().cycle().take(48).collect();
        let params = FastVadParams::new(TARGET_SAMPLE_RATE, &FastVadConfig::default()).unwrap();

        let mut state = VadState::Silence;
        let frame = vec![0.0f32; params.frame_samples];
        let mut output = Vec::new();
        let mut pre_roll = VecDeque::new();

        for decision in decisions {
            state = handle_frame(
                decision,
                frame.clone(),
                &mut output,
                &mut pre_roll,
                &params,
                state,
            );
            let _ = controller.observe(decision);
        }

        assert!(controller.current_index() < profile_index(FastVadProfile::Aggressive));
    }
}
