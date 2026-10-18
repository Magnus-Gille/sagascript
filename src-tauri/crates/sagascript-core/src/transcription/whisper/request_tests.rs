use super::*;
use std::cell::Cell;
use std::sync::{Arc, Mutex};

static SILENCE: [f32; 8_000] = [0.0; 8_000];

fn tone() -> Vec<f32> {
    vec![0.3; 8_000]
}

#[test]
fn a_new_request_is_a_plain_one_without_hooks() {
    let options = TranscribeOptions::default();
    let request = TranscribeRequest::new(&SILENCE, Language::Swedish, &options);

    assert_eq!(request.audio.len(), SILENCE.len());
    assert_eq!(request.language, Language::Swedish);
    assert!(std::ptr::eq(request.options, &options));
    assert!(request.hooks.on_progress.is_none());
    assert!(request.hooks.on_encode_start.is_none());
    assert!(!request.live);
}

#[test]
fn the_builders_set_one_field_each() {
    let options = TranscribeOptions::default();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let encoded = Cell::new(0);
    let on_encode_start = || encoded.set(encoded.get() + 1);
    let sink = Arc::clone(&seen);

    let mut request = TranscribeRequest::new(&SILENCE, Language::English, &options)
        .live()
        .on_progress(move |percent| sink.lock().unwrap().push(percent))
        .on_encode_start(&on_encode_start);

    assert!(request.live);
    (request.hooks.on_progress.as_mut().expect("progress hook"))(42);
    (request.hooks.on_encode_start.expect("encode hook"))();
    assert_eq!(*seen.lock().unwrap(), vec![42]);
    assert_eq!(encoded.get(), 1);
}

#[test]
fn hooks_default_to_none() {
    let hooks = TranscribeHooks::default();

    assert!(hooks.on_progress.is_none());
    assert!(hooks.on_encode_start.is_none());
}

#[test]
fn a_request_that_is_not_live_is_never_guarded() {
    let options = TranscribeOptions::default();

    // Silent and even empty audio pass through: file and diagnostic callers,
    // and the intentional silent warm-up, stay raw and fail later, if at all.
    for audio in [&SILENCE[..], &[][..]] {
        let request = TranscribeRequest::new(audio, Language::English, &options);
        let passed = request.after_live_guard().unwrap().expect("not skipped");
        assert_eq!(passed.audio.len(), audio.len());
        assert!(!passed.live);
    }
}

#[test]
fn live_near_silence_is_skipped_before_any_decoder_work() {
    let options = TranscribeOptions::default();
    let request = TranscribeRequest::new(&SILENCE, Language::English, &options).live();

    assert!(request.after_live_guard().unwrap().is_none());
}

#[test]
fn live_speech_passes_with_the_guard_spent() {
    let options = TranscribeOptions::default();
    let audio = tone();
    let request = TranscribeRequest::new(&audio, Language::English, &options).live();

    let passed = request.after_live_guard().unwrap().expect("not skipped");

    assert!(!passed.live, "the layer below must not guard a second time");
    assert_eq!(passed.audio.len(), audio.len());
}

#[test]
fn live_empty_and_non_finite_audio_are_rejected_not_skipped() {
    let options = TranscribeOptions::default();

    let empty = TranscribeRequest::new(&[], Language::English, &options).live();
    assert!(matches!(
        empty.after_live_guard(),
        Err(DictationError::NoAudioCaptured)
    ));

    for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let audio = [0.0, invalid, 0.0];
        let request = TranscribeRequest::new(&audio, Language::English, &options).live();
        assert!(matches!(
            request.after_live_guard(),
            Err(DictationError::TranscriptionFailed(message)) if message.contains("non-finite")
        ));
    }
}
