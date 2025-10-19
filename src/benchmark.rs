use std::{
    fmt,
    time::{Duration, Instant},
};

use comfy_table::modifiers::UTF8_ROUND_CORNERS;
use comfy_table::presets::UTF8_FULL;
use comfy_table::{Cell, CellAlignment, ContentArrangement, Row, Table};

use crate::transcription::BackendMetrics;

const DASH: &str = "—";

pub struct BenchmarkRecorder {
    provider_label: String,
    keybind_start: Instant,
    keybind_stop: Option<Instant>,
    recording_start: Instant,
    recording_stop: Option<Instant>,
    processing_start: Option<Instant>,
    original_samples: Option<usize>,
    original_sample_rate: Option<u32>,
    trimmed_samples: Option<usize>,
    trimmed_sample_rate: Option<u32>,
    fast_vad_dropped_samples: Option<usize>,
    fast_vad_duration: Option<Duration>,
    preprocess_duration: Option<Duration>,
    encode_duration: Option<Duration>,
    encoded_bytes: Option<usize>,
    upload_duration: Option<Duration>,
    response_duration: Option<Duration>,
    transcription_duration: Option<Duration>,
    audio_sent_samples: Option<usize>,
    audio_sent_sample_rate: Option<u32>,
    injection_start: Option<Instant>,
    injection_finish: Option<Instant>,
    injection_duration: Option<Duration>,
}

impl BenchmarkRecorder {
    pub fn new(provider_label: String, keybind_start: Instant, recording_start: Instant) -> Self {
        Self {
            provider_label,
            keybind_start,
            keybind_stop: None,
            recording_start,
            recording_stop: None,
            processing_start: None,
            original_samples: None,
            original_sample_rate: None,
            trimmed_samples: None,
            trimmed_sample_rate: None,
            fast_vad_dropped_samples: None,
            fast_vad_duration: None,
            preprocess_duration: None,
            encode_duration: None,
            encoded_bytes: None,
            upload_duration: None,
            response_duration: None,
            transcription_duration: None,
            audio_sent_samples: None,
            audio_sent_sample_rate: None,
            injection_start: None,
            injection_finish: None,
            injection_duration: None,
        }
    }

    pub fn mark_keybind_stop(&mut self, at: Instant) {
        self.keybind_stop = Some(at);
    }

    pub fn mark_recording_stop(&mut self, at: Instant) {
        self.recording_stop = Some(at);
    }

    pub fn record_original_audio(&mut self, samples: usize, sample_rate: u32) {
        if sample_rate > 0 {
            self.original_samples = Some(samples);
            self.original_sample_rate = Some(sample_rate);
        }
    }

    pub fn mark_processing_start(&mut self, at: Instant) {
        self.processing_start = Some(at);
    }

    pub fn record_preprocess_duration(&mut self, duration: Duration) {
        self.preprocess_duration = Some(duration);
        self.fast_vad_duration = Some(duration);
    }

    pub fn record_trimmed_audio(
        &mut self,
        samples: usize,
        sample_rate: u32,
        dropped_samples: Option<usize>,
    ) {
        if sample_rate > 0 {
            self.trimmed_samples = Some(samples);
            self.trimmed_sample_rate = Some(sample_rate);
            if let Some(value) = dropped_samples {
                self.fast_vad_dropped_samples = Some(value);
            }
        }
    }

    pub fn record_audio_sent(&mut self, samples: usize, sample_rate: u32) {
        if sample_rate > 0 {
            self.audio_sent_samples = Some(samples);
            self.audio_sent_sample_rate = Some(sample_rate);
        }
    }

    pub fn record_backend_metrics(&mut self, metrics: BackendMetrics) {
        self.encode_duration = metrics.encode_duration;
        self.encoded_bytes = metrics.encoded_bytes;
        self.upload_duration = metrics.upload_duration;
        self.response_duration = metrics.response_duration;
        self.transcription_duration = Some(metrics.transcription_duration);
    }

    pub fn mark_injection_start(&mut self, at: Instant) {
        self.injection_start = Some(at);
    }

    pub fn mark_injection_end(&mut self, at: Instant) {
        if let Some(start) = self.injection_start {
            self.injection_duration = Some(at.saturating_duration_since(start));
        }
        self.injection_finish = Some(at);
    }

    pub fn mark_injection_skipped(&mut self, at: Instant) {
        self.injection_start = Some(at);
        self.injection_finish = Some(at);
        self.injection_duration = Some(Duration::from_secs(0));
    }

