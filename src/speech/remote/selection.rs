use std::env;

use super::error::SpeechToTextError;

/// Supported remote speech-to-text vendors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Groq,
    Gemini,
}

impl ProviderKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            ProviderKind::Groq => "groq",
            ProviderKind::Gemini => "gemini",
        }
    }
}

/// Selection strategy that determines which backend should be used for a
/// transcription request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderSelection {
    /// Let the provider pick the first available backend (currently prefers
    /// Groq because of its Whisper-compatible endpoint latency).
    Auto,
    /// Always use a specific backend.
    Single(ProviderKind),
}

impl ProviderSelection {
    const ENV_KEY: &'static str = "HYPRWHSPR_STT_PROVIDER";

    pub fn from_environment() -> Result<Option<Self>, SpeechToTextError> {
        match env::var(Self::ENV_KEY) {
            Ok(value) => {
                let trimmed = value.trim();
                if trimmed.is_empty() {
                    return Ok(None);
                }
                Self::parse(trimmed).map(Some)
            }
            Err(env::VarError::NotPresent) => Ok(None),
            Err(env::VarError::NotUnicode(_)) => Err(SpeechToTextError::Configuration(
                "HYPRWHSPR_STT_PROVIDER contains invalid UTF-8".to_string(),
            )),
        }
    }

    pub fn parse(raw: &str) -> Result<Self, SpeechToTextError> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "auto" => Ok(ProviderSelection::Auto),
            "groq" => Ok(ProviderSelection::Single(ProviderKind::Groq)),
            "gemini" => Ok(ProviderSelection::Single(ProviderKind::Gemini)),
            other => Err(SpeechToTextError::UnsupportedProvider(other.to_string())),
        }
    }

    pub fn choose(&self, available: &[ProviderKind]) -> Result<ProviderKind, SpeechToTextError> {
        match self {
            ProviderSelection::Single(kind) => {
                if available.contains(kind) {
                    Ok(*kind)
                } else {
                    Err(SpeechToTextError::ProviderUnavailable(kind.as_str().into()))
                }
            }
            ProviderSelection::Auto => available
                .first()
                .copied()
                .ok_or(SpeechToTextError::ProviderNotConfigured),
        }
    }
}
