use anyhow::{Context, Result};
use bytes::Bytes;
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufWriter};
use tokio::process::Command;
use tokio::try_join;
use tracing::debug;

pub struct EncodedAudio {
    pub data: Bytes,
    pub content_type: &'static str,
}

/// Encodes raw PCM audio (mono, 16 kHz, f32 samples) into FLAC using ffmpeg.
///
/// FLAC offers lossless compression with ~40-60% smaller payloads compared to WAV
/// for 16 kHz speech, while preserving Whisper-grade accuracy. Alternative lossy
/// codecs (e.g. Opus) offer smaller payloads but cause hallucinations in tests with
/// both Groq Whisper and Gemini 2.5 Pro Flash, so we stick with FLAC here.
pub async fn encode_to_flac(audio: &[f32]) -> Result<EncodedAudio> {
    if audio.is_empty() {
        return Ok(EncodedAudio {
            data: Bytes::new(),
            content_type: "audio/flac",
        });
    }

    let mut child = Command::new("ffmpeg")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-f")
        .arg("f32le")
        .arg("-ar")
        .arg("16000")
        .arg("-ac")
        .arg("1")
        .arg("-i")
        .arg("pipe:0")
        .arg("-compression_level")
        .arg("12")
        .arg("-f")
        .arg("flac")
        .arg("pipe:1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("Failed to spawn ffmpeg for FLAC encoding. Ensure ffmpeg is installed")?;

    let mut stdin = child
        .stdin
        .take()
        .context("Failed to open ffmpeg stdin")?;
    let mut stdout = child
        .stdout
        .take()
        .context("Failed to open ffmpeg stdout")?;
    let mut stderr = child
        .stderr
        .take()
        .context("Failed to open ffmpeg stderr")?;

    let audio_chunks = audio;

    let write_future = async move {
        let mut writer = BufWriter::new(&mut stdin);
        const CHUNK_SIZE: usize = 4096;
        let mut buffer = vec![0u8; CHUNK_SIZE * std::mem::size_of::<f32>()];

        for chunk in audio_chunks.chunks(CHUNK_SIZE) {
            let required = chunk.len() * std::mem::size_of::<f32>();
            if buffer.len() < required {
                buffer.resize(required, 0);
            }

            for (idx, sample) in chunk.iter().enumerate() {
                let bytes = sample.to_le_bytes();
                let offset = idx * 4;
                buffer[offset..offset + 4].copy_from_slice(&bytes);
            }

            writer
                .write_all(&buffer[..required])
                .await
                .context("Failed to stream PCM audio into ffmpeg")?;
        }

        writer
            .flush()
            .await
            .context("Failed to flush PCM audio into ffmpeg")?;
        stdin
            .shutdown()
            .await
            .context("Failed to close ffmpeg stdin")?;
        Ok::<(), anyhow::Error>(())
    };

    let read_future = async move {
        let mut encoded = Vec::new();
        stdout
            .read_to_end(&mut encoded)
            .await
            .context("Failed to read FLAC output from ffmpeg")?;
        Ok::<Bytes, anyhow::Error>(Bytes::from(encoded))
    };

    let stderr_future = async move {
        let mut buf = Vec::new();
        stderr
            .read_to_end(&mut buf)
            .await
            .context("Failed to read ffmpeg stderr")?;
        Ok::<Bytes, anyhow::Error>(Bytes::from(buf))
    };

    let (_, encoded, stderr_bytes) = try_join!(write_future, read_future, stderr_future)?;

    let status = child
        .wait()
        .await
        .context("Failed to wait for ffmpeg")?;

    if !status.success() {
        let stderr_text = String::from_utf8_lossy(&stderr_bytes);
        return Err(anyhow::anyhow!(
            "ffmpeg exited with status {:?}: {}",
            status.code(),
            stderr_text
        ));
    }

    debug!(
        "Encoded PCM into FLAC ({} bytes -> {} bytes)",
        audio.len() * std::mem::size_of::<f32>(),
        encoded.len()
    );

    Ok(EncodedAudio {
        data: encoded,
        content_type: "audio/flac",
    })
}