    pub(crate) fn finalize(self) -> Option<BenchmarkSummary> {
        let injection_finish = self.injection_finish?;

        let keybind_to_record_start_ms = diff_ms(self.keybind_start, self.recording_start);
        let recording_duration_ms = self
            .recording_stop
            .map(|stop| diff_ms(self.recording_start, stop));
        let stop_to_processing_ms = match (self.keybind_stop, self.processing_start) {
            (Some(stop), Some(processing)) => Some(diff_ms(stop, processing)),
            _ => None,
        };
        let preprocess_ms = self
            .preprocess_duration
            .map(|duration| duration.as_secs_f64() * 1000.0);
        let encode_ms = self
            .encode_duration
            .map(|duration| duration.as_secs_f64() * 1000.0);
        let upload_ms = self
            .upload_duration
            .map(|duration| duration.as_secs_f64() * 1000.0);
        let response_ms = self
            .response_duration
            .map(|duration| duration.as_secs_f64() * 1000.0);
        let transcription_ms = self
            .transcription_duration
            .map(|duration| duration.as_secs_f64() * 1000.0);
        let injection_ms = self
            .injection_duration
            .map(|duration| duration.as_secs_f64() * 1000.0);
        let total_ms = diff_ms(self.keybind_start, injection_finish);

        let original_audio_ms = audio_ms(self.original_samples, self.original_sample_rate);
        let original_audio_kb = raw_audio_kb(self.original_samples);
        let trimmed_audio_ms = audio_ms(self.trimmed_samples, self.trimmed_sample_rate);
        let trimmed_audio_kb = raw_audio_kb(self.trimmed_samples);
        let sent_audio_ms = audio_ms(self.audio_sent_samples, self.audio_sent_sample_rate);
        let sent_audio_kb = self.encoded_bytes.map(|bytes| bytes as f64 / 1024.0);

        let saved_audio_ms = match (original_audio_ms, trimmed_audio_ms) {
            (Some(original), Some(trimmed)) if original >= trimmed => Some(original - trimmed),
            _ => None,
        };

        let saved_audio_kb = match (original_audio_kb, trimmed_audio_kb) {
            (Some(original), Some(trimmed)) if original >= trimmed => Some(original - trimmed),
            _ => None,
        };

        let saved_audio_pct = match (saved_audio_ms, original_audio_ms) {
            (Some(saved), Some(original)) if original > 0.0 => Some((saved / original) * 100.0),
            _ => None,
        };

        let fast_vad_saved_time_ms = self
            .fast_vad_dropped_samples
            .zip(self.original_sample_rate)
            .map(|(samples, rate)| samples as f64 / rate as f64 * 1000.0)
            .or(saved_audio_ms);

        let fast_vad_trim_ms = preprocess_ms.or(self
            .fast_vad_duration
            .map(|duration| duration.as_secs_f64() * 1000.0));

        Some(BenchmarkSummary {
            provider_label: self.provider_label,
            keybind_to_record_start_ms,
            recording_duration_ms,
            stop_to_processing_ms,
            fast_vad_trim_ms,
            encode_ms,
            upload_ms,
            response_ms,
            transcription_ms,
            injection_ms,
            total_ms,
            original_audio_ms,
            original_audio_kb,
            trimmed_audio_ms,
            trimmed_audio_kb,
            sent_audio_ms,
            sent_audio_kb,
            encode_audio_kb: self.encoded_bytes.map(|bytes| bytes as f64 / 1024.0),
            saved_audio_ms,
            saved_audio_kb,
            fast_vad_saved_time_ms,
            saved_audio_pct,
        })
    }
}

fn diff_ms(start: Instant, end: Instant) -> f64 {
    end.saturating_duration_since(start).as_secs_f64() * 1000.0
}

fn audio_ms(samples: Option<usize>, sample_rate: Option<u32>) -> Option<f64> {
    let (samples, rate) = samples.zip(sample_rate)?;
    if rate == 0 {
        return None;
    }
    Some(samples as f64 / rate as f64 * 1000.0)
}

fn raw_audio_kb(samples: Option<usize>) -> Option<f64> {
    samples.map(|count| (count * std::mem::size_of::<f32>()) as f64 / 1024.0)
}

fn ms_cell(value: Option<f64>) -> Cell {
    let content = value
        .map(|v| format!("{v:.1}"))
        .unwrap_or_else(|| DASH.to_string());
    Cell::new(content).set_alignment(CellAlignment::Right)
}

fn kb_cell(value: Option<f64>) -> Cell {
    let content = value
        .map(|v| format!("{v:.1}"))
        .unwrap_or_else(|| DASH.to_string());
    Cell::new(content).set_alignment(CellAlignment::Right)
}

