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

    let dendrogram = build_dendrogram(embeddings);
    let raw_labels = cutree(&dendrogram, n, threshold);
    relabel_by_first_appearance(embeddings, &raw_labels)
}

/// Average-linkage dendrogram over the cosine distances of `embeddings` (at least two).
fn build_dendrogram(embeddings: &[(usize, [f32; EMBEDDING_DIM])]) -> kodama::Dendrogram<f32> {
    let n = embeddings.len();
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

    linkage(&mut condensed, n, Method::Average)
}

/// Remap raw cluster IDs to contiguous 0-based labels in order of first appearance.
fn relabel_by_first_appearance(
    embeddings: &[(usize, [f32; EMBEDDING_DIM])],
    raw_labels: &[usize],
) -> Vec<(usize, usize)> {
    let n = embeddings.len();
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

/// Cluster embeddings into exactly `target` speakers (issue #305), or as close as the data allows.
///
/// A "speaker" is a cluster with at least `min_seconds` of speech, the same notion
/// [`absorb_small_clusters`] uses. A plain count cut is not enough: average linkage leaves short
/// outlier segments as singletons until late, so cutting a Riksdag debate into 7 clusters yields
/// one giant cluster and six one-segment outliers. So:
///
/// 1. Walk the dendrogram from the root (k = 1, 2, ...) to the first cut with at least `target`
///    clusters of `min_seconds`. Splitting is not monotone (both halves of a big cluster can be
///    small), hence a walk instead of a search. Ties between equal-dissimilarity merges follow
///    kodama's deterministic step order.
/// 2. Absorb every smaller cluster into its nearest large cluster by centroid, with no distance
///    limit: the hint says those segments belong to one of the speakers.
///
/// Each split raises the number of large clusters by at most one, so the first qualifying cut has
/// exactly `target` of them.
///
/// If even the finest cut has fewer than `target` large clusters (short or sparse audio), fall
/// back to cutting the dendrogram into min(`target`, embeddings) clusters without absorption.
/// `durations[i]` belongs to `embeddings[i]`. The result never has fewer clusters than the target
/// unless there are fewer embeddings.
pub fn cluster_to_count(
    embeddings: &[(usize, [f32; EMBEDDING_DIM])],
    durations: &[f64],
    min_seconds: f64,
    target: usize,
) -> Vec<(usize, usize)> {
    debug_assert_eq!(embeddings.len(), durations.len());
    let n = embeddings.len();
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![(embeddings[0].0, 0)];
    }
    let target = target.clamp(1, n);
    let dendrogram = build_dendrogram(embeddings);

    let large_clusters = |raw: &[usize]| {
        let mut seconds: std::collections::BTreeMap<usize, f64> = std::collections::BTreeMap::new();
        for (&label, &d) in raw.iter().zip(durations) {
            *seconds.entry(label).or_default() += d.max(0.0);
        }
        seconds.values().filter(|&&s| s >= min_seconds).count()
    };
    let Some(raw) = (1..=n)
        .map(|k| cutree_count(&dendrogram, n, k))
        .find(|raw| large_clusters(raw) >= target)
    else {
        let raw = cutree_count(&dendrogram, n, target);
        return relabel_by_first_appearance(embeddings, &raw);
    };

    let mut labels: Vec<usize> = raw;
    absorb_small_clusters_floor(embeddings, &mut labels, durations, min_seconds, f32::INFINITY, target);
    let labels: Vec<(usize, usize)> = embeddings
        .iter()
        .zip(&labels)
        .map(|((local_idx, _), &label)| (*local_idx, label))
        .collect();
    // Contiguous labels in order of first appearance, like `cluster_speakers`.
    let mut order: Vec<usize> = Vec::new();
    labels
        .iter()
        .map(|&(local_idx, label)| {
            let id = order.iter().position(|&l| l == label).unwrap_or_else(|| {
                order.push(label);
                order.len() - 1
            });
            (local_idx, id)
        })
        .collect()
}

/// A usable embedding is finite and non-degenerate. Zero (or non-finite) vectors carry no
/// speaker information; cosine similarity maps them to 0, which would make every one a separate
/// speaker. Callers route such segments through the track fallback instead.
pub fn is_usable_embedding(e: &[f32; EMBEDDING_DIM]) -> bool {
    e.iter().all(|x| x.is_finite()) && e.iter().map(|x| x * x).sum::<f32>().sqrt() > 1e-6
}

