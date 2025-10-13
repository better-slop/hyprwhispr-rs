use anyhow::{Context, Result};
use regex::Regex;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tracing::{debug, info, warn};

const NON_SPEECH_MARKERS: &[&str] = &["BLANK_AUDIO", "INAUDIBLE", "NO_SPEECH", "SILENCE"];

#[derive(Debug, Clone)]
pub struct WhisperVadOptions {
    pub enabled: bool,
    pub model_path: Option<PathBuf>,
    pub threshold: f32,
    pub min_speech_ms: u32,
    pub min_silence_ms: u32,
    pub max_speech_s: f32,
    pub speech_pad_ms: u32,
    pub samples_overlap: f32,
}

impl WhisperVadOptions {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            model_path: None,
            threshold: 0.5,
            min_speech_ms: 250,
            min_silence_ms: 100,
            max_speech_s: f32::INFINITY,
            speech_pad_ms: 30,
            samples_overlap: 0.10,
        }
    }

    fn is_active(&self) -> bool {
        self.enabled && self.model_path.is_some()
    }
}

pub struct WhisperManager {
    model_path: PathBuf,
    binary_path: PathBuf,
    threads: usize,
    whisper_prompt: String,
    temp_dir: PathBuf,
    gpu_layers: i32,
    vad: WhisperVadOptions,
    no_speech_threshold: f32,
}

impl WhisperManager {
    pub fn new(
        model_path: PathBuf,
        binary_path: PathBuf,
        threads: usize,
        whisper_prompt: String,
        temp_dir: PathBuf,
        gpu_layers: i32,
        vad: WhisperVadOptions,
        no_speech_threshold: f32,
    ) -> Result<Self> {
        Ok(Self {
            model_path,
            binary_path,
            threads,
            whisper_prompt,
            temp_dir,
            gpu_layers,
            vad,
            no_speech_threshold,
        })
    }

    pub fn initialize(&self) -> Result<()> {
        if !self.model_path.exists() {
            return Err(anyhow::anyhow!(
                "Whisper model not found at: {:?}",
                self.model_path
            ));
        }

        if !self.binary_path.exists() {
            return Err(anyhow::anyhow!(
                "Whisper binary not found at: {:?}",
                self.binary_path
            ));
        }

        // Detect GPU support
        let gpu_info = Self::detect_gpu();

        info!("✅ Whisper initialized");
        info!("   Model: {:?}", self.model_path);
        info!("   Binary: {:?}", self.binary_path);
        info!("   GPU: {}", gpu_info);
        if self.gpu_layers > 0 {
            info!("   GPU: enabled (AUR version uses GPU by default)");
        } else {
            info!("   GPU: disabled (CPU only)");
        }

        if self.vad.enabled {
            if let Some(path) = &self.vad.model_path {
                info!("   VAD: enabled ({})", path.display());
            } else {
                warn!("   VAD: enabled but model file not found (will run without VAD)");
            }
        } else {
            info!("   VAD: disabled");
        }

        Ok(())
    }

    fn detect_gpu() -> String {
        use std::process::Command;

        // Check NVIDIA
        if Command::new("nvidia-smi").output().is_ok() {
            return "NVIDIA GPU detected".to_string();
        }

        // Check AMD ROCm
        if Command::new("rocm-smi").output().is_ok() {
            return "AMD GPU (ROCm) detected".to_string();
        }

        // Check if /opt/rocm exists
        if std::path::Path::new("/opt/rocm").exists() {
            return "AMD GPU (ROCm) available".to_string();
        }

        "CPU only (no GPU detected)".to_string()
    }

