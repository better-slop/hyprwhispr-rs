use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::debug;

use super::{EncodedAudio, SpeechToTextProvider};

const ENDPOINT: &str = "https://generativelanguage.googleapis.com";

#[derive(Clone)]
pub struct GeminiProvider {
    client: Client,
    model: String,
    api_key: String,
    endpoint: String,
}

impl GeminiProvider {
    pub fn new(client: Client, model: String, api_key: String) -> Result<Self> {
        if model.trim().is_empty() {
            return Err(anyhow!("Gemini model name cannot be empty"));
        }
        Ok(Self {
            client,
            model,
            api_key,
            endpoint: ENDPOINT.to_string(),
        })
    }

    fn request_url(&self) -> String {
        format!(
            "{}/v1beta/models/{}:generateContent?key={}",
            self.endpoint, self.model, self.api_key
        )
    }
}

#[async_trait]
impl SpeechToTextProvider for GeminiProvider {
    fn name(&self) -> &'static str {
        "gemini"
    }

    async fn transcribe(&self, audio: EncodedAudio) -> Result<String> {
        let encoded = BASE64_STANDARD.encode(audio.bytes());
        let request = GeminiRequest {
            contents: vec![GeminiContent {
                role: "user".to_string(),
                parts: vec![
                    GeminiRequestPart {
                        text: Some("Transcribe the provided audio verbatim.".to_string()),
                        inline_data: None,
                    },
                    GeminiRequestPart {
                        text: None,
                        inline_data: Some(GeminiInlineData {
                            mime_type: audio.content_type().to_string(),
                            data: encoded,
                        }),
                    },
                ],
            }],
            generation_config: GeminiGenerationConfig { temperature: 0.0 },
        };

        let url = self.request_url();
        debug!(model = %self.model, endpoint = %url, "Sending Gemini transcription request");

        let response = self
            .client
            .post(&url)
            .json(&request)
            .send()
            .await
            .context("Gemini transcription request failed")?;

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            if let Ok(parsed) = serde_json::from_str::<GeminiErrorResponse>(&body) {
                if let Some(message) = parsed.error.and_then(|err| err.message) {
                    return Err(anyhow!("Gemini returned an error: {}", message));
                }
            }
            let snippet: String = body.chars().take(512).collect();
            return Err(anyhow!(
                "Gemini transcription failed with HTTP {}: {}",
                status,
                snippet
            ));
        }

        let payload: GeminiResponse = response
            .json()
            .await
            .context("Failed to parse Gemini transcription response")?;

        let transcription = payload
            .candidates
            .into_iter()
            .flat_map(|candidate| candidate.content.into_iter())
            .flat_map(|content| content.parts.into_iter())
            .filter_map(|part| part.text)
            .find(|text| !text.trim().is_empty());

        transcription.ok_or_else(|| anyhow!("Gemini response did not contain transcription text"))
    }
}

#[derive(Debug, Serialize)]
struct GeminiRequest {
    contents: Vec<GeminiContent>,
    #[serde(rename = "generationConfig")]
    generation_config: GeminiGenerationConfig,
}

#[derive(Debug, Serialize)]
struct GeminiContent {
    role: String,
    parts: Vec<GeminiRequestPart>,
}

#[derive(Debug, Serialize)]
struct GeminiRequestPart {
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none", rename = "inlineData")]
    inline_data: Option<GeminiInlineData>,
}

#[derive(Debug, Serialize)]
struct GeminiInlineData {
    #[serde(rename = "mimeType")]
    mime_type: String,
    data: String,
}

#[derive(Debug, Serialize)]
struct GeminiGenerationConfig {
    temperature: f32,
}

#[derive(Debug, Deserialize)]
struct GeminiResponse {
    candidates: Vec<GeminiCandidate>,
}

#[derive(Debug, Deserialize)]
struct GeminiCandidate {
    content: Option<GeminiResponseContent>,
}

#[derive(Debug, Deserialize)]
struct GeminiResponseContent {
    parts: Vec<GeminiResponsePart>,
}

#[derive(Debug, Deserialize)]
struct GeminiResponsePart {
    text: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GeminiErrorResponse {
    error: Option<GeminiErrorBody>,
}

#[derive(Debug, Deserialize)]
struct GeminiErrorBody {
    message: Option<String>,
}
