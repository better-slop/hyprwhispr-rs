use std::env;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use bytes::Bytes;
use flacenc::bitsink::ByteSink;
use flacenc::component::BitRepr;
use flacenc::config::Encoder as FlacEncoderConfig;
use flacenc::encode_with_fixed_block_size;
use flacenc::error::Verify;
use flacenc::source::MemSource;
use tokio::time::sleep;
use tracing::{debug, warn};

use crate::config::{RemoteProviderKind, RemoteTranscriptionConfig};

mod gemini;
mod groq;

use gemini::GeminiProvider;
use groq::GroqProvider;

const MONO_CHANNELS: u8 = 1;
const SAMPLE_RATE_HZ: u32 = 16_000;
const MAX_BACKOFF: Duration = Duration::from_millis(5_000);
const BASE_BACKOFF: Duration = Duration::from_millis(250);

#[derive(Clone, Copy)]
struct AudioEncoding {
    mime_type: &'static str,
    file_extension: &'static str,
}

const FLAC_ENCODING: AudioEncoding = AudioEncoding {
    mime_type: "audio/flac",
    file_extension: "flac",
};

#[derive(Clone)]
pub struct EncodedAudio {
    bytes: Bytes,
    encoding: AudioEncoding,
    sample_rate: u32,
    channels: u8,
}

impl EncodedAudio {
    pub fn bytes(&self) -> Bytes {
        self.bytes.clone()
    }

    pub fn content_type(&self) -> &'static str {
        self.encoding.mime_type
    }

    pub fn file_extension(&self) -> &'static str {
        self.encoding.file_extension
    }

    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> u8 {
        self.channels
    }
}

#[async_trait]
pub trait SpeechToTextProvider: Send + Sync {
    fn name(&self) -> &'static str;
    async fn transcribe(&self, audio: EncodedAudio) -> Result<String>;
}

pub struct RemoteTranscriber {
    provider: Arc<dyn SpeechToTextProvider>,
    max_attempts: u32,
}

impl RemoteTranscriber {
    pub fn from_config(config: &RemoteTranscriptionConfig) -> Result<Option<Self>> {
        let provider_kind = match &config.provider {
            Some(kind) => kind.clone(),
            None => return Ok(None),
        };

        let timeout = Duration::from_millis(config.request_timeout_ms);
        let client = reqwest::Client::builder()
            .user_agent("hyprwhspr-remote-transcriber/0.1")
            .connect_timeout(Duration::from_secs(10))
            .timeout(timeout)
            .tcp_keepalive(Some(Duration::from_secs(30)))
            .build()
            .context("Failed to build HTTP client for remote transcription")?;

        let provider: Arc<dyn SpeechToTextProvider> = match provider_kind {
            RemoteProviderKind::Groq => {
                let api_key = env::var("GROQ_API_KEY").context(
                    "GROQ_API_KEY environment variable is required for Groq transcription",
                )?;
                let model = config
                    .groq_model
                    .clone()
                    .unwrap_or_else(|| "whisper-large-v3".to_string());
                Arc::new(GroqProvider::new(client.clone(), model, api_key)?)
            }
            RemoteProviderKind::Gemini => {
                let api_key = env::var("GEMINI_API_KEY")
                    .or_else(|_| env::var("GOOGLE_API_KEY"))
                    .context(
                        "Set GEMINI_API_KEY or GOOGLE_API_KEY to enable Gemini transcription",
                    )?;
                let model = config
                    .gemini_model
                    .clone()
                    .unwrap_or_else(|| "gemini-2.5-pro-flash".to_string());
                Arc::new(GeminiProvider::new(client.clone(), model, api_key)?)
            }
        };

        let attempts = config.max_retries.max(1);

        Ok(Some(Self {
            provider,
            max_attempts: attempts,
        }))
    }

    pub fn provider_name(&self) -> &'static str {
        self.provider.name()
    }

    pub fn max_attempts(&self) -> u32 {
        self.max_attempts
    }

    pub async fn transcribe(&self, pcm: &[f32]) -> Result<String> {
        if pcm.is_empty() {
            return Ok(String::new());
        }

        let encoded = self.encode_to_flac(pcm)?;
        let mut last_error: Option<anyhow::Error> = None;

        for attempt in 1..=self.max_attempts {
            match self.provider.transcribe(encoded.clone()).await {
                Ok(text) => return Ok(text),
                Err(err) => {
                    warn!(
                        attempt,
                        provider = self.provider_name(),
                        error = %err,
                        "Remote transcription attempt failed"
                    );
                    last_error = Some(err);

                    if attempt < self.max_attempts {
                        let delay = self.retry_delay(attempt);
                        debug!(
                            ?delay,
                            provider = self.provider_name(),
                            "Waiting before retrying remote transcription"
                        );
                        sleep(delay).await;
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| {
            anyhow!(
                "{} did not return a successful transcription after {} attempts",
                self.provider_name(),
                self.max_attempts
            )
        }))
    }

    fn retry_delay(&self, attempt: u32) -> Duration {
        let multiplier = 1u32.saturating_shl(attempt.saturating_sub(1).min(16));
        let scaled = BASE_BACKOFF
            .checked_mul(multiplier)
            .unwrap_or_else(|| MAX_BACKOFF);
        scaled.min(MAX_BACKOFF)
    }

    fn encode_to_flac(&self, pcm: &[f32]) -> Result<EncodedAudio> {
        // FLAC keeps Whisper-quality fidelity while typically halving payload size
        // relative to PCM WAV. Pure Rust encoding avoids shelling out to ffmpeg,
        // keeps memory safe, and streams cleanly into the HTTP body.
        let mut samples = Vec::with_capacity(pcm.len());
        for &sample in pcm {
            let scaled = (sample * i16::MAX as f32).clamp(i16::MIN as f32, i16::MAX as f32);
            samples.push(i32::from(scaled as i16));
        }

        let config = FlacEncoderConfig::default()
            .into_verified()
            .context("Invalid FLAC encoder configuration")?;
        let source = MemSource::from_samples(&samples, MONO_CHANNELS as usize, 16, SAMPLE_RATE_HZ);
        let stream = encode_with_fixed_block_size(&config, source, config.block_size)
            .context("Failed to encode audio as FLAC")?;

        let mut sink = ByteSink::new();
        stream.write(&mut sink);
        let bytes = Bytes::from(sink.into_inner());

        Ok(EncodedAudio {
            bytes,
            encoding: FLAC_ENCODING,
            sample_rate: SAMPLE_RATE_HZ,
            channels: MONO_CHANNELS,
        })
    }
}
