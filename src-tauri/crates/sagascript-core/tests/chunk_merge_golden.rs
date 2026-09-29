//! Golden tests: replay merges recorded from the parakeet-mlx Python reference.
//! Fixtures: tests/fixtures/chunk_merge (see README.md there).

use sagascript_core::transcription::chunk_merge::*;
use serde_json::Value;

const REAL: &str = include_str!("fixtures/chunk_merge/real_fleurs_sv_5min.json");
const SYNTH: &str = include_str!("fixtures/chunk_merge/synthetic.json");
const TOL: f64 = 1e-6;

fn toks(v: &Value) -> Vec<AlignedToken> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|r| {
            AlignedToken::new(
                r[0].as_u64().unwrap() as u32,
                r[1].as_str().unwrap(),
                r[2].as_f64().unwrap(),
                r[3].as_f64().unwrap(),
            )
        })
        .collect()
}

fn assert_same(got: &[AlignedToken], want: &[AlignedToken], ctx: &str) {
    assert_eq!(got.len(), want.len(), "{ctx}: length");
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        assert_eq!((g.id, &g.text), (w.id, &w.text), "{ctx}: token {i}");
        assert!((g.start - w.start).abs() < TOL, "{ctx}: start {i}");
        assert!((g.duration - w.duration).abs() < TOL, "{ctx}: duration {i}");
        assert!((g.end - w.end).abs() < TOL, "{ctx}: end {i}");
    }
}

/// Replay one recorded merge; `a` is stored without its untouched prefix.
fn replay_case(c: &Value, ctx: &str) -> MergePath {
    let overlap = c["overlap"].as_f64().unwrap();
    let (a, b) = (toks(&c["a"]), toks(&c["b"]));
    match (&c["contiguous"], merge_longest_contiguous(&a, &b, overlap)) {
        (Value::Null, Err(_)) => {}
        (want, Ok(got)) if !want.is_null() => assert_same(&got, &toks(want), &format!("{ctx} contiguous")),
        (want, got) => panic!("{ctx}: contiguous mismatch want={want:?} got_ok={}", got.is_ok()),
    }
    assert_same(
        &merge_longest_common_subsequence(&a, &b, overlap),
        &toks(&c["lcs"]),
        &format!("{ctx} lcs"),
    );
    let (got, path) = merge_window_traced(&a, &b, overlap);
    let want_path = match c["path"].as_str().unwrap() {
        "contiguous" => MergePath::Contiguous,
        "lcs" => MergePath::Lcs,
        p => panic!("unknown path {p}"),
    };
    assert_eq!(path, want_path, "{ctx}: path");
    assert_same(&got, &toks(&c["result"]), &format!("{ctx} result"));
    path
}

#[test]
fn chunk_merge_synthetic_cases_replay() {
    let v: Value = serde_json::from_str(SYNTH).unwrap();
    let cases = v["cases"].as_array().unwrap();
    assert!(cases.len() >= 10);
    let mut lcs_seen = false;
    for c in cases {
        let name = c["name"].as_str().unwrap();
        lcs_seen |= replay_case(c, name) == MergePath::Lcs;
    }
    assert!(lcs_seen, "synthetic set must force the LCS fallback");
}

#[test]
fn chunk_merge_synthetic_text_and_sentences() {
    let v: Value = serde_json::from_str(SYNTH).unwrap();
    let tc = &v["text_case"];
    let t = toks(&tc["tokens"]);
    let sentences: Vec<String> = tokens_to_sentences(&t).into_iter().map(|s| s.text).collect();
    let want: Vec<String> = tc["sentences"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s.as_str().unwrap().to_string())
        .collect();
    assert_eq!(sentences, want);
    assert_eq!(tokens_to_text(&t), tc["text"].as_str().unwrap());
}

#[test]
fn chunk_merge_real_audio_replay() {
    let v: Value = serde_json::from_str(REAL).unwrap();
    let total = v["total_samples"].as_u64().unwrap() as usize;
    let sr = v["sample_rate"].as_u64().unwrap() as u32;
    for run in v["runs"].as_array().unwrap() {
        let (win_s, ov_s) = (run["window_s"].as_f64().unwrap(), run["overlap_s"].as_f64().unwrap());
        let ctx = format!("window={win_s} overlap={ov_s}");
        let windows = run["windows"].as_array().unwrap();

        let plan = plan_windows(total, sr, win_s, ov_s);
        assert_eq!(plan.len(), windows.len(), "{ctx}: window count");
        for (p, w) in plan.iter().zip(windows) {
            assert_eq!(p.start_sample as u64, w["start_sample"].as_u64().unwrap(), "{ctx}");
            assert_eq!(p.end_sample as u64, w["end_sample"].as_u64().unwrap(), "{ctx}");
        }

        // Every recorded merge call individually.
        let merges = run["merges"].as_array().unwrap();
        assert_eq!(merges.len(), windows.len() - 1);
        let mut lcs = 0;
        for (i, m) in merges.iter().enumerate() {
            lcs += usize::from(replay_case(m, &format!("{ctx} merge {i}")) == MergePath::Lcs);
        }
        assert!(lcs > 0, "{ctx}: fixture should exercise the LCS fallback");

        // Full pipeline: window tokens -> merged tokens -> sentences -> text.
        let per_window: Vec<_> = windows.iter().map(|w| toks(&w["tokens"])).collect();
        let merged = merge_all(per_window, ov_s);
        let sentences = tokens_to_sentences(&merged);
        let flat: Vec<AlignedToken> = sentences.iter().flat_map(|s| s.tokens.clone()).collect();
        assert_same(&flat, &toks(&run["final_tokens"]), &format!("{ctx} final tokens"));
        let want_sentences: Vec<&str> = run["sentences"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s.as_str().unwrap())
            .collect();
        let got_sentences: Vec<&str> = sentences.iter().map(|s| s.text.as_str()).collect();
        assert_eq!(got_sentences, want_sentences, "{ctx}: sentences");
        assert_eq!(tokens_to_text(&merged), run["final_text"].as_str().unwrap(), "{ctx}: text");
    }
}
