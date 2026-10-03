//! Diarization-only evaluation helper (issue #284); no Whisper involved.
//!
//!   diarize_eval analyze <audio> <analysis.json>
//!   diarize_eval cluster <analysis.json> <threshold> <segments.json> [min_speaker_seconds] [absorb_max_distance] [hint] [hint_merge_max_distance]
//!
//! `hint` is a speaker-count hint (issue #305): `N` for exactly N speakers, `N!` for a forced exact
//! count (merges regardless of distance), or `MIN-MAX`, `MIN-`, `-MAX`.
//! Omit it for the plain threshold cut.
use sagascript_core::diarization::{self, DiarizationAnalysis, DiarizeConfig, SpeakerCountHint};

fn parse_hint(text: &str) -> SpeakerCountHint {
    let number = |t: &str| (!t.is_empty()).then(|| t.parse::<usize>().expect("speaker count"));
    if let Some(n) = text.strip_suffix('!') {
        return SpeakerCountHint::forced(number(n).expect("speaker count"));
    }
    match text.split_once('-') {
        None => SpeakerCountHint::exact(number(text).expect("speaker count")),
        Some((min, max)) => SpeakerCountHint::range(number(min), number(max)),
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a.get(1).map(String::as_str) {
        Some("analyze") => {
            let audio = sagascript_core::audio::decoder::decode_audio_file(std::path::Path::new(&a[2])).expect("decode");
            let t = std::time::Instant::now();
            let (analysis, timings) = diarization::analyze(&audio, &DiarizeConfig::default()).expect("analyze");
            eprintln!("analysis {:.1}s: {timings:?}", t.elapsed().as_secs_f64());
            std::fs::write(&a[3], serde_json::to_vec(&analysis).unwrap()).unwrap();
        }
        Some("cluster") => {
            let analysis: DiarizationAnalysis = serde_json::from_slice(&std::fs::read(&a[2]).unwrap()).unwrap();
            let mut cfg = DiarizeConfig { threshold: a[3].parse().unwrap(), ..Default::default() };
            if let Some(m) = a.get(5) { cfg.min_speaker_seconds = m.parse().unwrap(); }
            if let Some(m) = a.get(6) { cfg.absorb_max_distance = m.parse().unwrap(); }
            if let Some(h) = a.get(7) { cfg.speaker_hint = Some(parse_hint(h)); }
            if let Some(d) = a.get(8) { cfg.hint_merge_max_distance = d.parse().unwrap(); }
            let t = std::time::Instant::now();
            let segs = diarization::cluster(&analysis, &cfg).expect("cluster");
            eprintln!("cluster {:.3}s", t.elapsed().as_secs_f64());
            std::fs::write(&a[4], serde_json::to_vec(&segs).unwrap()).unwrap();
        }
        _ => eprintln!("usage: analyze|cluster"),
    }
}
