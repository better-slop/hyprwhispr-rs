use hyprwhspr_rs::audio::FastVad;
use hyprwhspr_rs::config::FastVadConfig;
use std::f32::consts::PI;

const SAMPLE_RATE_HZ: u32 = 16_000;

fn silence_ms(duration_ms: u32) -> Vec<f32> {
    let samples = (SAMPLE_RATE_HZ as u64 * duration_ms as u64 / 1000) as usize;
    vec![0.0; samples]
}

fn tone_ms(duration_ms: u32) -> Vec<f32> {
    let samples = (SAMPLE_RATE_HZ as u64 * duration_ms as u64 / 1000) as usize;
    let mut buffer = Vec::with_capacity(samples);
    for n in 0..samples {
        let phase = (n as f32 / SAMPLE_RATE_HZ as f32) * 2.0 * PI * 440.0;
        buffer.push((phase.sin() * 0.5).clamp(-1.0, 1.0));
    }
    buffer
}

#[test]
fn trims_segments_and_preserves_padding() {
    let config = FastVadConfig {
        enabled: true,
        min_speech_ms: 90,
        ..Default::default()
    };

    let mut vad = FastVad::maybe_new(&config)
        .expect("fast VAD initialization should succeed")
        .expect("fast VAD should be enabled");

    let mut audio = Vec::new();
    audio.extend(silence_ms(350));
    audio.extend(tone_ms(520));
    audio.extend(silence_ms(680));
    audio.extend(tone_ms(430));
    audio.extend(silence_ms(250));

    let outcome = vad.trim(&audio).expect("fast VAD should process audio");

    assert!(outcome.segments >= 1);
    assert!(outcome.trimmed_audio.len() < audio.len());
    assert!(outcome.dropped_samples > 0);
}

#[test]
fn silence_short_circuits_transmission() {
    let config = FastVadConfig {
        enabled: true,
        ..Default::default()
    };

    let mut vad = FastVad::maybe_new(&config)
        .expect("fast VAD initialization should succeed")
        .expect("fast VAD should be enabled");

    let audio = silence_ms(1200);
    let outcome = vad.trim(&audio).expect("fast VAD should process silence");
    assert_eq!(outcome.trimmed_audio.len(), 0);
    assert_eq!(outcome.segments, 0);
    assert_eq!(outcome.dropped_samples, audio.len());
}
