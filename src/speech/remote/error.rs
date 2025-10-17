use std::io;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum SpeechToTextError {
    #[error("no remote speech-to-text provider configured")]
    ProviderNotConfigured,
    #[error("unsupported speech-to-text provider '{0}'")]
    UnsupportedProvider(String),
    #[error("speech-to-text provider '{0}' is not available")]
    ProviderUnavailable(String),
    #[error("missing required environment variable: {0}")]
    MissingEnvironment(String),
    #[error("configuration error: {0}")]
    Configuration(String),
    #[error("failed to launch encoder at '{0}'")]
    EncoderMissing(String),
    #[error("audio encoding failed: {0}")]
    Encoding(String),
    #[error("HTTP request to {provider} failed: {source}")]
    Http {
        provider: &'static str,
        #[source]
        source: reqwest::Error,
    },
    #[error("HTTP status {status} from {provider}: {message}")]
    HttpStatus {
        provider: &'static str,
        status: reqwest::StatusCode,
        message: String,
    },
    #[error("unable to parse response from {provider}: {message}")]
    ResponseParse {
        provider: &'static str,
        message: String,
    },
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
}

impl SpeechToTextError {
    pub fn http(provider: &'static str, source: reqwest::Error) -> Self {
        Self::Http { provider, source }
    }

    pub fn status(provider: &'static str, status: reqwest::StatusCode, message: String) -> Self {
        Self::HttpStatus {
            provider,
            status,
            message,
        }
    }

    pub fn response(provider: &'static str, message: impl Into<String>) -> Self {
        Self::ResponseParse {
            provider,
            message: message.into(),
        }
    }
}
