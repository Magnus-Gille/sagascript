//! Diarization-only evaluation helper (issue #284); no Whisper involved.
//!
//!   diarize_eval analyze <audio> <analysis.json>
//!   diarize_eval cluster <analysis.json> <threshold> <segments.json>
use sagascript_core::diarization::{self, DiarizationAnalysis, DiarizeConfig};

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
            let cfg = DiarizeConfig { threshold: a[3].parse().unwrap(), ..Default::default() };
            let t = std::time::Instant::now();
            let segs = diarization::cluster(&analysis, &cfg).expect("cluster");
            eprintln!("cluster {:.3}s", t.elapsed().as_secs_f64());
            std::fs::write(&a[4], serde_json::to_vec(&segs).unwrap()).unwrap();
        }
        _ => eprintln!("usage: analyze|cluster"),
    }
}
