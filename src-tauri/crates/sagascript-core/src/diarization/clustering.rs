/// Agglomerative hierarchical clustering for speaker embeddings.
///
/// Uses average linkage with cosine distance to group embeddings
/// from the segmentation stage into global speaker identities.
/// Average linkage is more robust than complete linkage for speaker diarization
/// because it uses mean inter-cluster distance rather than worst-case.
use kodama::{linkage, Method};

use crate::diarization::embedding::EMBEDDING_DIM;

// The default clustering threshold lives in `DiarizeConfig::default()`
// (diarization/mod.rs) — the single source of truth. A stale `DEFAULT_THRESHOLD`
// constant here (unused, and out of sync at 0.8) was removed with the #75 fix.

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
            // Clamp to [0, 2] — cosine distance range for L2-normalized vectors.
            // Non-finite distances (NaN embeddings) map to max distance so they
            // never merge and cannot panic kodama::linkage.
            let dist = (1.0 - sim).clamp(0.0, 2.0);
            condensed.push(if dist.is_finite() { dist } else { 2.0 });
        }
    }

    if !condensed.is_empty() {
        let mut sorted = condensed.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
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

/// Absorb clusters with less than `min_seconds` of total speech into the nearest
/// cluster that has at least `min_seconds` (cosine similarity of centroids; the
/// similarity normalises, so unnormalised centroid sums are fine).
///
/// Segment-level clustering at a threshold that separates real speakers also
/// produces small spurious clusters from short or noisy utterances (phone audio,
/// laughter, backchannels). A real participant speaks for more than a few
/// seconds in total, so those clusters are merged instead of being reported as
/// extra speakers. `labels[i]` and `durations[i]` belong to `embeddings[i]`.
/// Only clusters that reach `min_seconds` are merge targets. If no cluster does (very
/// short audio) the smallest cluster merges into the nearest remaining one until a single
/// speaker is left; the largest cluster is never absorbed, so at least one speaker remains.
pub fn absorb_small_clusters(
    embeddings: &[(usize, [f32; EMBEDDING_DIM])],
    labels: &mut [usize],
    durations: &[f64],
    min_seconds: f64,
) {
    debug_assert_eq!(embeddings.len(), labels.len());
    debug_assert_eq!(embeddings.len(), durations.len());
    loop {
        let mut total: std::collections::BTreeMap<usize, f64> = std::collections::BTreeMap::new();
        for (&label, &d) in labels.iter().zip(durations) {
            *total.entry(label).or_default() += d.max(0.0);
        }
        if total.len() < 2 {
            return;
        }
        // Smallest cluster below the minimum; ties resolved by lowest label.
        let Some((&small, _)) = total
            .iter()
            .filter(|(_, &t)| t < min_seconds)
            .min_by(|a, b| a.1.partial_cmp(b.1).unwrap_or(std::cmp::Ordering::Equal))
        else {
            return;
        };
        let centroid = |label: usize| {
            let mut c = [0.0f32; EMBEDDING_DIM];
            for ((_, e), _) in embeddings.iter().zip(labels.iter()).filter(|(_, &l)| l == label) {
                for (acc, v) in c.iter_mut().zip(e.iter()) {
                    *acc += *v;
                }
            }
            c
        };
        let small_centroid = centroid(small);
        let any_large = total.values().any(|&t| t >= min_seconds);
        let target = total
            .iter()
            .rev() // max_by keeps the last maximum: reversed, ties resolve to the lowest label
            .filter(|(&l, &t)| l != small && (!any_large || t >= min_seconds))
            .map(|(&l, _)| l)
            .map(|l| (l, cosine_similarity(&small_centroid, &centroid(l))))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(l, _)| l);
        let Some(target) = target else { return };
        for label in labels.iter_mut().filter(|l| **l == small) {
            *label = target;
        }
    }
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

/// Cosine similarity between two vectors. Normalises by both norms itself, so inputs need
/// not be unit length (the absorb step passes summed centroids).
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

    #[test]
    fn cluster_speakers_survives_nan_in_distance_matrix() {
        // A NaN component (e.g. a non-finite ONNX embedding output that bypassed
        // l2_normalize's sanitization) must not panic the distance sort.
        let mut nan_emb = [0.0f32; EMBEDDING_DIM];
        nan_emb[0] = f32::NAN;
        let input = vec![(0, nan_emb), (1, unit_embedding(1)), (2, unit_embedding(2))];
        let result = cluster_speakers(&input, 0.8);
        assert_eq!(result.len(), 3);
    }

    #[test]
    fn small_cluster_is_absorbed_into_nearest_larger_cluster() {
        let a = unit_embedding(0);
        let b = unit_embedding(1);
        let mut near_a = [0.0f32; EMBEDDING_DIM];
        near_a[0] = 0.8;
        near_a[2] = 0.6;
        let input = vec![(0, a), (1, b), (2, near_a)];
        let mut labels = vec![0, 1, 2];
        absorb_small_clusters(&input, &mut labels, &[30.0, 30.0, 2.0], 8.0);
        assert_eq!(labels, vec![0, 1, 0], "2 s cluster joins the closer large cluster");
    }

    #[test]
    fn clusters_at_or_above_minimum_are_kept() {
        let input = vec![(0, unit_embedding(0)), (1, unit_embedding(1))];
        let mut labels = vec![0, 1];
        absorb_small_clusters(&input, &mut labels, &[8.0, 20.0], 8.0);
        assert_eq!(labels, vec![0, 1]);
    }

    #[test]
    fn small_cluster_never_merges_into_another_small_cluster() {
        // Cluster 1 (2 s) is closest to cluster 2 (3 s, also small) but must join the large cluster 0.
        let mut near = [0.0f32; EMBEDDING_DIM];
        near[1] = 1.0;
        near[2] = 0.1;
        let input = vec![(0, unit_embedding(0)), (1, near), (2, unit_embedding(1)), (3, unit_embedding(1))];
        let mut labels = vec![0, 1, 2, 3];
        absorb_small_clusters(&input, &mut labels, &[40.0, 2.0, 3.0, 1.0], 8.0);
        let unique: std::collections::HashSet<_> = labels.iter().collect();
        assert_eq!(unique.len(), 1, "all small clusters end up in the large one: {labels:?}");
        assert!(labels.iter().all(|&l| l == 0));
    }

    #[test]
    fn single_speaker_input_is_unchanged() {
        let input = vec![(0, unit_embedding(0)), (1, unit_embedding(0)), (2, unit_embedding(0))];
        let mut labels = vec![0, 0, 0];
        absorb_small_clusters(&input, &mut labels, &[1.0, 1.0, 1.0], 8.0);
        assert_eq!(labels, vec![0, 0, 0]);
        let clustered = cluster_speakers(&input, 0.48);
        assert!(clustered.iter().all(|(_, l)| *l == clustered[0].1));
    }

    #[test]
    fn equal_sized_ties_resolve_deterministically_to_lowest_label() {
        // Two equally small clusters, equidistant to two equally large ones.
        let mut mid = [0.0f32; EMBEDDING_DIM];
        mid[0] = 1.0;
        mid[1] = 1.0;
        let input = vec![(0, unit_embedding(0)), (1, unit_embedding(1)), (2, mid)];
        let run = || {
            let mut labels = vec![0, 1, 2];
            absorb_small_clusters(&input, &mut labels, &[30.0, 30.0, 2.0], 8.0);
            labels
        };
        let first = run();
        assert_eq!(first, vec![0, 1, 0], "tie goes to the lowest label");
        assert_eq!(first, run());
    }

    #[test]
    fn duplicate_embeddings_in_different_clusters_do_not_panic() {
        let e = unit_embedding(3);
        let input = vec![(0, e), (1, e), (2, unit_embedding(5))];
        let mut labels = vec![0, 1, 2];
        absorb_small_clusters(&input, &mut labels, &[1.0, 1.0, 20.0], 8.0);
        assert!(labels.iter().all(|l| *l == 2), "small duplicates join the large cluster: {labels:?}");
        // Equal embeddings always cluster together at any positive threshold.
        let clustered = cluster_speakers(&[(0, e), (1, e), (2, e)], 0.01);
        assert!(clustered.iter().all(|(_, l)| *l == clustered[0].1));
    }

    #[test]
    fn largest_cluster_survives_when_everything_is_small() {
        let input = vec![(0, unit_embedding(0)), (1, unit_embedding(1)), (2, unit_embedding(2))];
        let mut labels = vec![0, 1, 2];
        absorb_small_clusters(&input, &mut labels, &[1.0, 3.0, 2.0], 8.0);
        let unique: std::collections::HashSet<_> = labels.iter().collect();
        assert_eq!(unique.len(), 1, "short audio collapses to a single speaker: {labels:?}");
    }

    #[test]
    fn absorption_sums_durations_per_cluster_not_per_segment() {
        // Three 3 s segments of one speaker = 9 s >= 8 s: a real speaker.
        let input = vec![
            (0, unit_embedding(0)),
            (1, unit_embedding(1)),
            (2, unit_embedding(1)),
            (3, unit_embedding(1)),
        ];
        let mut labels = vec![0, 1, 1, 1];
        absorb_small_clusters(&input, &mut labels, &[30.0, 3.0, 3.0, 3.0], 8.0);
        assert_eq!(labels, vec![0, 1, 1, 1]);
    }
}
