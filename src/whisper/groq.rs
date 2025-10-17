use anyhow::{Context, Result};
use async_trait::async_trait;
use reqwest::{multipart, Client};
use serde::Deserialize;
use tracing::{debug, info};

use super::transcriber::Transcriber;
use super::wav::pcm_f32_to_wav_bytes;

const GROQ_ENDPOINT: &str = "https://api.groq.com/openai/v1/audio/transcriptions";
const GROQ_MODEL: &str = "whisper-large-v3";

pub struct GroqClient {
    client: Client,
    api_key: String,
}

impl GroqClient {
    pub fn new() -> Result<Self> {
        let api_key =
            std::env::var("GROQ_API_KEY").context("Missing GROQ_API_KEY environment variable")?;

        // TODO: add request timeouts and retry policy tuning.
        let client = Client::builder()
            .build()
            .context("Failed to build HTTP client")?;

        Ok(Self { client, api_key })
    }

    pub fn model_name(&self) -> &'static str {
        GROQ_MODEL
    }
}

#[derive(Debug, Deserialize)]
struct GroqTranscription {
    text: String,
    #[serde(default)]
    x_groq: Option<serde_json::Value>,
}

#[async_trait]
impl Transcriber for GroqClient {
    async fn transcribe(&self, audio: Vec<f32>, sample_rate_hz: u32) -> Result<String> {
        if audio.is_empty() {
            return Ok(String::new());
        }

        let wav_bytes = pcm_f32_to_wav_bytes(&audio, sample_rate_hz)?;
        debug!(
            sample_rate_hz,
            "Submitting audio to Groq ({} bytes)",
            wav_bytes.len()
        );

        let file_part = multipart::Part::bytes(wav_bytes)
            .file_name("audio.wav")
            .mime_str("audio/wav")?;

        // TODO: allow configuring the model id and optional language/prompt hints.
        let form = multipart::Form::new()
            .text("model", GROQ_MODEL)
            .text("response_format", "json")
            .part("file", file_part);

        let response = self
            .client
            .post(GROQ_ENDPOINT)
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send()
            .await
            .context("Failed to send request to Groq")?;

        let response = response
            .error_for_status()
            .context("Groq transcription request failed")?;

        let parsed: GroqTranscription = response
            .json()
            .await
            .context("Failed to parse Groq transcription response")?;

        // TODO: emit telemetry for Groq request identifiers (x_groq.id) when present.
        info!("✅ Groq transcription complete");
        Ok(parsed.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_optional_metadata() {
        let payload = r#"{
            "text": "hello world",
            "x_groq": {"id": "req_123"}
        }"#;

        let parsed: GroqTranscription = serde_json::from_str(payload).expect("valid json");
        assert_eq!(parsed.text, "hello world");
        assert!(parsed.x_groq.is_some());
    }

    #[test]
    fn parses_without_metadata() {
        let payload = r#"{"text": "hi"}"#;
        let parsed: GroqTranscription = serde_json::from_str(payload).expect("valid json");
        assert_eq!(parsed.text, "hi");
        assert!(parsed.x_groq.is_none());
    }
}
