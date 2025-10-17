use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use reqwest::multipart;
use serde::Deserialize;
use tracing::{info, warn};

use super::transcriber::Transcriber;
use super::wav::pcm_f32_to_wav_bytes;

const TRANSCRIPTION_URL: &str = "https://api.groq.com/openai/v1/audio/transcriptions";
const MODEL_ID: &str = "whisper-large-v3";

pub struct GroqClient {
    client: reqwest::Client,
    api_key: String,
}

impl GroqClient {
    pub fn new() -> Result<Self> {
        let api_key = std::env::var("GROQ_API_KEY").map_err(|_| {
            anyhow::anyhow!(
                "Missing GROQ_API_KEY environment variable. Set it to your Groq API key."
            )
        })?;

        let client = reqwest::Client::builder()
            // TODO: Add timeouts and retry policy for network resiliency.
            .build()
            .context("Failed to build Groq HTTP client")?;

        Ok(Self { client, api_key })
    }
}

#[derive(Debug, Deserialize)]
struct GroqTranscription {
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

        let effective_sample_rate = if sample_rate_hz == 0 {
            warn!("Sample rate was 0Hz; defaulting to 16000Hz for WAV encoding");
            16_000
        } else {
            sample_rate_hz
        };

        let wav_bytes = pcm_f32_to_wav_bytes(&audio, effective_sample_rate)
            .context("Failed to encode audio as WAV")?;

        let file_part = multipart::Part::bytes(wav_bytes)
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .context("Failed to set WAV content type")?;

        let form = multipart::Form::new()
            .part("file", file_part)
            .text("model", MODEL_ID.to_string())
            .text("response_format", "json".to_string());

        let response = self
            .client
            .post(TRANSCRIPTION_URL)
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send()
            .await
            .context("Failed to send request to Groq")?;

        let response = response
            .error_for_status()
            .context("Groq API returned an error status")?;

        let transcription: GroqTranscription = response
            .json()
            .await
            .context("Failed to parse Groq transcription response")?;

        if transcription.text.trim().is_empty() {
            bail!("Groq API returned an empty transcription");
        }

        // TODO: surface Groq request identifiers from `x_groq` for telemetry.
        info!("✅ Groq transcription complete");
        Ok(transcription.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_transcription_response() {
        let payload = r#"{"text":"hello world"}"#;
        let parsed: GroqTranscription = serde_json::from_str(payload).expect("json");
        assert_eq!(parsed.text, "hello world");
        assert!(parsed.x_groq.is_none());
    }

    #[test]
    fn preserves_x_groq_metadata() {
        let payload = r#"{"text":"hi","x_groq":{"id":"123"}}"#;
        let parsed: GroqTranscription = serde_json::from_str(payload).expect("json");
        assert_eq!(parsed.text, "hi");
        assert!(parsed.x_groq.is_some());
    }
}
