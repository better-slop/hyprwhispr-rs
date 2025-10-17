use std::io::Cursor;

use anyhow::{Context, Result};

/// Convert PCM f32 samples into a 16-bit mono WAV byte stream.
///
/// This helper normalizes the floating point range to i16 and writes a WAV
/// header using `hound` so both the local whisper backend and remote backends
/// can share the same serialization logic.
pub fn pcm_f32_to_wav_bytes(samples: &[f32], sample_rate_hz: u32) -> Result<Vec<u8>> {
    let mut cursor = Cursor::new(Vec::new());

    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: sample_rate_hz,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };

    {
        let mut writer =
            hound::WavWriter::new(&mut cursor, spec).context("Failed to create WAV writer")?;

        for sample in samples {
            let clamped = (*sample * i16::MAX as f32).clamp(i16::MIN as f32, i16::MAX as f32);
            writer
                .write_sample(clamped as i16)
                .context("Failed to write WAV sample")?;
        }

        writer.finalize().context("Failed to finalize WAV data")?;
    }

    Ok(cursor.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn converts_audio_to_wav_bytes() {
        let samples = vec![0.0, 0.5, -0.5, 1.0, -1.0];
        let wav_bytes = pcm_f32_to_wav_bytes(&samples, 16_000).expect("wav conversion");
        assert!(wav_bytes.len() > 44, "expected wav header + data");

        let mut cursor = Cursor::new(wav_bytes);
        let mut reader = hound::WavReader::new(&mut cursor).expect("reader");
        assert_eq!(reader.spec().sample_rate, 16_000);
        assert_eq!(reader.spec().channels, 1);
        let decoded: Vec<i16> = reader.samples::<i16>().map(|s| s.unwrap()).collect();
        assert_eq!(decoded.len(), samples.len());
    }
}
