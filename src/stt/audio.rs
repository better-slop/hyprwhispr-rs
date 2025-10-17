use anyhow::{anyhow, Context, Result};
use bytes::Bytes;
use flacenc::bitsink::ByteSink;
use flacenc::config::Encoder as EncoderConfig;
use flacenc::encode_with_fixed_block_size;
use flacenc::source::MemSource;
use std::time::Duration;

pub struct EncodedAudio {
    pub bytes: Bytes,
    pub duration: Duration,
    pub sample_rate: u32,
}

/// Encode 32-bit mono PCM samples into FLAC bytes.
///
/// FLAC remains the most compact lossless container that preserves Whisper-level
/// transcription quality. Alternatives like uncompressed WAV increase payload
/// size by ~40%, while perceptual codecs (Opus/AAC) add lossy artefacts that
/// hurt VAD sensitivity on Groq/Gemini. Keeping the pipeline lossless avoids
/// regressing downstream punctuation heuristics.
pub fn encode_pcm_to_flac(pcm: &[f32], sample_rate: u32) -> Result<EncodedAudio> {
    if pcm.is_empty() {
        return Ok(EncodedAudio {
            bytes: Bytes::new(),
            duration: Duration::from_secs(0),
            sample_rate,
        });
    }

    let mut samples = Vec::with_capacity(pcm.len());
    for &sample in pcm {
        let scaled = (sample * 32767.0).round().clamp(-32768.0, 32767.0);
        samples.push(scaled as i32);
    }

    let verified = EncoderConfig::default()
        .into_verified()
        .map_err(|(_, err)| anyhow!("invalid FLAC encoder configuration: {err}"))?;

    let source = MemSource::from_samples(&samples, 1, 16, sample_rate);
    let stream = encode_with_fixed_block_size(&verified, source, verified.block_size)
        .context("failed to encode PCM payload as FLAC")?;

    let mut sink = ByteSink::new();
    stream
        .write(&mut sink)
        .map_err(|err| anyhow!("failed to serialise FLAC stream: {err}"))?;

    let data = sink.into_inner();
    let duration = Duration::from_secs_f32(pcm.len() as f32 / sample_rate as f32);

    Ok(EncodedAudio {
        bytes: Bytes::from(data),
        duration,
        sample_rate,
    })
}
