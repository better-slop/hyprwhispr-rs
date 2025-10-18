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
use prompt::PromptBlueprint;

pub enum TranscriptionBackend {
    Whisper(WhisperManager),
    Groq(GroqTranscriber),
    Gemini(GeminiTranscriber),
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
            TranscriptionProvider::Local => {
                let prompt = Self::prompt_for(config, TranscriptionProvider::Local);
                let manager = WhisperManager::new(
                    config_manager.get_model_path(),
                    config_manager.get_whisper_binary_path(),
                    config.threads,
                    prompt,
                    config_manager.get_temp_dir(),
                    config.gpu_layers,
                    vad,
                    config.no_speech_threshold,
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
            TranscriptionBackend::Whisper(_) => TranscriptionProvider::Local,
            TranscriptionBackend::Groq(_) => TranscriptionProvider::Groq,
            TranscriptionBackend::Gemini(_) => TranscriptionProvider::Gemini,
        }
    }

    pub fn needs_refresh(current: &Config, new: &Config) -> bool {
        if current.transcription.provider != new.transcription.provider {
            return true;
        }

        match new.transcription.provider {
            TranscriptionProvider::Local => {
                current.model != new.model
                    || current.threads != new.threads
                    || current.gpu_layers != new.gpu_layers
                    || current.vad != new.vad
                    || (current.no_speech_threshold - new.no_speech_threshold).abs() > f32::EPSILON
                    || current.models_dirs != new.models_dirs
                    || Self::prompt_for(current, TranscriptionProvider::Local)
                        != Self::prompt_for(new, TranscriptionProvider::Local)
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

    pub async fn transcribe(&self, audio_data: Vec<f32>) -> Result<String> {
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
            TranscriptionProvider::Local => {
                PromptBlueprint::new(None, &config.whisper_prompt).resolve()
            }
            TranscriptionProvider::Groq => {
                PromptBlueprint::new(None, &config.whisper_prompt).resolve()
            }
            TranscriptionProvider::Gemini => PromptBlueprint::new(
                config.transcription.gemini.prompt.as_deref(),
                &config.whisper_prompt,
            )
            .resolve(),
        }
    }
}
