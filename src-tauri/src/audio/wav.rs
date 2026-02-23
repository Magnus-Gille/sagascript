/// Encode f32 samples (16kHz mono) to WAV format
pub fn encode_wav(samples: &[f32]) -> Vec<u8> {
    let sample_rate: u32 = 16_000;
    let channels: u16 = 1;
    let bits_per_sample: u16 = 16;

    // Convert f32 to i16
    let int16_samples: Vec<i16> = samples
        .iter()
        .map(|&s| {
            let clamped = s.clamp(-1.0, 1.0);
            (clamped * i16::MAX as f32) as i16
        })
        .collect();

    let data_size = (int16_samples.len() * 2) as u32;
    let file_size = data_size + 36;

    let mut wav = Vec::with_capacity(44 + data_size as usize);

    // RIFF header
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&file_size.to_le_bytes());
    wav.extend_from_slice(b"WAVE");

    // fmt chunk
    wav.extend_from_slice(b"fmt ");
    wav.extend_from_slice(&16u32.to_le_bytes()); // chunk size
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM format
    wav.extend_from_slice(&channels.to_le_bytes());
    wav.extend_from_slice(&sample_rate.to_le_bytes());
    let byte_rate = sample_rate * channels as u32 * bits_per_sample as u32 / 8;
    wav.extend_from_slice(&byte_rate.to_le_bytes());
    let block_align = channels * bits_per_sample / 8;
    wav.extend_from_slice(&block_align.to_le_bytes());
    wav.extend_from_slice(&bits_per_sample.to_le_bytes());

    // data chunk
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_size.to_le_bytes());

    // Audio samples
    for sample in &int16_samples {
        wav.extend_from_slice(&sample.to_le_bytes());
    }

    wav
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wav_header() {
        let samples = vec![0.0f32; 16000]; // 1 second of silence
        let wav = encode_wav(&samples);

        // Check RIFF header
        assert_eq!(&wav[0..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[12..16], b"fmt ");
        assert_eq!(&wav[36..40], b"data");

        // 44 byte header + 32000 bytes of audio (16000 samples * 2 bytes)
        assert_eq!(wav.len(), 44 + 32000);
    }

    #[test]
    fn test_sample_clamping() {
        let samples = vec![-2.0, -1.0, 0.0, 1.0, 2.0];
        let wav = encode_wav(&samples);
        // Should not panic and produce valid output
        assert_eq!(wav.len(), 44 + 10);
    }

    #[test]
    fn test_empty_input() {
        let wav = encode_wav(&[]);
        // Just a header, no audio data
        assert_eq!(wav.len(), 44);
        assert_eq!(&wav[0..4], b"RIFF");
        // data chunk size should be 0
        let data_size = u32::from_le_bytes([wav[40], wav[41], wav[42], wav[43]]);
        assert_eq!(data_size, 0);
    }

    #[test]
    fn test_single_sample() {
        let wav = encode_wav(&[0.5]);
        assert_eq!(wav.len(), 44 + 2); // 1 sample * 2 bytes
    }

    #[test]
    fn test_format_fields() {
        let wav = encode_wav(&[0.0; 100]);

        // fmt chunk size = 16
        let fmt_size = u32::from_le_bytes([wav[16], wav[17], wav[18], wav[19]]);
        assert_eq!(fmt_size, 16);

        // PCM format = 1
        let format = u16::from_le_bytes([wav[20], wav[21]]);
        assert_eq!(format, 1);

        // Channels = 1
        let channels = u16::from_le_bytes([wav[22], wav[23]]);
        assert_eq!(channels, 1);

        // Sample rate = 16000
        let sample_rate = u32::from_le_bytes([wav[24], wav[25], wav[26], wav[27]]);
        assert_eq!(sample_rate, 16000);

        // Byte rate = 16000 * 1 * 16/8 = 32000
        let byte_rate = u32::from_le_bytes([wav[28], wav[29], wav[30], wav[31]]);
        assert_eq!(byte_rate, 32000);

        // Block align = 1 * 16/8 = 2
        let block_align = u16::from_le_bytes([wav[32], wav[33]]);
        assert_eq!(block_align, 2);

        // Bits per sample = 16
        let bits = u16::from_le_bytes([wav[34], wav[35]]);
        assert_eq!(bits, 16);
    }

    #[test]
    fn test_sample_encoding_values() {
        // Encode known values and verify the i16 output
        let wav = encode_wav(&[0.0, 1.0, -1.0]);

        // Samples start at byte 44
        let s0 = i16::from_le_bytes([wav[44], wav[45]]);
        let s1 = i16::from_le_bytes([wav[46], wav[47]]);
        let s2 = i16::from_le_bytes([wav[48], wav[49]]);

        assert_eq!(s0, 0); // 0.0 → 0
        assert_eq!(s1, i16::MAX); // 1.0 → 32767
        assert_eq!(s2, -i16::MAX); // -1.0 → -32767
    }

    #[test]
    fn test_clamped_values_match_extremes() {
        // Values > 1.0 should clamp to i16::MAX
        let wav = encode_wav(&[5.0, -5.0]);
        let s0 = i16::from_le_bytes([wav[44], wav[45]]);
        let s1 = i16::from_le_bytes([wav[46], wav[47]]);
        assert_eq!(s0, i16::MAX);
        assert_eq!(s1, -i16::MAX);
    }

    #[test]
    fn test_riff_file_size_field() {
        let samples = vec![0.0f32; 1000];
        let wav = encode_wav(&samples);
        let file_size = u32::from_le_bytes([wav[4], wav[5], wav[6], wav[7]]);
        // RIFF file size = total - 8 (RIFF + size field)
        assert_eq!(file_size as usize, wav.len() - 8);
    }
}
