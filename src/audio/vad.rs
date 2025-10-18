use crate::config::FastVadConfig;
#[cfg(feature = "fast-vad")]
use crate::config::FastVadProfile;
#[cfg(feature = "fast-vad")]
use std::collections::VecDeque;
#[cfg(any(test, feature = "bench-fast-vad", feature = "fast-vad"))]
use std::time::Duration;
use tracing::debug;
#[cfg(feature = "fast-vad")]
use tracing::{trace, warn};

const TARGET_SAMPLE_RATE: u32 = 16_000;
const FRAME_MS: u32 = 30;

pub struct FastStreamTrimmer {
    config: FastVadConfig,
    #[cfg(feature = "fast-vad")]
    detector: earshot::VoiceActivityDetector,
}

impl FastStreamTrimmer {
    pub fn new(config: FastVadConfig) -> Self {
        Self {
            #[cfg(feature = "fast-vad")]
            detector: build_detector(&config),
            config,
        }
    }

    pub fn update_config(&mut self, config: FastVadConfig) {
        self.config = config;
        #[cfg(feature = "fast-vad")]
        {
            self.detector = build_detector(&self.config);
        }
    }

    pub fn trim(&mut self, audio: Vec<f32>, sample_rate: u32) -> Vec<f32> {
        if !self.config.enabled || audio.is_empty() {
            return audio;
        }

        #[cfg(feature = "fast-vad")]
        {
            return self.trim_enabled(audio, sample_rate);
        }

        #[cfg(not(feature = "fast-vad"))]
        {
            debug!("fast_vad feature disabled at compile time; returning original audio");
            audio
        }
    }

    #[cfg(feature = "fast-vad")]
    fn trim_enabled(&mut self, audio: Vec<f32>, sample_rate: u32) -> Vec<f32> {
        let pcm = if sample_rate == TARGET_SAMPLE_RATE {
            audio
        } else {
            warn!(
                "Input sample rate {}Hz differs from required {}Hz; resampling",
                sample_rate, TARGET_SAMPLE_RATE
            );
            resample_to_16khz(&audio, sample_rate)
        };

        let frame_samples = (TARGET_SAMPLE_RATE as usize * FRAME_MS as usize) / 1000;
        if frame_samples == 0 {
            return pcm;
        }

        self.detector.reset();

        let mut trimmed_frames: Vec<Vec<f32>> = Vec::new();
        let mut active_segment: Vec<Vec<f32>> = Vec::new();
        let mut trailing_silence: VecDeque<Vec<f32>> = VecDeque::new();
        let mut pre_buffer: VecDeque<Vec<f32>> = VecDeque::new();
        let mut pending_frames: Vec<Vec<f32>> = Vec::new();
        let mut speech_active = false;
        let mut pending_count = 0usize;
        let mut silence_counter = 0usize;
        let mut history: VecDeque<bool> = VecDeque::new();

        let base_min_speech_frames = frames_for_duration(self.config.min_speech_ms).max(1);
        let base_silence_frames = frames_for_duration(self.config.silence_timeout_ms).max(1);
        let pre_roll_frames = frames_for_duration(self.config.pre_roll_ms);
        let post_roll_frames = frames_for_duration(self.config.post_roll_ms);
        let volatility_window_frames = frames_for_duration(self.config.volatility_window_ms).max(1);

        let mut frame_i16 = vec![0i16; frame_samples];
        let mut last_error_reported = false;

        for frame in pcm.chunks(frame_samples) {
            if frame.len() != frame_samples {
                trace!(
                    "Skipping partial frame ({} samples) at end of buffer",
                    frame.len()
                );
                break;
            }

            for (dest, sample) in frame_i16.iter_mut().zip(frame.iter()) {
                *dest = float_to_i16(*sample);
            }

            let decision = match self.detector.predict_16khz(&frame_i16) {
                Ok(flag) => flag,
                Err(err) => {
                    if !last_error_reported {
                        warn!("Earshot VAD prediction failed: {}", err);
                        last_error_reported = true;
                    }
                    false
                }
            };

            if history.len() == volatility_window_frames {
                history.pop_front();
            }
            history.push_back(decision);
            let volatility = volatility(&history);
            let effective_min_speech = adjusted_frames(
                base_min_speech_frames,
                volatility,
                self.config.volatility_sensitivity,
            );
            let effective_silence = adjusted_frames(
                base_silence_frames,
                volatility * 0.5,
                self.config.volatility_sensitivity,
            )
            .max(post_roll_frames.max(1));

            let frame_vec = frame.to_vec();

            if !speech_active {
                if decision {
                    pending_frames.push(frame_vec);
                    pending_count += 1;
                    if pending_count >= effective_min_speech {
                        speech_active = true;
                        if pre_roll_frames > 0 {
                            active_segment.extend(pre_buffer.drain(..));
                        }
                        active_segment.append(&mut pending_frames);
                        pending_count = 0;
                        trailing_silence.clear();
                    }
                } else {
                    pending_frames.clear();
                    pending_count = 0;
                    if pre_roll_frames > 0 {
                        pre_buffer.push_back(frame_vec);
                        if pre_buffer.len() > pre_roll_frames {
                            pre_buffer.pop_front();
                        }
                    }
                }
                continue;
            }

            if decision {
                if !trailing_silence.is_empty() {
                    active_segment.extend(trailing_silence.drain(..));
                }
                active_segment.push(frame_vec);
                silence_counter = 0;
                continue;
            }

            silence_counter += 1;
            if post_roll_frames > 0 {
                trailing_silence.push_back(frame_vec);
                if trailing_silence.len() > post_roll_frames {
                    trailing_silence.pop_front();
                }
            }

            if silence_counter >= effective_silence {
                active_segment.extend(trailing_silence.drain(..));
                trimmed_frames.append(&mut active_segment);
                speech_active = false;
                silence_counter = 0;
                pre_buffer.clear();
                pending_frames.clear();
            }
        }

        if speech_active {
            active_segment.extend(trailing_silence.drain(..));
            trimmed_frames.append(&mut active_segment);
        }

        let mut trimmed = Vec::with_capacity(trimmed_frames.len() * frame_samples);
        for frame in trimmed_frames {
            trimmed.extend(frame);
        }

        trimmed
    }
}

