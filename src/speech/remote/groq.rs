use std::env;
use std::time::Duration;

use reqwest::{multipart, Client, Url};
use serde::Deserialize;
use tokio::time::sleep;
use tracing::{debug, warn};

use super::encoder::EncodedAudio;
use super::error::SpeechToTextError;

const PROVIDER_NAME: &str = "groq";
// Groq exposes OpenAI-compatible Whisper endpoints; "whisper-large-v3-turbo"
// hits their low-latency fleet while keeping accuracy comparable to
// whisper-large-v3.
const DEFAULT_MODEL: &str = "whisper-large-v3-turbo";
const ENDPOINT: &str = "https://api.groq.com/openai/v1/audio/transcriptions";
const MAX_RETRIES: usize = 3;
const INITIAL_BACKOFF: Duration = Duration::from_millis(250);

#[derive(Debug, Clone)]
pub struct GroqTranscriber {
    client: Client,
    api_key: String,
    model: String,
    endpoint: Url,
}

impl GroqTranscriber {
    pub fn maybe_from_environment(client: Client) -> Result<Option<Self>, SpeechToTextError> {
        let api_key = match env::var("GROQ_API_KEY") {
            Ok(value) if !value.trim().is_empty() => value,
            _ => return Ok(None),
        };

        let model = env::var("GROQ_SPEECH_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
        let endpoint = Url::parse(ENDPOINT).map_err(|err| {
            SpeechToTextError::Configuration(format!("invalid Groq endpoint: {}", err))
        })?;

        Ok(Some(Self {
            client,
            api_key,
            model,
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

        loop {
            attempt += 1;
            let mut form = multipart::Form::new()
                .text("model", self.model.clone())
                .text("response_format", "json".to_string())
                .part(
                    "file",
                    multipart::Part::bytes(audio.data.clone())
                        .file_name(audio.file_name.clone())
                        .mime_str(audio.mime_type)
                        .map_err(|err| {
                            SpeechToTextError::Configuration(format!(
                                "failed to build Groq request: {}",
                                err
                            ))
                        })?,
                );

            if let Some(prompt) = prompt {
                if !prompt.trim().is_empty() {
                    form = form.text("prompt", prompt.to_string());
                }
            }

            let request = self
                .client
                .post(self.endpoint.clone())
                .bearer_auth(&self.api_key)
                .multipart(form);

            debug!("groq transcription attempt {}", attempt);

            match request.send().await {
                Ok(response) => {
                    if response.status().is_success() {
                        let payload: GroqResponse = response.json().await.map_err(|err| {
                            SpeechToTextError::response(PROVIDER_NAME, err.to_string())
                        })?;
                        return Ok(payload.text.unwrap_or_default());
                    }

                    let status = response.status();
                    let body = response
                        .text()
                        .await
                        .unwrap_or_else(|_| "<unavailable>".to_string());
                    warn!("groq returned {}: {}", status, truncate(&body));

                    if attempt >= MAX_RETRIES || !status.is_server_error() {
                        return Err(SpeechToTextError::status(
                            PROVIDER_NAME,
                            status,
                            truncate(&body),
                        ));
                    }
                }
                Err(err) => {
                    warn!("groq request failed: {}", err);
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
struct GroqResponse {
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
