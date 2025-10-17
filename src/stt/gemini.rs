use crate::stt::audio::EncodedAudio;
use anyhow::{anyhow, Context, Result};
use base64::engine::general_purpose::STANDARD_NO_PAD;
use base64::Engine;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::trace;

#[derive(Clone)]
pub struct GeminiBackend {
    base_url: String,
    model: String,
    api_key: String,
}

impl GeminiBackend {
    pub fn new(base_url: String, model: String, api_key: String) -> Self {
        Self {
            base_url,
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
        let encoded_audio = STANDARD_NO_PAD.encode(audio.bytes.as_ref());

        let instruction = if prompt.trim().is_empty() {
            "Transcribe the audio verbatim. Use punctuation and capitalization.".to_string()
        } else {
            prompt.to_string()
        };

        let request_body = GeminiRequest {
            contents: vec![GeminiContent {
                role: "user",
                parts: vec![
                    GeminiPart {
                        text: Some(&instruction),
                        inline_data: None,
                    },
                    GeminiPart {
                        text: None,
                        inline_data: Some(GeminiInlineData {
                            mime_type: "audio/flac",
                            data: &encoded_audio,
                        }),
                    },
                ],
            }],
            generation_config: GeminiGenerationConfig {
                temperature: 0.0,
                // Gemini respects response MIME hints: JSON keeps downstream parsing simple.
                response_mime_type: "application/json",
            },
        };

        let url = format!(
            "{}/{}:generateContent",
            self.base_url.trim_end_matches('/'),
            self.model
        );

        trace!("Dispatching FLAC payload to Gemini endpoint");

        let response = client
            .post(url)
            .query(&[("key", &self.api_key)])
            .json(&request_body)
            .send()
            .await
            .context("failed to reach Gemini transcription endpoint")?;

        let status = response.status();
        if !status.is_success() {
            let body = response
                .text()
                .await
                .unwrap_or_else(|_| "<unreadable body>".to_string());
            return Err(anyhow!("Gemini returned {status}: {body}"));
        }

        let payload: GeminiResponse = response
            .json()
            .await
            .context("failed to parse Gemini transcription response")?;

        payload
            .candidates
            .into_iter()
            .flat_map(|candidate| candidate.content.parts)
            .find_map(|part| {
                let text = part.text.trim();
                if text.is_empty() {
                    None
                } else {
                    Some(text.to_string())
                }
            })
            .ok_or_else(|| anyhow!("Gemini response missing transcription text"))
    }
}

#[derive(Serialize)]
struct GeminiRequest<'a> {
    contents: Vec<GeminiContent<'a>>,
    #[serde(rename = "generationConfig")]
    generation_config: GeminiGenerationConfig,
}

#[derive(Serialize)]
struct GeminiContent<'a> {
    role: &'static str,
    parts: Vec<GeminiPart<'a>>,
}

#[derive(Serialize)]
struct GeminiPart<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<&'a str>,
    #[serde(rename = "inlineData", skip_serializing_if = "Option::is_none")]
    inline_data: Option<GeminiInlineData<'a>>,
}

#[derive(Serialize)]
struct GeminiInlineData<'a> {
    #[serde(rename = "mimeType")]
    mime_type: &'static str,
    data: &'a str,
}

#[derive(Serialize)]
struct GeminiGenerationConfig {
    temperature: f32,
    #[serde(rename = "responseMimeType")]
    response_mime_type: &'static str,
}

#[derive(Deserialize)]
struct GeminiResponse {
    #[serde(default)]
    candidates: Vec<GeminiCandidate>,
}

#[derive(Deserialize)]
struct GeminiCandidate {
    content: GeminiCandidateContent,
}

#[derive(Deserialize)]
struct GeminiCandidateContent {
    #[serde(default)]
    parts: Vec<GeminiCandidatePart>,
}

#[derive(Deserialize)]
struct GeminiCandidatePart {
    #[serde(default)]
    text: String,
}
