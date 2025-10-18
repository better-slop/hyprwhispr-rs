use crate::config::GroqConfig;
use crate::transcription::audio::{encode_to_flac, EncodedAudio};
use crate::transcription::postprocess::clean_transcription;
use anyhow::{Context, Result};
use reqwest::{multipart, Client, Url};
use serde::Deserialize;
use std::cmp;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{info, warn};

#[derive(Clone)]
pub struct GroqTranscriber {
    client: Client,
    endpoint: Url,
    api_key: String,
    model: String,
    prompt: String,
    request_timeout: Duration,
    max_retries: u32,
}

impl GroqTranscriber {
    pub fn new(
        api_key: String,
        config: &GroqConfig,
        request_timeout: Duration,
        max_retries: u32,
        prompt: String,
    ) -> Result<Self> {
        let endpoint = Url::parse(&config.endpoint)
            .with_context(|| format!("Invalid Groq endpoint: {}", config.endpoint))?;

        let client = Client::builder()
            .user_agent("hyprwhspr-rs (groq)")
            .connect_timeout(Duration::from_secs(10))
            .timeout(request_timeout)
            .pool_idle_timeout(Duration::from_secs(30))
            .build()
            .context("Failed to build Groq HTTP client")?;

        Ok(Self {
            client,
            endpoint,
            api_key,
            model: config.model.clone(),
            prompt,
            request_timeout,
            max_retries,
        })
    }

    pub fn initialize(&self) -> Result<()> {
        if self.api_key.trim().is_empty() {
            anyhow::bail!("GROQ_API_KEY is required to use the Groq transcription backend");
        }

        info!(
            "✅ Groq transcription ready (model: {}, timeout: {:?})",
            self.model, self.request_timeout
        );
        Ok(())
    }

    pub fn provider_name(&self) -> &'static str {
        "Groq Whisper"
    }

    pub async fn transcribe(&self, audio_data: Vec<f32>) -> Result<String> {
        if audio_data.is_empty() {
            return Ok(String::new());
        }

        let duration_secs = audio_data.len() as f32 / 16000.0;
        info!(
            provider = self.provider_name(),
            "🧠 Transcribing {:.2}s of audio via Groq", duration_secs
        );

        let encoded = encode_to_flac(&audio_data).await?;
        let raw = self.send_with_retry(&encoded).await?;
        let cleaned = clean_transcription(&raw, &self.prompt);

        if cleaned.is_empty() {
            warn!("Groq returned empty or non-speech transcription");
        } else {
            info!("✅ Transcription (Groq): {}", cleaned);
        }

        Ok(cleaned)
    }

    async fn send_with_retry(&self, audio: &EncodedAudio) -> Result<String> {
        let attempts = cmp::max(1, self.max_retries.saturating_add(1));

        for attempt in 0..attempts {
            match self.send_once(audio).await {
                Ok(text) => return Ok(text),
                Err(err) => {
                    let is_last_attempt = attempt + 1 == attempts;
                    if is_last_attempt {
                        return Err(err);
                    }

                    warn!(
                        attempt = attempt + 1,
                        max_attempts = attempts,
                        "Groq transcription attempt failed: {}",
                        err
                    );

                    let backoff = Duration::from_millis(500 * (1 << attempt));
                    sleep(backoff).await;
                }
            }
        }

        Err(anyhow::anyhow!("Unknown Groq transcription failure"))
    }

    async fn send_once(&self, audio: &EncodedAudio) -> Result<String> {
        let mut form = multipart::Form::new()
            .text("model", self.model.clone())
            .text("response_format", "json".to_string())
            .text("temperature", "0");

        if !self.prompt.trim().is_empty() {
            form = form.text("prompt", self.prompt.clone());
        }

        let file_part = multipart::Part::stream(audio.data.clone())
            .file_name("audio.flac")
            .mime_str(audio.content_type)
            .context("Failed to set Groq audio content type")?;

        form = form.part("file", file_part);

        let response = self
            .client
            .post(self.endpoint.clone())
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send()
            .await
            .context("Failed to send Groq transcription request")?;

        if response.status().is_success() {
            let payload: GroqTranscriptionResponse = response
                .json()
                .await
                .context("Failed to deserialize Groq transcription response")?;
            return Ok(payload.text.unwrap_or_default());
        }

        let status = response.status();
        let body = response
            .json::<GroqErrorResponse>()
            .await
            .unwrap_or_default();

        let message = body
            .error
            .and_then(|err| err.message)
            .unwrap_or_else(|| format!("Groq transcription failed with status {status}"));

        Err(anyhow::anyhow!(message).context(format!("Groq request failed ({status})")))
    }
}

#[derive(Debug, Deserialize, Default)]
struct GroqTranscriptionResponse {
    text: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct GroqErrorResponse {
    error: Option<GroqErrorDetail>,
}

#[derive(Debug, Deserialize, Default)]
struct GroqErrorDetail {
    message: Option<String>,
}
