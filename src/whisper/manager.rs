use crate::transcription::{
    clean_transcription, contains_only_non_speech_markers, BackendMetrics, TranscriptionResult,
};
use anyhow::{anyhow, Context, Result};
use std::convert::TryFrom;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;
use tracing::{debug, info, trace, warn};

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
    binary_paths: Vec<PathBuf>,
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
        binary_paths: Vec<PathBuf>,
        threads: usize,
        whisper_prompt: String,
        temp_dir: PathBuf,
        gpu_layers: i32,
        vad: WhisperVadOptions,
        no_speech_threshold: f32,
    ) -> Result<Self> {
        if binary_paths.is_empty() {
            return Err(anyhow!(
                "No whisper binaries found. Install whisper.cpp or provide a valid binary path."
            ));
        }

        Ok(Self {
            model_path,
            binary_paths,
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
            return Err(anyhow!("Whisper model not found at: {:?}", self.model_path));
        }

        let available_binary = self
            .binary_paths
            .iter()
            .find(|path| path.exists())
            .ok_or_else(|| {
                let attempted = self
                    .binary_paths
                    .iter()
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ");
                anyhow!(
                    "None of the configured whisper binaries were found. Tried: {}",
                    if attempted.is_empty() {
                        "<none>".to_string()
                    } else {
                        attempted
                    }
                )
            })?;

        // Detect GPU support
        let gpu_info = Self::detect_gpu();

        info!("✅ Whisper initialized");
        info!("   Model: {:?}", self.model_path);
        info!("   Binary: {:?}", available_binary);
        if self.binary_paths.len() > 1 {
            let fallback_list = self
                .binary_paths
                .iter()
                .filter(|p| p != &available_binary)
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>();
            if !fallback_list.is_empty() {
                debug!("   Additional binaries: {}", fallback_list.join(", "));
            }
        }
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

    pub async fn transcribe(&self, audio_data: Vec<f32>) -> Result<TranscriptionResult> {
        if audio_data.is_empty() {
            return Ok(TranscriptionResult {
                text: String::new(),
                metrics: BackendMetrics::default(),
            });
        }

        let duration_secs = audio_data.len() as f32 / 16000.0;
        info!("🧠 Transcribing {:.2}s of audio...", duration_secs);

        // Save audio to temporary WAV file
        let temp_wav = self
            .temp_dir
            .join(format!("audio_{}.wav", std::process::id()));
        let encode_start = Instant::now();
        self.save_audio_as_wav(&audio_data, &temp_wav)?;
        let encode_duration = encode_start.elapsed();
        let encoded_bytes = fs::metadata(&temp_wav)
            .ok()
            .and_then(|meta| usize::try_from(meta.len()).ok());

        debug!("Saved audio to: {:?}", temp_wav);

        // Run whisper.cpp CLI
        let transcribe_start = Instant::now();
        let transcription = self.run_whisper_cli(&temp_wav).await?;
        let transcription_duration = transcribe_start.elapsed();
        let trimmed = transcription.trim();
        let cleaned_transcription = clean_transcription(trimmed, &self.whisper_prompt);

        // Always clean up after successful transcription pass
        let _ = fs::remove_file(&temp_wav);

        let metrics = BackendMetrics {
            encode_duration: Some(encode_duration),
            encoded_bytes,
            upload_duration: None,
            response_duration: None,
            transcription_duration,
        };

        if cleaned_transcription.is_empty() {
            if trimmed.is_empty() {
                warn!("Whisper returned empty transcription");
            } else if contains_only_non_speech_markers(trimmed) {
                debug!("Whisper produced only non-speech markers: {}", trimmed);
            } else {
                debug!(
                    "Transcription removed by prompt artifact filter: raw='{}'",
                    trimmed
                );
            }
            return Ok(TranscriptionResult {
                text: String::new(),
                metrics,
            });
        }

        if cleaned_transcription != trimmed {
            debug!(
                "Stripped prompt artifacts from transcription: raw='{}', cleaned='{}'",
                transcription, cleaned_transcription
            );
        }
        info!("✅ Transcription: {}", cleaned_transcription);

        Ok(TranscriptionResult {
            text: cleaned_transcription,
            metrics,
        })
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
        let mut last_error: Option<anyhow::Error> = None;
        let mut attempted: Vec<PathBuf> = Vec::new();

        for binary in &self.binary_paths {
            if !binary.exists() {
                debug!(
                    "Skipping whisper binary {:?} because it does not exist",
                    binary
                );
                continue;
            }

            attempted.push(binary.clone());

            match self.invoke_whisper(binary, audio_file) {
                Ok(result) => {
                    if last_error.is_some() {
                        info!("Whisper succeeded using fallback binary: {:?}", binary);
                    }
                    return Ok(result);
                }
                Err(err) => {
                    warn!("Whisper binary {:?} failed: {:#}", binary, err);
                    last_error = Some(err);
                    continue;
                }
            }
        }

        let tried = if attempted.is_empty() {
            "<none>".to_string()
        } else {
            attempted
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        };

        Err(last_error.unwrap_or_else(|| anyhow!("All whisper binaries failed. Tried: {}", tried)))
    }

    fn invoke_whisper(&self, binary: &Path, audio_file: &PathBuf) -> Result<String> {
        let mut cmd = Command::new(binary);

        // Basic args
        cmd.args(&[
            "-m",
            self.model_path
                .to_str()
                .ok_or_else(|| anyhow!("Model path contains invalid UTF-8"))?,
            "-f",
            audio_file
                .to_str()
                .ok_or_else(|| anyhow!("Audio path contains invalid UTF-8"))?,
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

        debug!("Running whisper (binary: {:?}): {:?}", binary, cmd);

        let output = cmd
            .output()
            .with_context(|| format!("Failed to execute whisper binary at {:?}", binary))?;

        // Log whisper output for debugging
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        trace!("Whisper stdout ({}): {}", binary.display(), stdout);
        trace!("Whisper stderr ({}): {}", binary.display(), stderr);

        if !output.status.success() {
            let exit_code = output.status.code().map_or_else(
                || "terminated by signal".to_string(),
                |code| format!("exit code {}", code),
            );
            warn!("Whisper binary {:?} failed with {}", binary, exit_code);
            warn!("Stderr: {}", stderr);
            return Err(anyhow!(
                "Whisper failed ({exit_code}) using {:?}: {}",
                binary,
                stderr.trim()
            ));
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
                    binary.display(),
                    self.model_path.display(),
                    audio_file,
                    self.gpu_layers
                );
            }

            Ok(transcription.trim().to_string())
        } else {
            // Fallback to stdout
            warn!(
                "No .txt file created by whisper using {:?}, falling back to stdout",
                binary
            );
            Ok(stdout.trim().to_string())
        }
    }
}