    pub async fn transcribe(&self, audio_data: Vec<f32>) -> Result<String> {
        if audio_data.is_empty() {
            return Ok(String::new());
        }

        let duration_secs = audio_data.len() as f32 / 16000.0;
        info!("🧠 Transcribing {:.2}s of audio...", duration_secs);

        // Save audio to temporary WAV file
        let temp_wav = self
            .temp_dir
            .join(format!("audio_{}.wav", std::process::id()));
        self.save_audio_as_wav(&audio_data, &temp_wav)?;

        debug!("Saved audio to: {:?}", temp_wav);

        // Run whisper.cpp CLI
        let transcription = self.run_whisper_cli(&temp_wav).await?;
        let cleaned_transcription = self.strip_prompt_artifacts(&transcription);

        // Always clean up after successful transcription pass
        let _ = fs::remove_file(&temp_wav);

        if Self::contains_only_non_speech_markers(&cleaned_transcription) {
            debug!(
                "Whisper produced only non-speech markers: {}",
                cleaned_transcription
            );
            return Ok(String::new());
        }

        if cleaned_transcription.trim().is_empty() {
            warn!("Whisper returned empty transcription");
        } else {
            if cleaned_transcription != transcription {
                debug!(
                    "Stripped prompt artifacts from transcription: raw='{}', cleaned='{}'",
                    transcription, cleaned_transcription
                );
            }
            info!("✅ Transcription: {}", cleaned_transcription);
        }

        Ok(cleaned_transcription)
    }

    fn save_audio_as_wav(&self, audio_data: &[f32], path: &PathBuf) -> Result<()> {
        use std::io::Write;

        // Convert f32 samples to i16
        let samples_i16: Vec<i16> = audio_data
            .iter()
            .map(|&sample| (sample * 32767.0).clamp(-32768.0, 32767.0) as i16)
            .collect();

        // WAV file header
        let mut file = fs::File::create(path)?;

        let channels: u16 = 1;
        let sample_rate: u32 = 16000;
        let bits_per_sample: u16 = 16;
        let byte_rate = sample_rate * channels as u32 * bits_per_sample as u32 / 8;
        let block_align = channels * bits_per_sample / 8;
        let data_size = (samples_i16.len() * 2) as u32;

        // RIFF header
        file.write_all(b"RIFF")?;
        file.write_all(&(36 + data_size).to_le_bytes())?;
        file.write_all(b"WAVE")?;

        // fmt chunk
        file.write_all(b"fmt ")?;
        file.write_all(&16u32.to_le_bytes())?; // Chunk size
        file.write_all(&1u16.to_le_bytes())?; // Audio format (PCM)
        file.write_all(&channels.to_le_bytes())?;
        file.write_all(&sample_rate.to_le_bytes())?;
        file.write_all(&byte_rate.to_le_bytes())?;
        file.write_all(&block_align.to_le_bytes())?;
        file.write_all(&bits_per_sample.to_le_bytes())?;

        // data chunk
        file.write_all(b"data")?;
        file.write_all(&data_size.to_le_bytes())?;

        // Write samples
        for sample in samples_i16 {
            file.write_all(&sample.to_le_bytes())?;
        }

        debug!("Saved audio to WAV: {:?}", path);
        Ok(())
    }

