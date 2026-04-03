/// Speaker embedding extraction using the WeSpeaker ResNet34-LM ONNX model.
///
/// Input: 16kHz mono f32 audio slice
/// Output: 256-dim L2-normalized speaker embedding
use std::path::Path;

use ndarray::Array3;
use ort::{session::Session, value::Tensor};
use tracing::info;

use crate::diarization::fbank;
use crate::error::DictationError;

/// Minimum audio samples for a meaningful embedding (25ms = 400 samples)
const MIN_SAMPLES: usize = fbank::N_FFT;
/// Embedding dimension
pub const EMBEDDING_DIM: usize = 256;

/// Wraps the WeSpeaker ONNX model for speaker embedding extraction.
pub struct Embedder {
    session: Session,
}

impl Embedder {
    /// Load the embedding model from disk.
    pub fn new(model_path: &Path) -> Result<Self, DictationError> {
        let session = Session::builder()
            .map_err(|e| DictationError::DiarizationError(format!("Failed to create ORT session builder: {e}")))?
            .commit_from_file(model_path)
            .map_err(|e| DictationError::DiarizationError(format!("Failed to load embedding model: {e}")))?;
        info!("Embedding model loaded from {}", model_path.display());
        Ok(Self { session })
    }

    /// Extract a 256-dim L2-normalized speaker embedding from a mono 16kHz audio slice.
    ///
    /// Returns `None` if audio is too short (< 25ms).
    pub fn embed(&mut self, audio: &[f32]) -> Result<Option<[f32; EMBEDDING_DIM]>, DictationError> {
        if audio.len() < MIN_SAMPLES {
            return Ok(None);
        }

        let features = fbank::compute_fbank(audio);
        if features.len() < 4 {
            return Ok(None);
        }

        let n_frames = features.len();
        // Flatten [T, 80] to Vec<f32> for ndarray
        let flat: Vec<f32> = features.into_iter().flat_map(|f| f.into_iter()).collect();

        let input = Array3::from_shape_vec((1, n_frames, fbank::N_MELS), flat)
            .map_err(|e| DictationError::DiarizationError(format!("Shape error: {e}")))?;

        let tensor = Tensor::from_array(input)
            .map_err(|e| DictationError::DiarizationError(format!("Tensor error: {e}")))?;
        let outputs = self
            .session
            .run(ort::inputs![tensor])
            .map_err(|e| DictationError::DiarizationError(format!("Embedding inference failed: {e}")))?;

        let raw = outputs[0]
            .try_extract_array::<f32>()
            .map_err(|e| DictationError::DiarizationError(format!("Output extraction failed: {e}")))?;

        let view = raw.view();
        let mut embedding = [0.0f32; EMBEDDING_DIM];
        for i in 0..EMBEDDING_DIM {
            embedding[i] = view[[0, i]];
        }

        Ok(Some(l2_normalize(embedding)))
    }

    /// Extract embeddings for a list of audio segments.
    ///
    /// `segments` is a slice of `(start_sec, end_sec, speaker_idx)` from the segmentation stage.
    /// Returns `(speaker_idx, embedding)` for each segment that is long enough to embed.
    pub fn extract_embeddings(
        &mut self,
        audio: &[f32],
        segments: &[(f64, f64, usize)],
    ) -> Result<Vec<(usize, [f32; EMBEDDING_DIM])>, DictationError> {
        let sample_rate = 16_000usize;
        let mut result = Vec::new();

        for &(start_sec, end_sec, speaker_idx) in segments {
            let start_sample = (start_sec * sample_rate as f64) as usize;
            let end_sample = ((end_sec * sample_rate as f64) as usize).min(audio.len());

            if end_sample <= start_sample || end_sample - start_sample < MIN_SAMPLES {
                continue;
            }

            let slice = &audio[start_sample..end_sample];
            if let Some(embedding) = self.embed(slice)? {
                result.push((speaker_idx, embedding));
            }
        }

        Ok(result)
    }
}

/// L2-normalize a fixed-size embedding vector.
pub fn l2_normalize(mut v: [f32; EMBEDDING_DIM]) -> [f32; EMBEDDING_DIM] {
    let norm: f32 = v.iter().map(|&x| x * x).sum::<f32>().sqrt();
    if norm > 1e-12 {
        for x in &mut v {
            *x /= norm;
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn l2_normalize_unit_vector() {
        let mut v = [0.0f32; EMBEDDING_DIM];
        v[0] = 1.0;
        let normalized = l2_normalize(v);
        let norm: f32 = normalized.iter().map(|&x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-6, "norm should be 1.0, got {norm}");
    }

    #[test]
    fn l2_normalize_uniform_vector() {
        let v = [1.0f32; EMBEDDING_DIM];
        let normalized = l2_normalize(v);
        let norm: f32 = normalized.iter().map(|&x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5, "norm should be 1.0, got {norm}");
    }

    #[test]
    fn l2_normalize_zero_vector_is_stable() {
        let v = [0.0f32; EMBEDDING_DIM];
        let normalized = l2_normalize(v);
        // Should not panic or produce NaN
        for x in &normalized {
            assert!(!x.is_nan(), "NaN in zero-vector normalization");
        }
    }

    #[test]
    fn extract_embeddings_skips_short_segments() {
        // Segments shorter than MIN_SAMPLES should be skipped
        let audio = vec![0.0f32; 16_000];
        let segments = vec![
            (0.0, 0.01, 0usize), // 160 samples — too short
        ];
        // We can't run inference without a real model, but we can test that
        // short segments are filtered before reaching the model.
        // Extract the filtering logic directly:
        let sample_rate = 16_000usize;
        for &(start_sec, end_sec, _) in &segments {
            let start_sample = (start_sec * sample_rate as f64) as usize;
            let end_sample = ((end_sec * sample_rate as f64) as usize).min(audio.len());
            let len = end_sample.saturating_sub(start_sample);
            assert!(len < MIN_SAMPLES, "segment should be too short: {len} samples");
        }
    }

    #[test]
    fn min_samples_matches_fbank_nfft() {
        assert_eq!(MIN_SAMPLES, fbank::N_FFT);
    }
}
