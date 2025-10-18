#![cfg(feature = "fast-vad")]

use hyprwhspr_rs::audio::FastStreamTrimmer;
use hyprwhspr_rs::config::FastVadConfig;

const SAMPLE_RATE: u32 = 16_000;

fn silence(ms: u32) -> Vec<f32> {
    let samples = (SAMPLE_RATE as f32 * ms as f32 / 1000.0).round() as usize;
    vec![0.0; samples]
}

fn sine(ms: u32, freq: f32) -> Vec<f32> {
    let samples = (SAMPLE_RATE as f32 * ms as f32 / 1000.0).round() as usize;
    (0..samples)
        .map(|i| {
            let t = i as f32 / SAMPLE_RATE as f32;
            (2.0 * std::f32::consts::PI * freq * t).sin() * 0.5
        })
        .collect()
}

#[test]
fn integration_trims_with_expected_padding() {
    let mut config = FastVadConfig::default();
    config.enabled = true;
    config.pre_roll_ms = 90;
    config.post_roll_ms = 150;
    config.min_speech_ms = 120;
    config.silence_timeout_ms = 450;

    let mut audio = silence(600);
    audio.extend(sine(900, 440.0));
    audio.extend(silence(700));

    let mut trimmer = FastStreamTrimmer::new(config.clone());
    let trimmed = trimmer.trim(audio, SAMPLE_RATE);

    let frame_samples = (SAMPLE_RATE as usize * 30) / 1000; // 480
    let expected_frames = 3 + 30 + 5; // pre + speech + post
    assert_eq!(trimmed.len(), expected_frames * frame_samples);

    let leading_silence = &trimmed[..3 * frame_samples];
    assert!(leading_silence.iter().all(|s| s.abs() < 1e-6));

    let trailing_silence = &trimmed[trimmed.len() - 5 * frame_samples..];
    assert!(trailing_silence.iter().all(|s| s.abs() < 1e-6));
}

#[test]
fn integration_handles_multiple_segments() {
    let mut config = FastVadConfig::default();
    config.enabled = true;
    config.pre_roll_ms = 60;
    config.post_roll_ms = 90;
    config.min_speech_ms = 90;
    config.silence_timeout_ms = 360;

    let mut audio = silence(400);
    audio.extend(sine(500, 260.0));
    audio.extend(silence(600));
    audio.extend(sine(400, 520.0));
    audio.extend(silence(800));

    let mut trimmer = FastStreamTrimmer::new(config);
    let trimmed = trimmer.trim(audio, SAMPLE_RATE);

    assert!(!trimmed.is_empty());

    // Expect at least the speech frames plus configured padding
    let speech_ms = 500 + 400;
    let padding_ms = 2 * (60 + 90);
    let expected_min_samples =
        ((speech_ms + padding_ms) as f32 * SAMPLE_RATE as f32 / 1000.0) as usize;
    assert!(trimmed.len() >= expected_min_samples);
    // Should be substantially smaller than original buffer (~2.7s)
    assert!(trimmed.len() < (SAMPLE_RATE as usize * 2));
}
