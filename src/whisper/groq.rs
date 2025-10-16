use std::time::Duration;

use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::multipart;
use serde::Deserialize;
use tracing::{debug, info};

use super::transcriber::Transcriber;
use super::wav::pcm_f32_to_wav_bytes;

const GROQ_ENDPOINT: &str = "https://api.groq.com/openai/v1/audio/transcriptions";
// TODO: Make model configurable via CLI/config.
const GROQ_MODEL: &str = "whisper-large-v3";

pub struct GroqClient {
    client: reqwest::Client,
    api_key: String,
}

impl GroqClient {
    pub fn new() -> Result<Self> {
        let api_key = std::env::var("GROQ_API_KEY")
            .context("Missing GROQ_API_KEY environment variable for Groq backend")?;

        // TODO: Make timeout configurable and add retries with jitter.
        let client = reqwest::Client::builder()
            .use_rustls_tls()
            .timeout(Duration::from_secs(60))
            .build()
            .context("Failed to build Groq HTTP client")?;

        Ok(Self { client, api_key })
    }
}

#[derive(Debug, Deserialize)]
pub struct GroqTranscription {
    pub text: String,
    #[serde(default)]
    pub x_groq: Option<serde_json::Value>,
}

#[async_trait]
impl Transcriber for GroqClient {
    async fn transcribe(&self, audio: Vec<f32>, sample_rate_hz: u32) -> Result<String> {
        if audio.is_empty() {
            return Ok(String::new());
        }

        let wav_bytes = pcm_f32_to_wav_bytes(&audio, sample_rate_hz)?;

        let part = multipart::Part::bytes(wav_bytes)
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .context("Failed to build audio multipart for Groq")?;

        let form = multipart::Form::new()
            .text("model", GROQ_MODEL.to_string())
            .text("response_format", "json")
            // TODO: Allow providing language/prompt hints.
            .part("file", part);

        debug!("Sending audio to Groq API");

        let response = self
            .client
            .post(GROQ_ENDPOINT)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .multipart(form)
            .send()
            .await
            .context("Failed to send request to Groq")?;

        let response = response.error_for_status().context("Groq request failed")?;
        let transcription: GroqTranscription = response
            .json()
            .await
            .context("Failed to parse Groq transcription response")?;

        info!(
            "Groq transcription complete{}",
            transcription
                .x_groq
                .as_ref()
                .and_then(|value| value.get("id"))
                .map(|id| format!(" (request id={})", id))
                .unwrap_or_default()
        );

        Ok(transcription.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_groq_transcription_response() {
        let payload = r#"{"text":"hello world","x_groq":{"id":"abc123"}}"#;
        let parsed: GroqTranscription = serde_json::from_str(payload).expect("parse");
        assert_eq!(parsed.text, "hello world");
        let request_id = parsed.x_groq.and_then(|value| {
            value
                .get("id")
                .and_then(|id| id.as_str())
                .map(str::to_string)
        });
        assert_eq!(request_id, Some("abc123".to_string()));
    }
}
