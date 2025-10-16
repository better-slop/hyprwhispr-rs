use anyhow::Result;
use tracing::debug;

/// Convert PCM f32 samples into a mono 16-bit WAV byte stream.
pub fn pcm_f32_to_wav_bytes(samples: &[f32], sample_rate_hz: u32) -> Result<Vec<u8>> {
    if sample_rate_hz == 0 {
        anyhow::bail!("Sample rate must be greater than zero");
    }

    let mut buffer = Vec::with_capacity(44 + samples.len() * 2);

    let channels: u16 = 1;
    let bits_per_sample: u16 = 16;
    let byte_rate = sample_rate_hz * channels as u32 * bits_per_sample as u32 / 8;
    let block_align = channels * bits_per_sample / 8;

    // Convert f32 samples to i16.
    let mut samples_i16: Vec<i16> = Vec::with_capacity(samples.len());
    for &sample in samples {
        let clamped = (sample * 32767.0).clamp(-32768.0, 32767.0);
        samples_i16.push(clamped as i16);
    }

    let data_size = (samples_i16.len() * 2) as u32;

    // RIFF header
    buffer.extend_from_slice(b"RIFF");
    buffer.extend_from_slice(&(36 + data_size).to_le_bytes());
    buffer.extend_from_slice(b"WAVE");

    // fmt chunk
    buffer.extend_from_slice(b"fmt ");
    buffer.extend_from_slice(&16u32.to_le_bytes());
    buffer.extend_from_slice(&1u16.to_le_bytes());
    buffer.extend_from_slice(&channels.to_le_bytes());
    buffer.extend_from_slice(&sample_rate_hz.to_le_bytes());
    buffer.extend_from_slice(&byte_rate.to_le_bytes());
    buffer.extend_from_slice(&block_align.to_le_bytes());
    buffer.extend_from_slice(&bits_per_sample.to_le_bytes());

    // data chunk
    buffer.extend_from_slice(b"data");
    buffer.extend_from_slice(&data_size.to_le_bytes());

    for sample in samples_i16 {
        buffer.extend_from_slice(&sample.to_le_bytes());
    }

    debug!(
        "Generated WAV bytes: sample_rate={}Hz samples={} size={} bytes",
        sample_rate_hz,
        samples.len(),
        buffer.len()
    );

    Ok(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_samples_to_wav_bytes() {
        let samples = [0.0_f32, 0.5_f32, -0.5_f32, 1.0_f32];
        let bytes = pcm_f32_to_wav_bytes(&samples, 16_000).expect("wav generation");

        // Header checks
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(&bytes[12..16], b"fmt ");
        assert_eq!(bytes.len(), 44 + samples.len() * 2);

        // Check the first data sample (0.0 -> 0)
        let first_sample = i16::from_le_bytes([bytes[44], bytes[45]]);
        assert_eq!(first_sample, 0);

        // Check a positive sample is clamped correctly (0.5 -> ~16383)
        let second_sample = i16::from_le_bytes([bytes[46], bytes[47]]);
        assert!(second_sample > 16000 && second_sample < 17000);
    }

    #[test]
    fn errors_on_zero_sample_rate() {
        let err = pcm_f32_to_wav_bytes(&[0.0], 0).unwrap_err();
        assert!(err.to_string().contains("Sample rate"));
    }
}
