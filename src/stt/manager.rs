use crate::config::{GeminiConfig, SpeechProviderKind, TranscriptionConfig};
use crate::stt::audio::{encode_pcm_to_flac, EncodedAudio};
use crate::stt::gemini::GeminiBackend;
use crate::stt::groq::GroqBackend;
use anyhow::{anyhow, Context, Result};
use reqwest::Client;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{debug, info, warn};

const SAMPLE_RATE: u32 = 16_000;

pub struct SpeechToTextProvider {
    prompt: String,
    http_client: Client,
    order: Vec<SpeechProviderKind>,
    groq: Option<GroqBackend>,
    gemini: Option<GeminiBackend>,
    max_retries: u32,
    retry_backoff: Duration,
}

impl SpeechToTextProvider {
    pub fn new(config: &TranscriptionConfig, prompt: String) -> Result<Self> {
        let timeout = Duration::from_secs(config.request_timeout_secs.max(5));
        let http_client = Client::builder()
            .timeout(timeout)
            .connect_timeout(Duration::from_secs(10))
            .pool_idle_timeout(Some(Duration::from_secs(30)))
            .user_agent("hyprwhspr-rs/remote-stt")
            .build()
            .context("failed to construct HTTP client")?;

        let groq = resolve_groq(&config.groq)?;
        let gemini = resolve_gemini(&config.gemini)?;

        let mut order = Vec::new();
        match config.provider {
            SpeechProviderKind::Groq => {
                if groq.is_some() {
                    order.push(SpeechProviderKind::Groq);
                    if gemini.is_some() {
                        order.push(SpeechProviderKind::Gemini);
                    }
                } else if gemini.is_some() {
                    order.push(SpeechProviderKind::Gemini);
                }
            }
            SpeechProviderKind::Gemini => {
                if gemini.is_some() {
                    order.push(SpeechProviderKind::Gemini);
                    if groq.is_some() {
                        order.push(SpeechProviderKind::Groq);
                    }
                } else if groq.is_some() {
                    order.push(SpeechProviderKind::Groq);
                }
            }
        }

        if order.is_empty() {
            return Err(anyhow!(
                "no speech-to-text provider is fully configured (missing API keys?)"
            ));
        }

        Ok(Self {
            prompt,
            http_client,
            order,
            groq,
            gemini,
            max_retries: config.max_retries,
            retry_backoff: Duration::from_millis(config.retry_backoff_ms.max(100)),
        })
    }

    pub async fn transcribe(&self, audio_data: Vec<f32>) -> Result<String> {
        if audio_data.is_empty() {
            return Ok(String::new());
        }

        let encoded = encode_pcm_to_flac(&audio_data, SAMPLE_RATE)?;
        if encoded.bytes.is_empty() {
            return Ok(String::new());
        }

        let mut last_error: Option<anyhow::Error> = None;
        for provider in &self.order {
            match provider {
                SpeechProviderKind::Groq => {
                    if let Some(backend) = &self.groq {
                        let (result, error) = self
                            .run_attempts("Groq", &encoded, |client, prompt, audio| async move {
                                backend.transcribe(client, audio, prompt).await
                            })
                            .await;
                        if let Some(text) = result {
                            return Ok(text);
                        }
                        if let Some(err) = error {
                            last_error = Some(err);
                        }
                    }
                }
                SpeechProviderKind::Gemini => {
                    if let Some(backend) = &self.gemini {
                        let (result, error) = self
                            .run_attempts("Gemini", &encoded, |client, prompt, audio| async move {
                                backend.transcribe(client, audio, prompt).await
                            })
                            .await;
                        if let Some(text) = result {
                            return Ok(text);
                        }
                        if let Some(err) = error {
                            last_error = Some(err);
                        }
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow!("all speech-to-text providers failed")))
    }
}

impl SpeechToTextProvider {
    async fn run_attempts<F, Fut>(
        &self,
        name: &str,
        audio: &EncodedAudio,
        mut attempt: F,
    ) -> (Option<String>, Option<anyhow::Error>)
    where
        F: FnMut(&Client, &str, &EncodedAudio) -> Fut,
        Fut: std::future::Future<Output = Result<String>>,
    {
        info!(
            "🧠 sending {:.2}s of audio to {}",
            audio.duration.as_secs_f32(),
            name
        );
        let mut last_error = None;
        for idx in 0..=self.max_retries {
            let attempt_no = idx + 1;
            match attempt(&self.http_client, &self.prompt, audio).await {
                Ok(text) => {
                    debug!("{} transcription succeeded on attempt {}", name, attempt_no);
                    return (Some(text), None);
                }
                Err(err) => {
                    warn!(
                        "{} transcription attempt {} failed: {}",
                        name, attempt_no, err
                    );
                    last_error = Some(err);
                    if idx < self.max_retries {
                        sleep(self.retry_backoff).await;
                    }
                }
            }
        }

        (None, last_error)
    }
}

fn resolve_groq(config: &GroqConfig) -> Result<Option<GroqBackend>> {
    let api_key = match config
        .api_key
        .clone()
        .or_else(|| std::env::var("GROQ_API_KEY").ok())
    {
        Some(key) if !key.trim().is_empty() => key,
        _ => return Ok(None),
    };

    Ok(Some(GroqBackend::new(
        config.endpoint.clone(),
        config.model.clone(),
        api_key,
    )))
}

fn resolve_gemini(config: &GeminiConfig) -> Result<Option<GeminiBackend>> {
    let api_key = match config
        .api_key
        .clone()
        .or_else(|| std::env::var("GEMINI_API_KEY").ok())
    {
        Some(key) if !key.trim().is_empty() => key,
        _ => return Ok(None),
    };

    Ok(Some(GeminiBackend::new(
        config.endpoint.clone(),
        config.model.clone(),
        api_key,
    )))
}