fn savings_cell(value: Option<f64>, pct: Option<f64>) -> Cell {
    let content = match (value, pct) {
        (Some(v), Some(p)) => format!("{v:.1} ({p:.1}%)"),
        (Some(v), None) => format!("{v:.1}"),
        _ => DASH.to_string(),
    };
    Cell::new(content).set_alignment(CellAlignment::Right)
}

pub(crate) struct BenchmarkSummary {
    provider_label: String,
    keybind_to_record_start_ms: f64,
    recording_duration_ms: Option<f64>,
    stop_to_processing_ms: Option<f64>,
    fast_vad_trim_ms: Option<f64>,
    encode_ms: Option<f64>,
    upload_ms: Option<f64>,
    response_ms: Option<f64>,
    transcription_ms: Option<f64>,
    injection_ms: Option<f64>,
    total_ms: f64,
    original_audio_ms: Option<f64>,
    original_audio_kb: Option<f64>,
    trimmed_audio_ms: Option<f64>,
    trimmed_audio_kb: Option<f64>,
    sent_audio_ms: Option<f64>,
    sent_audio_kb: Option<f64>,
    encode_audio_kb: Option<f64>,
    saved_audio_ms: Option<f64>,
    saved_audio_kb: Option<f64>,
    fast_vad_saved_time_ms: Option<f64>,
    saved_audio_pct: Option<f64>,
}

impl fmt::Display for BenchmarkSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut table = Table::new();
        table
            .load_preset(UTF8_FULL)
            .apply_modifier(UTF8_ROUND_CORNERS)
            .set_content_arrangement(ContentArrangement::DynamicFullWidth)
            .force_no_tty();

        table.set_header(vec![
            Cell::new(format!("Benchmark · {}", self.provider_label)),
            Cell::new("Duration (ms)"),
            Cell::new("Audio (ms)"),
            Cell::new("Audio (KB)"),
        ]);

        for column in 1..4 {
            if let Some(col) = table.column_mut(column) {
                col.set_cell_alignment(CellAlignment::Right);
            }
        }

        table.add_row(Row::from(vec![
            Cell::new("Keybind → Record Start"),
            ms_cell(Some(self.keybind_to_record_start_ms)),
            empty_cell(),
            empty_cell(),
        ]));

        table.add_row(Row::from(vec![
            Cell::new("Recording (active)"),
            ms_cell(self.recording_duration_ms),
            ms_cell(self.original_audio_ms),
            kb_cell(self.original_audio_kb),
        ]));

        table.add_row(Row::from(vec![
            Cell::new("Stop → Processing"),
            ms_cell(self.stop_to_processing_ms),
            empty_cell(),
            empty_cell(),
        ]));

        table.add_row(Row::from(vec![
            Cell::new("Fast VAD Trim"),
            ms_cell(self.fast_vad_trim_ms),
            ms_cell(self.trimmed_audio_ms),
            kb_cell(self.trimmed_audio_kb),
        ]));

        table.add_row(Row::from(vec![
            Cell::new("Encode"),
            ms_cell(self.encode_ms),
            empty_cell(),
            kb_cell(self.encode_audio_kb),
        ]));

        table.add_row(Row::from(vec![
            Cell::new("Upload"),
            ms_cell(self.upload_ms),
            empty_cell(),
            empty_cell(),
        ]));

        table.add_row(Row::from(vec![
            Cell::new("Response"),
            ms_cell(self.response_ms),
            empty_cell(),
            empty_cell(),
        ]));

        table.add_row(Row::from(vec![
            Cell::new("Transcription"),
            ms_cell(self.transcription_ms),
            ms_cell(self.sent_audio_ms),
            kb_cell(self.sent_audio_kb),
        ]));

        table.add_row(Row::from(vec![
            Cell::new("Injection"),
            ms_cell(self.injection_ms),
            empty_cell(),
            empty_cell(),
        ]));

        table.add_row(Row::from(vec![
            Cell::new("Total Keybind → Text"),
            ms_cell(Some(self.total_ms)),
            empty_cell(),
            empty_cell(),
        ]));

        table.add_row(Row::from(vec![
            Cell::new("Fast VAD Savings"),
            savings_cell(self.fast_vad_saved_time_ms, self.saved_audio_pct),
            ms_cell(self.saved_audio_ms),
            kb_cell(self.saved_audio_kb),
        ]));

        let rendered = table.trim_fmt();
        f.write_str(&rendered)
    }
}

fn empty_cell() -> Cell {
    Cell::new(DASH).set_alignment(CellAlignment::Right)
}