/// Absorb clusters with less than `min_seconds` of total speech into a similar cluster.
///
/// Segment-level clustering at a threshold that separates real speakers also produces small
/// spurious clusters from short or noisy utterances (phone audio, laughter, backchannels). A
/// small cluster is merged into the nearest cluster by centroid cosine distance, but only when
/// that distance is at most `max_distance`; otherwise it stays a distinct speaker (a participant
/// who really only speaks for a few seconds is not relabelled as someone else). Targets are the
/// clusters that reach `min_seconds`; if none does (short audio) any other cluster is a target,
/// subject to the same distance limit, so clearly different short speakers are never collapsed.
/// `labels[i]` and `durations[i]` belong to `embeddings[i]`. Smallest cluster first, ties
/// resolved by lowest label (also for equally similar targets). Aggregates are computed once and
/// updated per merge; labels are rewritten once.
pub fn absorb_small_clusters(
    embeddings: &[(usize, [f32; EMBEDDING_DIM])],
    labels: &mut [usize],
    durations: &[f64],
    min_seconds: f64,
    max_distance: f32,
) {
    absorb_small_clusters_floor(embeddings, labels, durations, min_seconds, max_distance, 0);
}

/// [`absorb_small_clusters`] that never takes the cluster count below `min_clusters`: once that
/// many clusters remain it stops, even if smaller-than-minimum clusters are left (a speaker-count
/// hint says they are real speakers). `0` or `1` behaves like the unfloored function for any input
/// with at least two clusters.
pub fn absorb_small_clusters_floor(
    embeddings: &[(usize, [f32; EMBEDDING_DIM])],
    labels: &mut [usize],
    durations: &[f64],
    min_seconds: f64,
    max_distance: f32,
    min_clusters: usize,
) {
    debug_assert_eq!(embeddings.len(), labels.len());
    debug_assert_eq!(embeddings.len(), durations.len());
    struct Agg {
        seconds: f64,
        sum: [f32; EMBEDDING_DIM],
    }
    let mut clusters: std::collections::BTreeMap<usize, Agg> = std::collections::BTreeMap::new();
    for (((_, e), &label), &d) in embeddings.iter().zip(labels.iter()).zip(durations) {
        let agg = clusters.entry(label).or_insert(Agg { seconds: 0.0, sum: [0.0; EMBEDDING_DIM] });
        agg.seconds += d.max(0.0);
        for (acc, v) in agg.sum.iter_mut().zip(e.iter()) {
            *acc += *v;
        }
    }
    let mut merged_into: std::collections::BTreeMap<usize, usize> = std::collections::BTreeMap::new();
    let mut kept_distinct: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
    loop {
        if clusters.len() < 2 || clusters.len() <= min_clusters {
            break;
        }
        // Smallest cluster below the minimum that has not been found to have no acceptable target.
        let Some(small) = clusters
            .iter()
            .filter(|(l, a)| a.seconds < min_seconds && !kept_distinct.contains(*l))
            .min_by(|a, b| a.1.seconds.partial_cmp(&b.1.seconds).unwrap_or(std::cmp::Ordering::Equal))
            .map(|(&l, _)| l)
        else {
            break;
        };
        let any_large = clusters.values().any(|a| a.seconds >= min_seconds);
        let target = clusters
            .iter()
            .rev() // max_by keeps the last maximum: reversed, ties resolve to the lowest label
            .filter(|(&l, a)| l != small && (!any_large || a.seconds >= min_seconds))
            .map(|(&l, a)| (l, cosine_similarity(&clusters[&small].sum, &a.sum)))
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))
            .filter(|(_, sim)| 1.0 - sim <= max_distance)
            .map(|(l, _)| l);
        let Some(target) = target else {
            kept_distinct.insert(small);
            continue;
        };
        // A merge moves the target's centroid: earlier rejections are no longer valid.
        kept_distinct.clear();
        let moved = clusters.remove(&small).expect("small cluster exists");
        let t = clusters.get_mut(&target).expect("target cluster exists");
        t.seconds += moved.seconds;
        for (acc, v) in t.sum.iter_mut().zip(moved.sum.iter()) {
            *acc += *v;
        }
        for v in merged_into.values_mut().filter(|v| **v == small) {
            *v = target;
        }
        merged_into.insert(small, target);
    }
    for label in labels.iter_mut() {
        if let Some(&t) = merged_into.get(label) {
            *label = t;
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
    cut_merges(dendrogram, n, |_, dissimilarity| dissimilarity <= threshold)
}

/// Cut a dendrogram into exactly `k` clusters (1 <= k <= n) by applying its first `n - k` merges.
/// Average linkage is monotone, so those are the `n - k` lowest-dissimilarity merges.
fn cutree_count(dendrogram: &kodama::Dendrogram<f32>, n: usize, k: usize) -> Vec<usize> {
    let apply = n - k.clamp(1, n);
    cut_merges(dendrogram, n, |step_idx, _| step_idx < apply)
}

fn cut_merges(
    dendrogram: &kodama::Dendrogram<f32>,
    n: usize,
    apply: impl Fn(usize, f32) -> bool,
) -> Vec<usize> {
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

        if apply(step_idx, step.dissimilarity) {
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

    fn absorb(input: &[(usize, [f32; EMBEDDING_DIM])], labels: &[usize], dur: &[f64], max: f32) -> Vec<usize> {
        let mut l = labels.to_vec();
        absorb_small_clusters(input, &mut l, dur, 8.0, max);
        l
    }

    fn mix(a: usize, wa: f32, b: usize, wb: f32) -> [f32; EMBEDDING_DIM] {
        let mut v = [0.0f32; EMBEDDING_DIM];
        v[a] = wa;
        v[b] = wb;
        v
    }

    #[test]
    fn small_cluster_is_absorbed_into_nearest_larger_cluster() {
        let input = vec![(0, unit_embedding(0)), (1, unit_embedding(1)), (2, mix(0, 0.8, 2, 0.6))];
        // distance to cluster 0 is 0.2, to cluster 1 is 1.0
        assert_eq!(absorb(&input, &[0, 1, 2], &[30.0, 30.0, 2.0], 0.75), vec![0, 1, 0]);
    }

    #[test]
    fn clearly_different_short_speaker_stays_distinct() {
        // A 7 s speaker orthogonal (distance 1.0) to every large cluster is a real participant.
        let input = vec![(0, unit_embedding(0)), (1, unit_embedding(1)), (2, unit_embedding(2))];
        assert_eq!(absorb(&input, &[0, 1, 2], &[60.0, 60.0, 7.0], 0.75), vec![0, 1, 2]);
        // With the old unconditional behaviour (no distance limit) it would be merged.
        assert_ne!(absorb(&input, &[0, 1, 2], &[60.0, 60.0, 7.0], 2.0), vec![0, 1, 2]);
    }

    #[test]
    fn clusters_at_or_above_minimum_are_kept() {
        let input = vec![(0, unit_embedding(0)), (1, unit_embedding(1))];
        assert_eq!(absorb(&input, &[0, 1], &[8.0, 20.0], 2.0), vec![0, 1]);
    }

    #[test]
    fn all_short_but_different_speakers_are_not_collapsed() {
        let input = vec![(0, unit_embedding(0)), (1, unit_embedding(1)), (2, unit_embedding(2))];
        assert_eq!(absorb(&input, &[0, 1, 2], &[1.0, 3.0, 2.0], 0.75), vec![0, 1, 2]);
    }

    #[test]
    fn all_short_and_similar_speakers_merge_into_the_largest() {
        // Similar short clusters merge smallest-first; the largest one survives with the label.
        let input = vec![(0, mix(0, 1.0, 1, 0.1)), (1, mix(0, 1.0, 2, 0.1)), (2, mix(0, 1.0, 3, 0.1))];
        assert_eq!(absorb(&input, &[0, 1, 2], &[1.0, 3.0, 2.0], 0.75), vec![1, 1, 1]);
    }

    #[test]
    fn small_cluster_never_merges_into_another_small_cluster() {
        // Cluster 1 (2 s) is closest to cluster 2 (3 s, small) but must join large cluster 0,
        // which is the only target; cluster 3 (1 s) likewise.
        let input = vec![(0, mix(0, 1.0, 1, 0.5)), (1, mix(1, 1.0, 2, 0.1)), (2, unit_embedding(1)), (3, unit_embedding(1))];
        assert_eq!(absorb(&input, &[0, 1, 2, 3], &[40.0, 2.0, 3.0, 1.0], 1.0), vec![0, 0, 0, 0]);
    }

    #[test]
    fn single_speaker_input_is_unchanged() {
        let input = vec![(0, unit_embedding(0)), (1, unit_embedding(0)), (2, unit_embedding(0))];
        assert_eq!(absorb(&input, &[0, 0, 0], &[1.0, 1.0, 1.0], 0.75), vec![0, 0, 0]);
        let clustered = cluster_speakers(&input, 0.34);
        assert!(clustered.iter().all(|(_, l)| *l == clustered[0].1));
    }

    #[test]
    fn equal_sized_ties_resolve_deterministically_to_lowest_label() {
        // The small cluster is equidistant to two equally large ones.
        let input = vec![(0, unit_embedding(0)), (1, unit_embedding(1)), (2, mix(0, 1.0, 1, 1.0))];
        let first = absorb(&input, &[0, 1, 2], &[30.0, 30.0, 2.0], 0.75);
        assert_eq!(first, vec![0, 1, 0], "tie goes to the lowest label");
        assert_eq!(first, absorb(&input, &[0, 1, 2], &[30.0, 30.0, 2.0], 0.75));
    }

    #[test]
    fn duplicate_embeddings_in_different_clusters_do_not_panic() {
        let e = unit_embedding(3);
        let input = vec![(0, e), (1, e), (2, e)];
        assert_eq!(absorb(&input, &[0, 1, 2], &[1.0, 1.0, 20.0], 0.75), vec![2, 2, 2]);
        let clustered = cluster_speakers(&input, 0.01);
        assert!(clustered.iter().all(|(_, l)| *l == clustered[0].1));
    }

    #[test]
    fn absorption_sums_durations_per_cluster_not_per_segment() {
        // Three 3 s segments of one speaker = 9 s >= 8 s: a real speaker even when similar.
        let input = vec![
            (0, mix(0, 1.0, 5, 0.1)),
            (1, unit_embedding(1)),
            (2, unit_embedding(1)),
            (3, unit_embedding(1)),
        ];
        let mut labels = vec![0, 1, 1, 1];
        absorb_small_clusters(&input, &mut labels, &[30.0, 3.0, 3.0, 3.0], 8.0, 2.0);
        assert_eq!(labels, vec![0, 1, 1, 1]);
    }

    #[test]
    fn rejected_cluster_is_reconsidered_after_a_merge_moves_a_centroid() {
        // A=(1,0,0) 1 s, B=(0,1,0) 8 s, C=(0.6,0.6,sqrt(.28)) 2 s. A rejects B (distance 1.0);
        // C merges into B (0.4); B's new centroid is ~0.665 from A, inside the 0.75 limit, so A
        // must then be absorbed too.
        let mut a = [0.0f32; EMBEDDING_DIM]; a[0] = 1.0;
        let mut b = [0.0f32; EMBEDDING_DIM]; b[1] = 1.0;
        let mut c = [0.0f32; EMBEDDING_DIM]; c[0] = 0.6; c[1] = 0.6; c[2] = 0.28f32.sqrt();
        let input = vec![(0, a), (1, b), (2, c)];
        assert_eq!(absorb(&input, &[0, 1, 2], &[1.0, 8.0, 2.0], 0.75), vec![1, 1, 1]);
        // Repeatable.
        assert_eq!(absorb(&input, &[0, 1, 2], &[1.0, 8.0, 2.0], 0.75), vec![1, 1, 1]);
    }

    #[test]
    fn centroid_change_after_a_merge_alters_a_later_target() {
        // Large clusters L0 (e0) and L1 (e1), small S1 (e0+e1 mix, 2 s) and S2 (1 s) nearer L0
        // than L1 at first; after S1 joins L1 nothing else changes for S2 as L0 stays its target.
        let input = vec![(0, unit_embedding(0)), (1, unit_embedding(1)), (2, mix(1, 1.0, 0, 0.3)), (3, mix(0, 1.0, 1, 0.4))];
        assert_eq!(absorb(&input, &[0, 1, 2, 3], &[30.0, 30.0, 2.0, 1.0], 0.75), vec![0, 1, 1, 0]);
    }

    #[test]
    fn merge_destination_that_is_itself_small_is_absorbed_later() {
        // Everything below 8 s: smallest merges toward the nearest, and the destination is then
        // absorbed in turn into the cluster that ends up largest.
        let input = vec![(0, mix(0, 1.0, 1, 0.1)), (1, mix(0, 1.0, 2, 0.1)), (2, mix(0, 1.0, 3, 0.1)), (3, mix(0, 1.0, 4, 0.1))];
        let out = absorb(&input, &[0, 1, 2, 3], &[1.0, 2.0, 3.0, 2.5], 0.75);
        assert!(out.iter().all(|l| *l == out[0]), "chain collapses to one cluster: {out:?}");
        assert_eq!(out, absorb(&input, &[0, 1, 2, 3], &[1.0, 2.0, 3.0, 2.5], 0.75));
    }

    fn count(result: &[(usize, usize)]) -> usize {
        result.iter().map(|r| r.1).collect::<std::collections::BTreeSet<_>>().len()
    }

    /// Two near-identical vectors (A), plus B and C which are 0.55 apart and 1.0 from A.
    fn abc() -> Vec<(usize, [f32; EMBEDDING_DIM])> {
        let a1 = mix(0, 1.0, 1, 0.05);
        let a2 = mix(0, 1.0, 2, 0.05);
        let b = unit_embedding(10);
        let c = mix(10, 0.5, 11, 1.0);
        vec![(0, a1), (1, a2), (2, b), (3, c)]
    }

    const LONG: [f64; 4] = [30.0; 4];

    #[test]
    fn count_cut_applies_the_lowest_dissimilarity_merges_first() {
        let input = abc();
        assert_eq!(count(&cluster_speakers(&input, 0.34)), 3);
        let two = cluster_to_count(&input, &LONG, 8.0, 2);
        assert_eq!(count(&two), 2);
        assert_eq!(two[0].1, two[1].1, "A merges first");
        assert_eq!(two[2].1, two[3].1, "then B and C (0.55) before A (1.0)");
        assert_eq!(count(&cluster_to_count(&input, &LONG, 8.0, 1)), 1);
        assert_eq!(count(&cluster_to_count(&input, &LONG, 8.0, 3)), 3);
        assert_eq!(count(&cluster_to_count(&input, &LONG, 8.0, 4)), 4);
    }

    #[test]
    fn count_larger_than_the_clusterable_items_is_capped() {
        let input = abc();
        assert_eq!(count(&cluster_to_count(&input, &LONG, 8.0, 99)), 4);
        // Short audio (no cluster reaches the minimum) falls back to a plain count cut.
        assert_eq!(count(&cluster_to_count(&input, &[1.0; 4], 8.0, 3)), 3);
        // A single item cannot be split; nothing to cluster is empty.
        assert_eq!(cluster_to_count(&input[..1], &[30.0], 8.0, 3), vec![(0, 0)]);
        assert!(cluster_to_count(&[], &[], 8.0, 3).is_empty());
    }

    #[test]
    fn count_cut_with_tied_distances_is_exact_and_deterministic() {
        // Four mutually orthogonal vectors: every pairwise distance is 1.0.
        let input: Vec<_> = (0..4).map(|i| (i, unit_embedding(i))).collect();
        for k in 1..=4 {
            let first = cluster_to_count(&input, &[10.0; 4], 8.0, k);
            assert_eq!(count(&first), k);
            assert_eq!(first, cluster_to_count(&input, &[10.0; 4], 8.0, k));
        }
    }

    #[test]
    fn count_cut_counts_speakers_not_outlier_segments() {
        // Two real speakers (A, B; 0.8 apart, 60 s each) and two 1 s outliers orthogonal to
        // everything. A naive 2-cluster cut keeps A and B together and splits off an outlier.
        let input = vec![
            (0, unit_embedding(0)),
            (1, mix(0, 1.0, 2, 0.05)),
            (2, mix(0, 0.2, 1, 1.0)),
            (3, mix(0, 0.2, 1, 1.0)),
            (4, unit_embedding(20)),
            (5, unit_embedding(21)),
        ];
        let durations = [30.0, 30.0, 30.0, 30.0, 1.0, 1.0];
        let two = cluster_to_count(&input, &durations, 8.0, 2);
        assert_eq!(count(&two), 2);
        assert_eq!(two[0].1, two[1].1);
        assert_eq!(two[2].1, two[3].1);
        assert_ne!(two[0].1, two[2].1, "A and B stay separate speakers");
        // Asking for more speakers than there are large clusters falls back to a plain cut.
        assert_eq!(count(&cluster_to_count(&input, &durations, 8.0, 5)), 5);
    }

    #[test]
    fn absorption_floor_never_drops_below_the_requested_count() {
        let input = vec![(0, unit_embedding(0)), (1, unit_embedding(1)), (2, mix(0, 0.8, 2, 0.6))];
        let run = |floor: usize| {
            let mut l = vec![0, 1, 2];
            absorb_small_clusters_floor(&input, &mut l, &[30.0, 30.0, 2.0], 8.0, 0.75, floor);
            l
        };
        assert_eq!(run(0), vec![0, 1, 0], "no floor: the small cluster is absorbed");
        assert_eq!(run(1), vec![0, 1, 0]);
        assert_eq!(run(2), vec![0, 1, 0], "two clusters remain, which meets the floor");
        assert_eq!(run(3), vec![0, 1, 2], "floor 3 forbids any merge");
        assert_eq!(run(10), vec![0, 1, 2]);
    }

    #[test]
    fn zero_and_non_finite_embeddings_are_unusable() {
        assert!(!is_usable_embedding(&[0.0; EMBEDDING_DIM]));
        let mut nan = unit_embedding(0);
        nan[3] = f32::NAN;
        assert!(!is_usable_embedding(&nan));
        assert!(is_usable_embedding(&unit_embedding(0)));
    }
}
