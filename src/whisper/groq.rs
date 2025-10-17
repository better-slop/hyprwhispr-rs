use std::env;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use reqwest::multipart::{Form, Part};
use serde::Deserialize;
use tracing::{debug, info};

use super::transcriber::Transcriber;
use super::wav::pcm_f32_to_wav_bytes;

const GROQ_API_URL: &str = "https://api.groq.com/openai/v1/audio/transcriptions";
const GROQ_MODEL: &str = "whisper-large-v3";

#[derive(Debug, Clone)]
pub struct GroqClient {
    http_client: reqwest::Client,
    api_key: String,
}

impl GroqClient {
    pub fn new() -> Result<Self> {
        let api_key = env::var("GROQ_API_KEY").map_err(|_| {
            anyhow!("Environment variable GROQ_API_KEY must be set to use the Groq backend")
        })?;

        let http_client = reqwest::Client::builder()
            .use_rustls_tls()
            .build()
            .context("failed to build Groq HTTP client")?;

        Ok(Self {
            http_client,
            api_key,
        })
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
        let part = Part::bytes(wav_bytes)
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .context("failed to set multipart content type")?;

        let form = Form::new()
            .part("file", part)
            .text("model", GROQ_MODEL.to_string())
            .text("response_format", "json".to_string());

        debug!("Sending audio to Groq for transcription");
        // TODO: Add configurable request timeout and retry logic.
        let response = self
            .http_client
            .post(GROQ_API_URL)
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send()
            .await
            .context("failed to send request to Groq")?;

        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .context("failed to read Groq response body")?;

        if !status.is_success() {
            let body = String::from_utf8_lossy(&bytes);
            return Err(anyhow!(
                "Groq transcription request failed with status {}: {}",
                status,
                body
            ));
        }

        let transcription: GroqTranscription =
            serde_json::from_slice(&bytes).context("failed to parse Groq JSON response")?;

        if let Some(metadata) = &transcription.x_groq {
            debug!("Received Groq metadata: {}", metadata);
            // TODO: Surface Groq request identifiers in logs/telemetry.
        }

        info!("✅ Groq transcription received");
        Ok(transcription.text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_groq_response() {
        let payload = r#"{
            "text": "hello world",
            "x_groq": {"id": "req_123"}
        }"#;

        let parsed: GroqTranscription = serde_json::from_str(payload).expect("valid JSON");
        assert_eq!(parsed.text, "hello world");
        assert!(parsed.x_groq.is_some());
    }
}
