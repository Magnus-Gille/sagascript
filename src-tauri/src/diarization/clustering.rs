/// Agglomerative hierarchical clustering for speaker embeddings.
///
/// Uses average linkage with cosine distance to group embeddings
/// from the segmentation stage into global speaker identities.
/// Average linkage is more robust than complete linkage for speaker diarization
/// because it uses mean inter-cluster distance rather than worst-case.
use kodama::{linkage, Method};

use crate::diarization::embedding::EMBEDDING_DIM;

/// Default clustering threshold (cosine distance, 0.0-2.0 range).
/// At 0.8, embeddings with cosine similarity > 0.2 are merged.
pub const DEFAULT_THRESHOLD: f32 = 0.8;

/// Cluster embeddings and return a global speaker label per input.
///
/// `embeddings`: slice of `(local_speaker_idx, embedding)` from the embedding stage.
/// `threshold`: cosine distance threshold for cutting the dendrogram.
///
/// Returns `Vec<(local_speaker_idx, global_speaker_id)>` — one entry per input embedding.
/// Global speaker IDs are assigned 0, 1, 2, ... in order of first appearance.
pub fn cluster_speakers(
    embeddings: &[(usize, [f32; EMBEDDING_DIM])],
    threshold: f32,
) -> Vec<(usize, usize)> {
    let n = embeddings.len();

    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![(embeddings[0].0, 0)];
    }

    // Build condensed cosine distance matrix (upper triangle, row-major)
    let mut condensed: Vec<f32> = Vec::with_capacity(n * (n - 1) / 2);
    for i in 0..n {
        for j in (i + 1)..n {
            let sim = cosine_similarity(&embeddings[i].1, &embeddings[j].1);
            // Clamp to [0, 2] — cosine distance range for L2-normalized vectors
            condensed.push((1.0 - sim).clamp(0.0, 2.0));
        }
    }

    if !condensed.is_empty() {
        let mut sorted = condensed.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let n_dist = sorted.len();
        tracing::debug!(
            "Embedding distances: n={}, p25={:.3}, p50={:.3}, p75={:.3}, p90={:.3}",
            n_dist,
            sorted[n_dist * 25 / 100],
            sorted[n_dist * 50 / 100],
            sorted[n_dist * 75 / 100],
            sorted[(n_dist * 90 / 100).min(n_dist - 1)],
        );
    }

    let dendrogram = linkage(&mut condensed, n, Method::Average);
    let raw_labels = cutree(&dendrogram, n, threshold);

    // Remap raw cluster IDs to contiguous 0-based labels in order of first appearance
    let mut id_map: Vec<Option<usize>> = vec![None; n * 2];
    let mut next_id = 0usize;
    let mut labels = vec![0usize; n];
    for (i, &raw) in raw_labels.iter().enumerate() {
        if id_map.get(raw).and_then(|v| *v).is_none() {
            if raw < id_map.len() {
                id_map[raw] = Some(next_id);
            }
            next_id += 1;
        }
        labels[i] = id_map
            .get(raw)
            .and_then(|v| *v)
            .unwrap_or(next_id - 1);
    }

    embeddings
        .iter()
        .zip(labels.iter())
        .map(|((local_idx, _), &global_id)| (*local_idx, global_id))
        .collect()
}

/// Cut a dendrogram at `threshold`, returning a cluster label per observation.
///
/// Uses Union-Find over observations only (indices 0..n). For each step where
/// dissimilarity ≤ threshold, resolves both cluster1 and cluster2 to their
/// representative observation, then unions them.
///
/// Composite cluster labels (≥ n) are tracked to find their representative.
fn cutree(dendrogram: &kodama::Dendrogram<f32>, n: usize, threshold: f32) -> Vec<usize> {
    // Union-Find over observations 0..n
    let mut parent: Vec<usize> = (0..n).collect();

    fn find(parent: &mut [usize], mut x: usize) -> usize {
        while parent[x] != x {
            parent[x] = parent[parent[x]]; // path compression
            x = parent[x];
        }
        x
    }

    // For composite clusters (index >= n, created at step i = index - n),
    // track one representative observation index.
    let mut representative: Vec<usize> = vec![0; n - 1];

    for (step_idx, step) in dendrogram.steps().iter().enumerate() {
        // Resolve each cluster to a representative observation index
        let rep1 = if step.cluster1 < n {
            step.cluster1
        } else {
            representative[step.cluster1 - n]
        };
        let rep2 = if step.cluster2 < n {
            step.cluster2
        } else {
            representative[step.cluster2 - n]
        };

        // The new composite cluster (n + step_idx) is represented by rep1
        representative[step_idx] = rep1;

        if step.dissimilarity <= threshold {
            let root1 = find(&mut parent, rep1);
            let root2 = find(&mut parent, rep2);
            if root1 != root2 {
                parent[root1] = root2;
            }
        }
    }

    // Assign labels: root ID for each observation
    (0..n).map(|i| find(&mut parent, i)).collect()
}

