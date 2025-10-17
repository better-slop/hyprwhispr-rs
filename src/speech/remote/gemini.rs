use std::env;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD_NO_PAD;
use base64::Engine as _;
use reqwest::{Client, Url};
use serde::Deserialize;
use serde_json::json;
use tokio::time::sleep;
use tracing::{debug, warn};

use super::encoder::EncodedAudio;
use super::error::SpeechToTextError;

const PROVIDER_NAME: &str = "gemini";
// Gemini 2.5 Pro Flash offers the best latency/quality trade-off for speech
// transcripts and is what the Google team recommends for near-realtime jobs.
const DEFAULT_MODEL: &str = "models/gemini-2.5-pro-flash-exp";
const ENDPOINT_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";
const MAX_RETRIES: usize = 3;
const INITIAL_BACKOFF: Duration = Duration::from_millis(250);

#[derive(Debug, Clone)]
pub struct GeminiTranscriber {
    client: Client,
    api_key: String,
    endpoint: Url,
}

impl GeminiTranscriber {
    pub fn maybe_from_environment(client: Client) -> Result<Option<Self>, SpeechToTextError> {
        let api_key = match env::var("GEMINI_API_KEY") {
            Ok(value) if !value.trim().is_empty() => value,
            _ => return Ok(None),
        };

        let configured_model =
            env::var("GEMINI_SPEECH_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
        let model = if configured_model.starts_with("models/") {
            configured_model
        } else {
            format!("models/{}", configured_model)
        };

        let endpoint = Url::parse(&format!("{}/{}:generateContent", ENDPOINT_BASE, model))
            .map_err(|err| {
                SpeechToTextError::Configuration(format!("invalid Gemini endpoint: {}", err))
            })?;

        Ok(Some(Self {
            client,
            api_key,
            endpoint,
        }))
    }

    pub async fn transcribe(
        &self,
        audio: &EncodedAudio,
        prompt: Option<&str>,
    ) -> Result<String, SpeechToTextError> {
        let mut attempt = 0;
        let mut delay = INITIAL_BACKOFF;
        let instruction = prompt
            .map(str::to_string)
            .filter(|p| !p.trim().is_empty())
            .unwrap_or_else(|| "Transcribe the audio input verbatim.".to_string());

        loop {
            attempt += 1;
            let audio_payload = STANDARD_NO_PAD.encode(audio.data.as_ref());
            let request_body = json!({
                "contents": [{
                    "parts": [
                        { "text": instruction },
                        { "inline_data": { "mime_type": audio.mime_type, "data": audio_payload } }
                    ]
                }],
                "generationConfig": {
                    "temperature": 0.0,
                    "topP": 0.1,
                    "topK": 1,
                }
            });

            let mut url = self.endpoint.clone();
            url.query_pairs_mut().append_pair("key", &self.api_key);

            debug!("gemini transcription attempt {}", attempt);

            let response = self.client.post(url).json(&request_body).send().await;
            match response {
                Ok(resp) => {
                    if resp.status().is_success() {
                        let payload: GeminiResponse = resp.json().await.map_err(|err| {
                            SpeechToTextError::response(PROVIDER_NAME, err.to_string())
                        })?;
                        if let Some(text) = payload.primary_text() {
                            return Ok(text);
                        }
                        return Err(SpeechToTextError::response(
                            PROVIDER_NAME,
                            "Gemini response did not contain transcription text",
                        ));
                    }

                    let status = resp.status();
                    let body = resp
                        .text()
                        .await
                        .unwrap_or_else(|_| "<unavailable>".to_string());
                    warn!("gemini returned {}: {}", status, truncate(&body));

                    if attempt >= MAX_RETRIES || !status.is_server_error() {
                        return Err(SpeechToTextError::status(
                            PROVIDER_NAME,
                            status,
                            truncate(&body),
                        ));
                    }
                }
                Err(err) => {
                    warn!("gemini request failed: {}", err);
                    if attempt >= MAX_RETRIES {
                        return Err(SpeechToTextError::http(PROVIDER_NAME, err));
                    }
                }
            }

            sleep(delay).await;
            delay = (delay * 2).min(Duration::from_secs(2));
        }
    }
}

#[derive(Debug, Deserialize)]
struct GeminiResponse {
    candidates: Option<Vec<GeminiCandidate>>,
}

impl GeminiResponse {
    fn primary_text(self) -> Option<String> {
        self.candidates?
            .into_iter()
            .flat_map(|candidate| candidate.content.parts)
            .find_map(|part| part.text)
    }
}

#[derive(Debug, Deserialize)]
struct GeminiCandidate {
    content: GeminiContent,
}

#[derive(Debug, Deserialize)]
struct GeminiContent {
    parts: Vec<GeminiPart>,
}

#[derive(Debug, Deserialize)]
struct GeminiPart {
    text: Option<String>,
}

fn truncate(input: &str) -> String {
    const MAX_LEN: usize = 512;
    if input.len() <= MAX_LEN {
        input.to_string()
    } else {
        format!("{}…", &input[..MAX_LEN])
    }
}
