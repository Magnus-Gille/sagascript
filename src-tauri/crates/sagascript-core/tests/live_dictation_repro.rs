//! Reproduction harness for long Pianissimo dictations through a real engine
//! host (a 64 s dictation once failed with "engine host did not answer
//! `transcribe_window` within 45s"). Ignored: needs the installed engine host
//! (`SAGASCRIPT_ENGINE_HOST`), the installed Swedish model and a 16 kHz mono
//! speech file (`SAGASCRIPT_REPRO_AUDIO`).

use std::sync::atomic::AtomicBool;
use std::time::Instant;

use sagascript_core::audio::decoder::decode_audio_file;
use sagascript_core::models::PianissimoModel;
use sagascript_core::transcription::engine_host::CancelToken;
use sagascript_core::transcription::pianissimo_backend::PianissimoBackend;

#[test]
#[ignore = "needs the installed engine host, the Swedish model and SAGASCRIPT_REPRO_AUDIO"]
fn long_dictations_through_the_installed_host() {
    let path = std::env::var("SAGASCRIPT_REPRO_AUDIO").expect("SAGASCRIPT_REPRO_AUDIO");
    let samples = decode_audio_file(path.as_ref()).expect("decode");
    let settings = sagascript_core::machine::load();
    let backend = PianissimoBackend::start_with_cancel(
        &AtomicBool::new(false),
        &settings,
        PianissimoModel::Swedish,
    )
    .expect("start");
    let config = backend.client().config();
    let window_s = backend
        .client()
        .capabilities()
        .map_or(15.0, |caps| caps.preferred_window_s);
    eprintln!(
        "effective timeouts for a {window_s} s window: interactive {:?}, batch {:?}, load {:?}, \
         wedge grace {:?}; max_crash_retries {}\n{:?}",
        config.timeouts.interactive(window_s),
        config.timeouts.transcribe(window_s),
        config.timeouts.load,
        config.timeouts.wedge_grace,
        config.max_crash_retries,
        config.timeouts,
    );
    let lengths: Vec<usize> = std::env::var("SAGASCRIPT_REPRO_SECONDS")
        .unwrap_or_else(|_| "5,20,53,64,64,90,120".into())
        .split(',')
        .map(|s| s.trim().parse().expect("seconds"))
        .collect();
    let mut failures = 0;
    for seconds in lengths {
        let n = (seconds * 16_000).min(samples.len());
        let started = Instant::now();
        let result = backend
            .client()
            .transcribe_dictation(&samples[..n], &CancelToken::new());
        let elapsed = started.elapsed().as_secs_f64();
        match &result {
            Ok(t) => eprintln!(
                "{seconds:>4} s: ok in {elapsed:.2} s, {} windows, round trips {:?} ms, encode {:?} ms",
                t.windows.len(),
                t.windows.iter().map(|w| w.round_trip_ms).collect::<Vec<_>>(),
                t.windows.iter().map(|w| w.encode_ms).collect::<Vec<_>>()
            ),
            Err(e) => {
                failures += 1;
                eprintln!("{seconds:>4} s: FAILED after {elapsed:.2} s: {e}");
            }
        }
    }
    let snapshot = backend.client().snapshot();
    eprintln!(
        "host: spawns {}, loads {}, crashes in window {}",
        snapshot.spawns, snapshot.loads, snapshot.crashes_in_window
    );
    assert_eq!(failures, 0, "dictations failed");
}
