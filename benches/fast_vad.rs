#![cfg(feature = "fast-vad")]

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use hyprwhspr_rs::audio::trim_buffer;
use hyprwhspr_rs::config::FastVadConfig;

const SAMPLE_RATE: u32 = 16_000;

fn generate_sample_audio() -> Vec<f32> {
    let mut audio = Vec::new();
    let segments = [(0.6, 0.0), (1.2, 0.18), (0.4, 0.0), (0.9, 0.16), (0.7, 0.0)];

    for (seconds, amplitude) in segments {
        let samples = (seconds * SAMPLE_RATE as f32) as usize;
        for n in 0..samples {
            if amplitude == 0.0 {
                audio.push(0.0);
            } else {
                let phase = 2.0 * std::f32::consts::PI * 180.0 * n as f32 / SAMPLE_RATE as f32;
                audio.push(phase.sin() * amplitude);
            }
        }
    }

    audio
}

fn bench_fast_vad(c: &mut Criterion) {
    let mut config = FastVadConfig::default();
    config.enabled = true;

    let audio = generate_sample_audio();

    c.bench_function("earshot_trim", |b| {
        b.iter(|| {
            let trimmed = trim_buffer(black_box(&audio), SAMPLE_RATE, black_box(&config)).unwrap();
            black_box(trimmed);
        });
    });

    c.bench_function("baseline_passthrough", |b| {
        b.iter(|| {
            black_box(audio.clone());
        });
    });
}

criterion_group!(fast_vad, bench_fast_vad);
criterion_main!(fast_vad);