/// Cosine similarity between two L2-normalized (or unnormalized) vectors.
fn cosine_similarity(a: &[f32; EMBEDDING_DIM], b: &[f32; EMBEDDING_DIM]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(&x, &y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|&x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|&x| x * x).sum::<f32>().sqrt();
    let denom = norm_a * norm_b;
    if denom < 1e-12 {
        0.0
    } else {
        (dot / denom).clamp(-1.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diarization::embedding::l2_normalize;

    fn make_embedding(values: [f32; EMBEDDING_DIM]) -> [f32; EMBEDDING_DIM] {
        l2_normalize(values)
    }

    fn unit_embedding(dim: usize) -> [f32; EMBEDDING_DIM] {
        let mut v = [0.0f32; EMBEDDING_DIM];
        v[dim % EMBEDDING_DIM] = 1.0;
        v
    }

    #[test]
    fn empty_input_returns_empty() {
        let result = cluster_speakers(&[], 0.8);
        assert!(result.is_empty());
    }

    #[test]
    fn single_embedding_returns_speaker_zero() {
        let emb = unit_embedding(0);
        let result = cluster_speakers(&[(0, emb)], 0.8);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], (0, 0));
    }

    #[test]
    fn two_identical_embeddings_merge_into_one_speaker() {
        let emb = unit_embedding(0);
        let input = vec![(0, emb), (1, emb)];
        let result = cluster_speakers(&input, 0.8);
        assert_eq!(result.len(), 2);
        // Both should get the same global ID
        assert_eq!(result[0].1, result[1].1, "identical embeddings should cluster together");
    }

    #[test]
    fn two_orthogonal_embeddings_become_different_speakers() {
        // Orthogonal vectors: cosine distance = 1.0, which is above typical threshold
        let emb0 = unit_embedding(0);
        let emb1 = unit_embedding(1);
        let input = vec![(0, emb0), (1, emb1)];
        let result = cluster_speakers(&input, 0.5); // strict threshold
        assert_eq!(result.len(), 2);
        assert_ne!(result[0].1, result[1].1, "orthogonal embeddings should be different speakers");
    }

    #[test]
    fn threshold_below_distance_keeps_embeddings_separate() {
        // Orthogonal unit vectors have cosine distance 1.0.
        // Threshold 0.5 < 1.0, so they should NOT merge.
        let emb0 = unit_embedding(0);
        let emb1 = unit_embedding(1);
        let emb2 = unit_embedding(2);
        let input = vec![(0, emb0), (1, emb1), (2, emb2)];
        let result = cluster_speakers(&input, 0.5);
        let ids: Vec<usize> = result.iter().map(|r| r.1).collect();
        let unique: std::collections::HashSet<_> = ids.iter().cloned().collect();
        assert_eq!(unique.len(), 3, "threshold=0.5 < distance=1.0 should keep 3 speakers: {:?}", ids);
    }

    #[test]
    fn threshold_above_distance_merges_all() {
        // Orthogonal unit vectors have cosine distance 1.0.
        // Threshold 1.5 > 1.0, so complete linkage will merge all into one cluster.
        let emb0 = unit_embedding(0);
        let emb1 = unit_embedding(1);
        let emb2 = unit_embedding(2);
        let input = vec![(0, emb0), (1, emb1), (2, emb2)];
        let result = cluster_speakers(&input, 1.5);
        let ids: Vec<usize> = result.iter().map(|r| r.1).collect();
        let unique: std::collections::HashSet<_> = ids.iter().cloned().collect();
        assert_eq!(unique.len(), 1, "threshold=1.5 > distance=1.0 should merge all: {:?}", ids);
    }

    #[test]
    fn two_clusters_correctly_separated() {
        // Cluster A: embeddings near dim 0
        let mut a1 = [0.0f32; EMBEDDING_DIM];
        a1[0] = 0.9;
        a1[1] = 0.1;
        let a1 = make_embedding(a1);

        let mut a2 = [0.0f32; EMBEDDING_DIM];
        a2[0] = 0.95;
        a2[1] = 0.05;
        let a2 = make_embedding(a2);

        // Cluster B: embeddings near dim 100
        let mut b1 = [0.0f32; EMBEDDING_DIM];
        b1[100] = 0.9;
        b1[101] = 0.1;
        let b1 = make_embedding(b1);

        let mut b2 = [0.0f32; EMBEDDING_DIM];
        b2[100] = 0.95;
        b2[101] = 0.05;
        let b2 = make_embedding(b2);

        let input = vec![(0, a1), (0, a2), (1, b1), (1, b2)];
        let result = cluster_speakers(&input, 0.8);

        assert_eq!(result.len(), 4);
        // a1 and a2 should have the same global ID
        assert_eq!(result[0].1, result[1].1, "cluster A should merge");
        // b1 and b2 should have the same global ID
        assert_eq!(result[2].1, result[3].1, "cluster B should merge");
        // The two clusters should be different
        assert_ne!(result[0].1, result[2].1, "clusters A and B should differ");
    }

    #[test]
    fn cosine_similarity_identical() {
        let v = unit_embedding(5);
        assert!((cosine_similarity(&v, &v) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn cosine_similarity_orthogonal() {
        let v0 = unit_embedding(0);
        let v1 = unit_embedding(1);
        assert!(cosine_similarity(&v0, &v1).abs() < 1e-5);
    }

    #[test]
    fn cosine_similarity_zero_vector_stable() {
        let zero = [0.0f32; EMBEDDING_DIM];
        let v = unit_embedding(0);
        // Should not panic or produce NaN
        let sim = cosine_similarity(&zero, &v);
        assert!(!sim.is_nan());
    }
}
