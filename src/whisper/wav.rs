use std::io::Cursor;

use anyhow::{Context, Result};

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
            hound::WavWriter::new(&mut cursor, spec).context("failed to create WAV writer")?;
        for sample in samples {
            let scaled = (*sample * i16::MAX as f32).round();
            let clamped = scaled.clamp(i16::MIN as f32, i16::MAX as f32) as i16;
            writer
                .write_sample(clamped)
                .context("failed to write PCM sample to WAV")?;
        }
        writer.finalize().context("failed to finalize WAV data")?;
    }

    Ok(cursor.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_valid_wav() {
        let samples = vec![0.0_f32, 0.5, -0.5, 1.0, -1.0];
        let wav = pcm_f32_to_wav_bytes(&samples, 16_000).expect("wav conversion");
        let mut reader = hound::WavReader::new(Cursor::new(&wav)).expect("reader");
        let spec = reader.spec();
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_rate, 16_000);
        assert_eq!(spec.bits_per_sample, 16);

        let decoded: Vec<i16> = reader.samples::<i16>().map(|s| s.unwrap()).collect();
        assert_eq!(decoded.len(), samples.len());
    }
}