    async fn run_whisper_cli(&self, audio_file: &PathBuf) -> Result<String> {
        let mut cmd = Command::new(&self.binary_path);

        // Basic args
        cmd.args(&[
            "-m",
            self.model_path.to_str().unwrap(),
            "-f",
            audio_file.to_str().unwrap(),
            "--output-txt",
            "--language",
            "en",
            "--threads",
            &self.threads.to_string(),
            "--prompt",
            &self.whisper_prompt,
            "--no-timestamps", // Just plain text, no timestamps
        ]);

        cmd.arg("--no-speech-thold");
        cmd.arg(format!("{}", self.no_speech_threshold));

        if self.vad.is_active() {
            if let Some(model_path) = &self.vad.model_path {
                cmd.arg("--vad");
                cmd.arg("--vad-model");
                cmd.arg(model_path);

                cmd.arg("--vad-threshold");
                cmd.arg(format!("{}", self.vad.threshold));

                cmd.arg("--vad-min-speech-duration-ms");
                cmd.arg(format!("{}", self.vad.min_speech_ms));

                cmd.arg("--vad-min-silence-duration-ms");
                cmd.arg(format!("{}", self.vad.min_silence_ms));

                if self.vad.max_speech_s.is_finite() {
                    cmd.arg("--vad-max-speech-duration-s");
                    cmd.arg(format!("{}", self.vad.max_speech_s));
                }

                cmd.arg("--vad-speech-pad-ms");
                cmd.arg(format!("{}", self.vad.speech_pad_ms));

                cmd.arg("--vad-samples-overlap");
                cmd.arg(format!("{}", self.vad.samples_overlap));
            }
        }

        // GPU control: AUR version uses --no-gpu flag (opposite logic)
        // If gpu_layers == 0, disable GPU. Otherwise let it use GPU by default
        if self.gpu_layers == 0 {
            cmd.arg("--no-gpu");
            debug!("GPU disabled (CPU only)");
        } else {
            debug!("GPU enabled (will use GPU if available)");
        }

        debug!("Running whisper: {:?}", cmd);

        let output = cmd.output().context("Failed to execute whisper binary")?;

        // Log whisper output for debugging
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        debug!("Whisper stdout: {}", stdout);
        debug!("Whisper stderr: {}", stderr);

        if !output.status.success() {
            warn!(
                "Whisper command failed with exit code: {:?}",
                output.status.code()
            );
            warn!("Stderr: {}", stderr);
            return Err(anyhow::anyhow!("Whisper failed: {}", stderr));
        }

        // Try to read output txt file
        let txt_file = audio_file.with_extension("txt");
        if txt_file.exists() {
            let transcription = fs::read_to_string(&txt_file)?;
            let _ = fs::remove_file(&txt_file);

            if transcription.trim().is_empty() {
                warn!(
                    "Transcription file was empty. WAV file saved at: {:?}",
                    audio_file
                );
                info!(
                    "You can test manually with: {} -m {} -f {:?} -ngl {}",
                    self.binary_path.display(),
                    self.model_path.display(),
                    audio_file,
                    self.gpu_layers
                );
            }

            Ok(transcription.trim().to_string())
        } else {
            // Fallback to stdout
            warn!("No .txt file created by whisper, using stdout");
            Ok(stdout.trim().to_string())
        }
    }

    fn strip_prompt_artifacts(&self, transcription: &str) -> String {
        let trimmed = transcription.trim();
        if trimmed.is_empty() {
            return String::new();
        }

        if Self::is_prompt_artifact(trimmed, &self.whisper_prompt) {
            return String::new();
        }

        trimmed.to_string()
    }

    fn contains_only_non_speech_markers(transcription: &str) -> bool {
        let mut found_marker = false;

        for raw_token in transcription.split_whitespace() {
            let token = raw_token.trim_matches(|c: char| matches!(c, '.' | ',' | '!' | '?' | '"'));
            if token.is_empty() {
                continue;
            }

            if !token.starts_with('[') || !token.ends_with(']') {
                return false;
            }

            let inner = token[1..token.len() - 1].trim();
            if inner.is_empty() {
                return false;
            }

            let normalized: String = inner.chars().filter(|c| !c.is_ascii_whitespace()).collect();
            let upper = normalized.to_ascii_uppercase();

            if !NON_SPEECH_MARKERS.iter().any(|marker| *marker == upper) {
                return false;
            }

            found_marker = true;
        }

        found_marker
    }

    fn is_prompt_artifact(transcription: &str, prompt: &str) -> bool {
        let trimmed_prompt = prompt.trim();
        if trimmed_prompt.is_empty() {
            return false;
        }

        let mut phrases = vec![trimmed_prompt.to_string()];
        phrases.extend(
            trimmed_prompt
                .split(|c| c == '.' || c == '!' || c == '?')
                .map(str::trim)
                .filter(|segment| !segment.is_empty())
                .map(|segment| segment.to_string()),
        );

        let transcription_core = transcription.trim_matches(|c: char| c.is_ascii_whitespace());

        for phrase in phrases {
            let escaped = regex::escape(&phrase);
            let pattern = format!(r#"(?i)^(?:{}\s*[.!?\s"]*)+$"#, escaped);
            if let Ok(re) = Regex::new(&pattern) {
                if re.is_match(transcription_core) {
                    return true;
                }
            }
        }

        false
    }
}