#[cfg(feature = "fast-vad")]
fn build_detector(config: &FastVadConfig) -> earshot::VoiceActivityDetector {
    earshot::VoiceActivityDetector::new(match config.profile {
        FastVadProfile::Quality => earshot::VoiceActivityProfile::QUALITY,
        FastVadProfile::Aggressive => earshot::VoiceActivityProfile::AGGRESSIVE,
        FastVadProfile::VeryAggressive => earshot::VoiceActivityProfile::VERY_AGGRESSIVE,
        FastVadProfile::Turbo => earshot::VoiceActivityProfile::TURBO,
    })
}

fn frames_for_duration(duration_ms: u32) -> usize {
    if duration_ms == 0 {
        0
    } else {
        ((duration_ms as usize + FRAME_MS as usize - 1) / FRAME_MS as usize).max(1)
    }
}

#[cfg(feature = "fast-vad")]
fn adjusted_frames(base: usize, volatility: f32, sensitivity: f32) -> usize {
    let factor = 1.0 + (volatility * sensitivity).clamp(0.0, 4.0);
    ((base as f32 * factor).ceil() as usize).max(1)
}

#[cfg(feature = "fast-vad")]
fn volatility(history: &VecDeque<bool>) -> f32 {
    if history.len() <= 1 {
        return 0.0;
    }

    let transitions = history
        .iter()
        .zip(history.iter().skip(1))
        .filter(|(a, b)| a != b)
        .count();
    transitions as f32 / (history.len() - 1) as f32
}

fn float_to_i16(sample: f32) -> i16 {
    let clamped = sample.clamp(-1.0, 1.0);
    (clamped * i16::MAX as f32).round() as i16
}

fn resample_to_16khz(audio: &[f32], sample_rate: u32) -> Vec<f32> {
    if sample_rate == 0 || audio.is_empty() {
        return Vec::new();
    }

    if sample_rate == TARGET_SAMPLE_RATE {
        return audio.to_vec();
    }

    let src_rate = sample_rate as f32;
    let dst_rate = TARGET_SAMPLE_RATE as f32;
    let ratio = dst_rate / src_rate;
    let new_len = ((audio.len() as f32) * ratio).round() as usize;
    if new_len == 0 {
        return Vec::new();
    }

    let mut resampled = Vec::with_capacity(new_len);
    for i in 0..new_len {
        let src_pos = i as f32 / ratio;
        let idx = src_pos.floor() as usize;
        if idx >= audio.len() - 1 {
            resampled.push(*audio.last().unwrap());
        } else {
            let frac = src_pos - idx as f32;
            let a = audio[idx];
            let b = audio[idx + 1];
            resampled.push(a + (b - a) * frac);
        }
    }

    resampled
}

