mod audio;
mod gemini;
mod groq;
mod postprocess;
mod prompt;

use crate::config::{Config, ConfigManager, TranscriptionProvider};
use crate::whisper::{WhisperManager, WhisperVadOptions};
use anyhow::{Context, Result};
use std::env;
use std::time::Duration;

pub use audio::{encode_to_flac, EncodedAudio};
pub use gemini::GeminiTranscriber;
pub use groq::GroqTranscriber;
pub use postprocess::{clean_transcription, contains_only_non_speech_markers, is_prompt_artifact};
pub use prompt::{PromptBlueprint, DEFAULT_PROMPT};

pub enum TranscriptionBackend {
    Whisper(WhisperManager),
    Groq(GroqTranscriber),
    Gemini(GeminiTranscriber),
}

#[derive(Debug, Clone, Default)]
pub struct BackendMetrics {
    pub encode_duration: Option<Duration>,
    pub encoded_bytes: Option<usize>,
    pub upload_duration: Option<Duration>,
    pub response_duration: Option<Duration>,
    pub transcription_duration: Duration,
}

#[derive(Debug, Clone)]
pub struct TranscriptionResult {
    pub text: String,
    pub metrics: BackendMetrics,
}

impl TranscriptionBackend {
    pub fn build(
        config_manager: &ConfigManager,
        config: &Config,
        vad: WhisperVadOptions,
    ) -> Result<Self> {
        let timeout = Duration::from_secs(config.transcription.request_timeout_secs.max(5));
        let retries = config.transcription.max_retries;

        match config.transcription.provider {
            TranscriptionProvider::WhisperCpp => {
                let prompt = Self::prompt_for(config, TranscriptionProvider::WhisperCpp);
                let whisper_cfg = &config.transcription.whisper_cpp;
                let whisper_binaries =
                    config_manager.get_whisper_binary_candidates(whisper_cfg.fallback_cli);
                let manager = WhisperManager::new(
                    config_manager.get_model_path(),
                    whisper_binaries,
                    whisper_cfg.threads,
                    prompt,
                    config_manager.get_temp_dir(),
                    whisper_cfg.gpu_layers,
                    vad,
                    whisper_cfg.no_speech_threshold,
                )?;
                Ok(Self::Whisper(manager))
            }
            TranscriptionProvider::Groq => {
                let prompt = Self::prompt_for(config, TranscriptionProvider::Groq);
                let api_key = env::var("GROQ_API_KEY")
                    .context("GROQ_API_KEY environment variable is not set")?;
                let provider = GroqTranscriber::new(
                    api_key,
                    &config.transcription.groq,
                    timeout,
                    retries,
                    prompt,
                )?;
                Ok(Self::Groq(provider))
            }
            TranscriptionProvider::Gemini => {
                let prompt = Self::prompt_for(config, TranscriptionProvider::Gemini);
                let api_key = env::var("GEMINI_API_KEY")
                    .context("GEMINI_API_KEY environment variable is not set")?;
                let provider = GeminiTranscriber::new(
                    api_key,
                    &config.transcription.gemini,
                    timeout,
                    retries,
                    prompt,
                )?;
                Ok(Self::Gemini(provider))
            }
        }
    }

    pub fn initialize(&self) -> Result<()> {
        match self {
            TranscriptionBackend::Whisper(manager) => manager.initialize(),
            TranscriptionBackend::Groq(provider) => provider.initialize(),
            TranscriptionBackend::Gemini(provider) => provider.initialize(),
        }
    }

    pub fn provider(&self) -> TranscriptionProvider {
        match self {
            TranscriptionBackend::Whisper(_) => TranscriptionProvider::WhisperCpp,
            TranscriptionBackend::Groq(_) => TranscriptionProvider::Groq,
            TranscriptionBackend::Gemini(_) => TranscriptionProvider::Gemini,
        }
    }

    pub fn needs_refresh(current: &Config, new: &Config) -> bool {
        if current.transcription.provider != new.transcription.provider {
            return true;
        }

        match new.transcription.provider {
            TranscriptionProvider::WhisperCpp => {
                current.transcription.whisper_cpp != new.transcription.whisper_cpp
            }
            TranscriptionProvider::Groq => {
                current.transcription.request_timeout_secs != new.transcription.request_timeout_secs
                    || current.transcription.max_retries != new.transcription.max_retries
                    || current.transcription.groq != new.transcription.groq
                    || Self::prompt_for(current, TranscriptionProvider::Groq)
                        != Self::prompt_for(new, TranscriptionProvider::Groq)
            }
            TranscriptionProvider::Gemini => {
                current.transcription.request_timeout_secs != new.transcription.request_timeout_secs
                    || current.transcription.max_retries != new.transcription.max_retries
                    || current.transcription.gemini != new.transcription.gemini
                    || Self::prompt_for(current, TranscriptionProvider::Gemini)
                        != Self::prompt_for(new, TranscriptionProvider::Gemini)
            }
        }
    }

    pub async fn transcribe(&self, audio_data: Vec<f32>) -> Result<TranscriptionResult> {
        match self {
            TranscriptionBackend::Whisper(manager) => manager.transcribe(audio_data).await,
            TranscriptionBackend::Groq(provider) => provider.transcribe(audio_data).await,
            TranscriptionBackend::Gemini(provider) => provider.transcribe(audio_data).await,
        }
    }
}

impl TranscriptionBackend {
    fn prompt_for(config: &Config, provider: TranscriptionProvider) -> String {
        match provider {
            TranscriptionProvider::WhisperCpp => {
                PromptBlueprint::from(config.transcription.whisper_cpp.prompt.as_str()).resolve()
            }
            TranscriptionProvider::Groq => {
                PromptBlueprint::from(config.transcription.groq.prompt.as_str()).resolve()
            }
            TranscriptionProvider::Gemini => {
                PromptBlueprint::from(config.transcription.gemini.prompt.as_str()).resolve()
            }
        }
    }
}
