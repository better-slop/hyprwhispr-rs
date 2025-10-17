use crate::stt::audio::EncodedAudio;
use anyhow::{anyhow, Context, Result};
use reqwest::multipart::{Form, Part};
use reqwest::Client;
use serde::Deserialize;
use tracing::trace;

#[derive(Clone)]
pub struct GroqBackend {
    endpoint: String,
    model: String,
    api_key: String,
}

impl GroqBackend {
    pub fn new(endpoint: String, model: String, api_key: String) -> Self {
        Self {
            endpoint,
            model,
            api_key,
        }
    }

    pub async fn transcribe(
        &self,
        client: &Client,
        audio: &EncodedAudio,
        prompt: &str,
    ) -> Result<String> {
        let file_part = Part::bytes(audio.bytes.clone())
            .file_name("audio.flac")
            .mime_str("audio/flac")
            .context("failed to configure Groq audio part")?;

        let mut form = Form::new()
            .part("file", file_part)
            .text("model", self.model.clone())
            // Temperature 0 avoids paraphrasing so we mirror Whisper's deterministic output.
            .text("temperature", "0");

        if !prompt.trim().is_empty() {
            form = form.text("prompt", prompt.to_string());
        }

        // Groq honours OpenAI's no-speech heuristic when explicit language hints
        // are omitted, so we rely on the recorded auto-detection behaviour.
        trace!("Dispatching FLAC payload to Groq endpoint");

        let response = client
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send()
            .await
            .context("failed to reach Groq transcription endpoint")?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<unreadable body>".to_string());
            return Err(anyhow!("Groq returned {status}: {body}"));
        }

        let payload: GroqResponse = response
            .json()
            .await
            .context("failed to parse Groq transcription response")?;

        Ok(payload.text)
    }
}

#[derive(Deserialize)]
struct GroqResponse {
    text: String,
}