#[cfg(all(any(test, feature = "bench-fast-vad"), feature = "fast-vad"))]
pub fn benchmark_fast_vad(
    audio: &[f32],
    sample_rate: u32,
    iterations: usize,
    config: &FastVadConfig,
) -> (Duration, Duration) {
    let mut trimmer = FastStreamTrimmer::new(config.clone());
    let mut fast_elapsed = Duration::default();
    for _ in 0..iterations.max(1) {
        let start = std::time::Instant::now();
        let _ = trimmer.trim(audio.to_vec(), sample_rate);
        fast_elapsed += start.elapsed();
    }

    let mut baseline_elapsed = Duration::default();
    for _ in 0..iterations.max(1) {
        let start = std::time::Instant::now();
        let mut copy = Vec::with_capacity(audio.len());
        copy.extend_from_slice(audio);
        baseline_elapsed += start.elapsed();
    }

    (fast_elapsed, baseline_elapsed)
}

#[cfg(all(any(test, feature = "bench-fast-vad"), not(feature = "fast-vad")))]
pub fn benchmark_fast_vad(
    _audio: &[f32],
    _sample_rate: u32,
    _iterations: usize,
    _config: &FastVadConfig,
) -> (Duration, Duration) {
    (Duration::default(), Duration::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passthrough_when_disabled() {
        let mut trimmer = FastStreamTrimmer::new(FastVadConfig::default());
        let samples = vec![0.1f32, 0.2, -0.3];
        let out = trimmer.trim(samples.clone(), TARGET_SAMPLE_RATE);
        assert_eq!(out, samples);
    }
}

#[cfg(all(test, feature = "fast-vad"))]
mod fast_tests {
    use super::*;

    fn sine_wave(ms: u32, sample_rate: u32, freq: f32) -> Vec<f32> {
        let samples = (sample_rate as f32 * ms as f32 / 1000.0).round() as usize;
        (0..samples)
            .map(|i| {
                let t = i as f32 / sample_rate as f32;
                (2.0 * std::f32::consts::PI * freq * t).sin() * 0.5
            })
            .collect()
    }

    fn silence(ms: u32, sample_rate: u32) -> Vec<f32> {
        let samples = (sample_rate as f32 * ms as f32 / 1000.0).round() as usize;
        vec![0.0; samples]
    }

    #[test]
    fn trims_silence_and_keeps_padding() {
        let mut config = FastVadConfig::default();
        config.enabled = true;
        config.pre_roll_ms = 90;
        config.post_roll_ms = 150;
        config.min_speech_ms = 120;
        config.silence_timeout_ms = 400;

        let mut audio = silence(500, TARGET_SAMPLE_RATE);
        audio.extend(sine_wave(800, TARGET_SAMPLE_RATE, 220.0));
        audio.extend(silence(600, TARGET_SAMPLE_RATE));

        let mut trimmer = FastStreamTrimmer::new(config.clone());
        let trimmed = trimmer.trim(audio.clone(), TARGET_SAMPLE_RATE);

        assert!(trimmed.len() < audio.len());

        let expected_min = (config.min_speech_ms + config.pre_roll_ms + config.post_roll_ms)
            * TARGET_SAMPLE_RATE as u32
            / 1000;
        assert!(trimmed.len() as u32 >= expected_min);
    }

    #[test]
    fn drops_all_silence() {
        let mut config = FastVadConfig::default();
        config.enabled = true;
        let audio = silence(800, TARGET_SAMPLE_RATE);
        let mut trimmer = FastStreamTrimmer::new(config);
        let trimmed = trimmer.trim(audio, TARGET_SAMPLE_RATE);
        assert!(trimmed.is_empty());
    }

    #[test]
    fn benchmark_hook_runs() {
        let mut config = FastVadConfig::default();
        config.enabled = true;
        let mut audio = silence(200, TARGET_SAMPLE_RATE);
        audio.extend(sine_wave(400, TARGET_SAMPLE_RATE, 440.0));
        let (fast, baseline) = super::benchmark_fast_vad(&audio, TARGET_SAMPLE_RATE, 2, &config);
        println!("earshot_fast_vad: {:?} | passthrough: {:?}", fast, baseline);
        assert!(fast > Duration::ZERO);
        assert!(baseline > Duration::ZERO);
    }
}
