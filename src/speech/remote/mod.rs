mod encoder;
mod error;
mod gemini;
mod groq;
mod selection;

use std::time::Duration;

use reqwest::Client;
use tracing::{debug, info};

pub use encoder::EncodedAudio;
pub use error::SpeechToTextError;
pub use selection::{ProviderKind, ProviderSelection};

use encoder::FlacEncoder;
use gemini::GeminiTranscriber;
use groq::GroqTranscriber;

const SAMPLE_RATE: u32 = 16_000;

#[derive(Debug, Clone)]
pub struct RemoteSpeechProvider {
    encoder: FlacEncoder,
    selection: ProviderSelection,
    groq: Option<GroqTranscriber>,
    gemini: Option<GeminiTranscriber>,
}

impl RemoteSpeechProvider {
    pub fn from_environment() -> Result<Option<Self>, SpeechToTextError> {
        let Some(selection) = ProviderSelection::from_environment()? else {
            return Ok(None);
        };

        let client = build_http_client()?;
        let encoder = FlacEncoder::new(SAMPLE_RATE)?;

        let groq = GroqTranscriber::maybe_from_environment(client.clone())?;
        let gemini = GeminiTranscriber::maybe_from_environment(client.clone())?;

        let available = available_kinds(groq.as_ref(), gemini.as_ref());
        if available.is_empty() {
            return Err(SpeechToTextError::MissingEnvironment(
                "Set GROQ_API_KEY or GEMINI_API_KEY to enable remote transcription".to_string(),
            ));
        }

        let chosen = selection.choose(&available)?;
        info!("Remote speech-to-text provider ready: {}", chosen.as_str());

        if groq.is_none() {
            debug!("Groq backend disabled - missing GROQ_API_KEY");
        }
        if gemini.is_none() {
            debug!("Gemini backend disabled - missing GEMINI_API_KEY");
        }

        Ok(Some(Self {
            encoder,
            selection,
            groq,
            gemini,
        }))
    }

    pub async fn transcribe(
        &self,
        pcm: &[f32],
        prompt: Option<&str>,
    ) -> Result<String, SpeechToTextError> {
        if pcm.is_empty() {
            return Ok(String::new());
        }

        let encoded = self.encoder.encode(pcm).await?;
        let available = available_kinds(self.groq.as_ref(), self.gemini.as_ref());
        let mut order = match self.selection {
            ProviderSelection::Auto => available.clone(),
            ProviderSelection::Single(kind) => vec![kind],
        };

        if order.is_empty() {
            return Err(SpeechToTextError::ProviderNotConfigured);
        }

        let mut last_error = None;
        for provider in order.drain(..) {
            let result = match provider {
                ProviderKind::Groq => {
                    let backend = self
                        .groq
                        .as_ref()
                        .ok_or_else(|| SpeechToTextError::ProviderUnavailable("groq".into()))?;
                    backend.transcribe(&encoded, prompt).await
                }
                ProviderKind::Gemini => {
                    let backend = self
                        .gemini
                        .as_ref()
                        .ok_or_else(|| SpeechToTextError::ProviderUnavailable("gemini".into()))?;
                    backend.transcribe(&encoded, prompt).await
                }
            };

            match result {
                Ok(text) => return Ok(text),
                Err(err) => {
                    last_error = Some(err);
                    if !matches!(self.selection, ProviderSelection::Auto) {
                        break;
                    }
                    debug!("Provider {} failed, trying next option", provider.as_str());
                }
            }
        }

        Err(last_error.unwrap_or(SpeechToTextError::ProviderNotConfigured))
    }
}

fn build_http_client() -> Result<Client, SpeechToTextError> {
    Client::builder()
        .user_agent("hyprwhspr-remote-stt/1.0")
        .http2_prior_knowledge(false)
        .tcp_keepalive(Some(Duration::from_secs(30)))
        .pool_idle_timeout(Duration::from_secs(90).into())
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(45))
        .build()
        .map_err(|err| {
            SpeechToTextError::Configuration(format!("failed to build HTTP client: {}", err))
        })
}

fn available_kinds(
    groq: Option<&GroqTranscriber>,
    gemini: Option<&GeminiTranscriber>,
) -> Vec<ProviderKind> {
    let mut kinds = Vec::with_capacity(2);
    if groq.is_some() {
        kinds.push(ProviderKind::Groq);
    }
    if gemini.is_some() {
        kinds.push(ProviderKind::Gemini);
    }
    kinds
}
