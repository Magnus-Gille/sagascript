//! Opt-in regression check for issue #236 (silently dropped words on long
//! audio when whisper decodes without timestamps).
//!
//! Ignored by default: needs a local KB-Whisper model (never downloaded here)
//! and a long Swedish recording plus its reference transcript.
//!
//! ```text
//! SAGASCRIPT_LONGFORM_WAV=~/.cache/sagascript-bench/longform/fleurs-sv-distinct-5min.wav \
//! SAGASCRIPT_LONGFORM_REF=~/.cache/sagascript-bench/longform/fleurs-sv-distinct-5min.txt \
//! cargo test -p sagascript-core --test longform_deletions -- --ignored --nocapture
//! ```

use std::path::PathBuf;

use sagascript_core::audio::decoder::decode_audio_file;
use sagascript_core::settings::{Language, WhisperModel};
use sagascript_core::transcription::model;
use sagascript_core::transcription::whisper_backend::WhisperBackend;
use sagascript_core::transcription::TranscribeOptions;

fn words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|w| {
            w.chars()
                .filter(|c| c.is_alphanumeric())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        })
        .filter(|w| !w.is_empty())
        .collect()
}

/// Word-level edit distance returning the number of deletions on a minimum-cost path.
fn deletions(reference: &[String], hypothesis: &[String]) -> usize {
    // (cost, deletions) per cell.
    let mut prev: Vec<(usize, usize)> = (0..=hypothesis.len()).map(|j| (j, 0)).collect();
    for i in 1..=reference.len() {
        let mut cur = vec![(i, i)];
        for j in 1..=hypothesis.len() {
            let sub = (
                prev[j - 1].0 + usize::from(reference[i - 1] != hypothesis[j - 1]),
                prev[j - 1].1,
            );
            let del = (prev[j].0 + 1, prev[j].1 + 1);
            let ins = (cur[j - 1].0 + 1, cur[j - 1].1);
            cur.push([sub, del, ins].into_iter().min().unwrap());
        }
        prev = cur;
    }
    prev[hypothesis.len()].1
}

#[test]
#[ignore = "needs SAGASCRIPT_LONGFORM_WAV/REF and a local KB-Whisper model"]
fn long_swedish_file_does_not_drop_words() {
    let (Some(wav), Some(reference)) = (
        std::env::var_os("SAGASCRIPT_LONGFORM_WAV").map(PathBuf::from),
        std::env::var_os("SAGASCRIPT_LONGFORM_REF").map(PathBuf::from),
    ) else {
        panic!("set SAGASCRIPT_LONGFORM_WAV and SAGASCRIPT_LONGFORM_REF");
    };
    let model_id = WhisperModel::KbWhisperMedium;
    assert!(
        model::is_model_downloaded(model_id),
        "KB-Whisper medium model not present at {}",
        model::model_path(model_id).display()
    );

    let audio = decode_audio_file(&wav).expect("decode audio");
    let backend = WhisperBackend::new();
    backend.load_model(model_id).expect("load model");
    let opts = TranscribeOptions {
        beam_size: 5,
        ..Default::default()
    };
    let text = backend
        .transcribe_sync_with_options(&audio, Language::Swedish, &opts, |_| {}, None)
        .expect("transcribe");

    let reference = words(&std::fs::read_to_string(reference).expect("read reference"));
    let hypothesis = words(&text);
    let deleted = deletions(&reference, &hypothesis);
    let ratio = deleted as f64 / reference.len() as f64;
    println!("deletions {deleted}/{} = {:.2}%", reference.len(), ratio * 100.0);
    assert!(ratio < 0.03, "dropped {:.2}% of reference words", ratio * 100.0);
}
