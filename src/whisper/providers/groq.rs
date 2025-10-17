use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use reqwest::multipart::{Form, Part};
use reqwest::Client;
use serde::Deserialize;
use tracing::debug;

use super::{EncodedAudio, SpeechToTextProvider};

const ENDPOINT: &str = "https://api.groq.com/openai/v1/audio/transcriptions";
const RESPONSE_FORMAT: &str = "text";
const TEMPERATURE: &str = "0";

#[derive(Clone)]
pub struct GroqProvider {
    client: Client,
    model: String,
    api_key: String,
    endpoint: String,
}

impl GroqProvider {
    pub fn new(client: Client, model: String, api_key: String) -> Result<Self> {
        if model.trim().is_empty() {
            return Err(anyhow!("Groq model name cannot be empty"));
        }
        Ok(Self {
            client,
            model,
            api_key,
            endpoint: ENDPOINT.to_string(),
        })
    }
}

#[async_trait]
impl SpeechToTextProvider for GroqProvider {
    fn name(&self) -> &'static str {
        "groq"
    }

    async fn transcribe(&self, audio: EncodedAudio) -> Result<String> {
        let file_part = Part::bytes(audio.bytes())
            .file_name(format!("audio.{}", audio.file_extension()))
            .mime_str(audio.content_type())?;

        let form = Form::new()
            .text("model", self.model.clone())
            .text("response_format", RESPONSE_FORMAT.to_string())
            .text("temperature", TEMPERATURE.to_string())
            .part("file", file_part);

        debug!(model = %self.model, endpoint = %self.endpoint, "Sending Groq transcription request");

        let response = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send()
            .await
            .context("Groq transcription request failed")?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            let snippet: String = body.chars().take(512).collect();
            return Err(anyhow!(
                "Groq transcription failed with HTTP {}: {}",
                status,
                snippet
            ));
        }

        let payload: GroqResponse = response
            .json()
            .await
            .context("Failed to parse Groq transcription response")?;

        if let Some(text) = payload.text {
            return Ok(text);
        }

        if let Some(error) = payload.error.and_then(|inner| inner.message) {
            return Err(anyhow!("Groq returned an error: {}", error));
        }

        Err(anyhow!("Groq response did not contain transcription text"))
    }
}

#[derive(Debug, Deserialize)]
struct GroqResponse {
    text: Option<String>,
    error: Option<GroqErrorBody>,
}

#[derive(Debug, Deserialize)]
struct GroqErrorBody {
    message: Option<String>,
}
