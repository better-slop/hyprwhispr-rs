use std::path::PathBuf;
use std::time::Duration;

use bytes::Bytes;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use tokio::time::timeout;
use tracing::{debug, trace};

use super::error::SpeechToTextError;

// FLAC keeps the audio lossless while typically shrinking our 16 kHz mono PCM
// payloads by ~45%. Opus or Vorbis can be smaller but introduce compression
// artifacts that degrade Whisper-quality models, so we stick with FLAC here.
const FLAC_MIME: &str = "audio/flac";
const DEFAULT_FILENAME: &str = "audio.flac";
const ENCODER_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct EncodedAudio {
    pub data: Bytes,
    pub mime_type: &'static str,
    pub file_name: String,
}

impl EncodedAudio {
    pub fn new(data: Bytes) -> Self {
        Self {
            data,
            mime_type: FLAC_MIME,
            file_name: DEFAULT_FILENAME.to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct FlacEncoder {
    executable: PathBuf,
    sample_rate: u32,
}

impl FlacEncoder {
    pub fn new(sample_rate: u32) -> Result<Self, SpeechToTextError> {
        let executable = std::env::var_os("FFMPEG")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("ffmpeg"));

        if !executable.is_file() {
            // ffmpeg might be available on PATH without being an absolute file yet; defer the
            // existence check to spawn time for relative paths.
            if executable.components().count() > 1 {
                return Err(SpeechToTextError::EncoderMissing(
                    executable.to_string_lossy().to_string(),
                ));
            }
        }

        Ok(Self {
            executable,
            sample_rate,
        })
    }

    pub async fn encode(&self, pcm: &[f32]) -> Result<EncodedAudio, SpeechToTextError> {
        if pcm.is_empty() {
            return Ok(EncodedAudio::new(Bytes::new()));
        }

        let mut child = Command::new(&self.executable)
            .kill_on_drop(true)
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-f")
            .arg("f32le")
            .arg("-ar")
            .arg(self.sample_rate.to_string())
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
            .map_err(|_| {
                SpeechToTextError::EncoderMissing(self.executable.to_string_lossy().to_string())
            })?;

        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| SpeechToTextError::Encoding("failed to open ffmpeg stdin".into()))?;

        // Streaming write without buffering the entire PCM payload in memory twice.
        let write_future = async {
            const CHUNK_SIZE: usize = 4096;
            let mut buffer = Vec::with_capacity(CHUNK_SIZE * std::mem::size_of::<f32>());
            for chunk in pcm.chunks(CHUNK_SIZE) {
                buffer.clear();
                for &sample in chunk {
                    buffer.extend_from_slice(&sample.to_le_bytes());
                }
                stdin.write_all(&buffer).await?;
            }
            stdin.shutdown().await
        };

        timeout(ENCODER_TIMEOUT, write_future).await.map_err(|_| {
            SpeechToTextError::Encoding("timed out while feeding audio to ffmpeg".into())
        })??;

        let output = timeout(ENCODER_TIMEOUT, child.wait_with_output())
            .await
            .map_err(|_| {
                SpeechToTextError::Encoding("timed out while waiting for ffmpeg output".into())
            })??;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(SpeechToTextError::Encoding(format!(
                "ffmpeg exited with status {:?}: {}",
                output.status.code(),
                stderr
            )));
        }

        trace!("ffmpeg stderr: {}", String::from_utf8_lossy(&output.stderr));
        debug!(
            "Encoded {} samples into {} bytes of FLAC",
            pcm.len(),
            output.stdout.len()
        );

        Ok(EncodedAudio::new(Bytes::from(output.stdout)))
    }
}
