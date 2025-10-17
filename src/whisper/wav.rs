use anyhow::Result;
use std::io::Write;

/// Convert PCM f32 samples (mono) to a WAV byte vector.
///
/// Samples are clamped to the i16 range before being written.
pub fn pcm_f32_to_wav_bytes(samples: &[f32], sample_rate_hz: u32) -> Result<Vec<u8>> {
    let samples_i16: Vec<i16> = samples
        .iter()
        .map(|&sample| (sample * 32767.0).clamp(-32768.0, 32767.0) as i16)
        .collect();

    let channels: u16 = 1;
    let bits_per_sample: u16 = 16;
    let byte_rate = sample_rate_hz * channels as u32 * bits_per_sample as u32 / 8;
    let block_align = channels * bits_per_sample / 8;
    let data_size = (samples_i16.len() * 2) as u32;

    let mut buffer = Vec::with_capacity(44 + samples_i16.len() * 2);
    let mut cursor = std::io::Cursor::new(&mut buffer);

    // RIFF header
    cursor.write_all(b"RIFF")?;
    cursor.write_all(&(36 + data_size).to_le_bytes())?;
    cursor.write_all(b"WAVE")?;

    // fmt chunk
    cursor.write_all(b"fmt ")?;
    cursor.write_all(&16u32.to_le_bytes())?; // Chunk size
    cursor.write_all(&1u16.to_le_bytes())?; // Audio format (PCM)
    cursor.write_all(&channels.to_le_bytes())?;
    cursor.write_all(&sample_rate_hz.to_le_bytes())?;
    cursor.write_all(&byte_rate.to_le_bytes())?;
    cursor.write_all(&block_align.to_le_bytes())?;
    cursor.write_all(&bits_per_sample.to_le_bytes())?;

    // data chunk
    cursor.write_all(b"data")?;
    cursor.write_all(&data_size.to_le_bytes())?;

    for sample in samples_i16 {
        cursor.write_all(&sample.to_le_bytes())?;
    }

    Ok(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_valid_wav_header() {
        let samples = vec![0.0f32, 0.5, -0.5];
        let bytes = pcm_f32_to_wav_bytes(&samples, 16_000).expect("wav bytes");

        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WAVE");
        assert_eq!(&bytes[12..16], b"fmt ");
        assert_eq!(&bytes[36..40], b"data");

        // data chunk length should match number of samples * 2 bytes
        let data_len = u32::from_le_bytes(bytes[40..44].try_into().unwrap());
        assert_eq!(data_len as usize, samples.len() * 2);

        // Ensure we wrote expected byte length overall
        assert_eq!(bytes.len(), 44 + samples.len() * 2);
    }
}
