use crate::config::GeminiConfig;
use crate::transcription::audio::{encode_to_flac, EncodedAudio};
use crate::transcription::postprocess::clean_transcription;
use anyhow::{Context, Result};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use reqwest::{Client, Url};
use serde::{Deserialize, Serialize};
use std::cmp;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{info, warn};

#[derive(Clone)]
pub struct GeminiTranscriber {
    client: Client,
    endpoint: Url,
    api_key: String,
    prompt: String,
    temperature: f32,
    max_output_tokens: u32,
    model: String,
    request_timeout: Duration,
    max_retries: u32,
}

impl GeminiTranscriber {
    pub fn new(
        api_key: String,
        config: &GeminiConfig,
        request_timeout: Duration,
        max_retries: u32,
        prompt: String,
    ) -> Result<Self> {
        let trimmed_endpoint = config.endpoint.trim_end_matches('/');
        let endpoint = Url::parse(&format!(
            "{}/{}:generateContent",
            trimmed_endpoint, config.model
        ))
        .with_context(|| format!("Invalid Gemini endpoint: {}", config.endpoint))?;

        let client = Client::builder()
            .user_agent("hyprwhspr-rs (gemini)")
            .connect_timeout(Duration::from_secs(10))
            .timeout(request_timeout)
            .pool_idle_timeout(Duration::from_secs(30))
            .build()
            .context("Failed to build Gemini HTTP client")?;

        Ok(Self {
            client,
            endpoint,
            api_key,
            prompt,
            temperature: config.temperature,
            max_output_tokens: config.max_output_tokens,
            model: config.model.clone(),
            request_timeout,
            max_retries,
        })
    }

    pub fn initialize(&self) -> Result<()> {
        if self.api_key.trim().is_empty() {
            anyhow::bail!("GEMINI_API_KEY is required to use the Gemini transcription backend");
        }

        info!(
            "✅ Gemini transcription ready (model: {}, timeout: {:?})",
            self.model,
            self.request_timeout
        );
        Ok(())
    }

    pub fn provider_name(&self) -> &'static str {
        "Gemini 2.5 Pro Flash"
    }

    pub async fn transcribe(&self, audio_data: Vec<f32>) -> Result<String> {
        if audio_data.is_empty() {
            return Ok(String::new());
        }

        let duration_secs = audio_data.len() as f32 / 16000.0;
        info!(
            provider = self.provider_name(),
            "🧠 Transcribing {:.2}s of audio via Gemini",
            duration_secs
        );

        let encoded = encode_to_flac(&audio_data).await?;
        let audio_payload = BASE64.encode(encoded.data.as_ref());
        let raw = self.send_with_retry(&encoded, &audio_payload).await?;
        let cleaned = clean_transcription(&raw, &self.prompt);

        if cleaned.is_empty() {
            warn!("Gemini returned empty or non-speech transcription");
        } else {
            info!("✅ Transcription (Gemini): {}", cleaned);
        }

        Ok(cleaned)
    }

    async fn send_with_retry(&self, audio: &EncodedAudio, payload: &str) -> Result<String> {
        let attempts = cmp::max(1, self.max_retries.saturating_add(1));

        for attempt in 0..attempts {
            match self.send_once(audio, payload).await {
                Ok(text) => return Ok(text),
                Err(err) => {
                    if attempt + 1 == attempts {
                        return Err(err);
                    }

                    warn!(
                        attempt = attempt + 1,
                        max_attempts = attempts,
                        "Gemini transcription attempt failed: {}",
                        err
                    );

                    let backoff = Duration::from_millis(600 * (1 << attempt));
                    sleep(backoff).await;
                }
            }
        }

        Err(anyhow::anyhow!("Unknown Gemini transcription failure"))
    }

    async fn send_once(&self, audio: &EncodedAudio, payload: &str) -> Result<String> {
        let mut url = self.endpoint.clone();
        url.query_pairs_mut()
            .append_pair("key", &self.api_key);

        let instruction = build_instruction(&self.prompt);

        let body = GeminiRequest {
            contents: vec![GeminiContent {
                role: "user",
                parts: vec![
                    GeminiPart::Text {
                        text: &instruction,
                    },
                    GeminiPart::InlineData {
                        inline_data: InlineData {
                            mime_type: audio.content_type,
                            data: payload,
                        },
                    },
                ],
            }],
            generation_config: GenerationConfig {
                temperature: self.temperature,
                max_output_tokens: self.max_output_tokens,
            },
        };

        let response = self
            .client
            .post(url)
            .json(&body)
            .send()
            .await
            .context("Failed to send Gemini transcription request")?;

        if response.status().is_success() {
            let payload: GeminiResponse = response
                .json()
                .await
                .context("Failed to deserialize Gemini transcription response")?;
            let text = extract_text(payload).unwrap_or_default();
            return Ok(text);
        }

        let status = response.status();
        let body = response
            .json::<GeminiErrorResponse>()
            .await
            .unwrap_or_default();
        let message = body
            .error
            .and_then(|err| err.message)
            .unwrap_or_else(|| format!("Gemini transcription failed with status {status}"));

        Err(anyhow::anyhow!(message).context(format!("Gemini request failed ({status})")))
    }
}

fn build_instruction(prompt: &str) -> String {
    let mut instruction = String::from(
        "You are a dedicated speech-to-text engine. Return only the verbatim transcription of the provided audio.\n",
    );

    if !prompt.trim().is_empty() {
        instruction.push_str("\nTranscription style guidance: ");
        instruction.push_str(prompt.trim());
    }

    instruction
}

fn extract_text(response: GeminiResponse) -> Option<String> {
    response
        .candidates
        .into_iter()
        .flatten()
        .find_map(|candidate| {
            candidate.content.and_then(|content| {
                content
                    .parts
                    .into_iter()
                    .find_map(|part| part.text)
            })
        })
}

#[derive(Serialize)]
struct GeminiRequest<'a> {
    contents: Vec<GeminiContent<'a>>,
    #[serde(rename = "generationConfig")]
    generation_config: GenerationConfig,
}

#[derive(Serialize)]
struct GeminiContent<'a> {
    role: &'static str,
    parts: Vec<GeminiPart<'a>>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum GeminiPart<'a> {
    Text { text: &'a str },
    InlineData { inline_data: InlineData<'a> },
}

#[derive(Serialize)]
struct InlineData<'a> {
    #[serde(rename = "mimeType")]
    mime_type: &'a str,
    data: &'a str,
}

#[derive(Serialize)]
struct GenerationConfig {
    temperature: f32,
    #[serde(rename = "maxOutputTokens")]
    max_output_tokens: u32,
}

#[derive(Debug, Deserialize, Default)]
struct GeminiResponse {
    candidates: Option<Vec<GeminiCandidate>>, 
}

#[derive(Debug, Deserialize)]
struct GeminiCandidate {
    content: Option<GeminiCandidateContent>,
}

#[derive(Debug, Deserialize)]
struct GeminiCandidateContent {
    parts: Vec<GeminiCandidatePart>,
}

#[derive(Debug, Deserialize)]
struct GeminiCandidatePart {
    text: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
struct GeminiErrorResponse {
    error: Option<GeminiError>,
}

#[derive(Debug, Deserialize)]
struct GeminiError {
    message: Option<String>,
}
