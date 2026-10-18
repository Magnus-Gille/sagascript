use super::*;
use crate::models::Language;
use crate::transcription::whisper::{DictationTimings, TranscribeOptions};

#[cfg(test)]
mod packaged_x64_cpu_tests {
    use super::require_packaged_x64_cpu;

    #[test]
    fn packaged_baseline_requires_every_feature_before_native_model_loading() {
        for bits in 0..16 {
            let features = std::array::from_fn(|index| bits & (1 << index) != 0);
            assert_eq!(
                require_packaged_x64_cpu(Some("avx2"), features).is_ok(),
                bits == 15
            );
            assert!(require_packaged_x64_cpu(None, features).is_ok());
        }
        assert!(require_packaged_x64_cpu(Some("unknown"), [true; 4]).is_err());
    }

    #[test]
    fn unsupported_cpu_error_is_actionable_and_contains_no_model_or_audio_data() {
        let error = require_packaged_x64_cpu(Some("avx2"), [false; 4])
            .unwrap_err()
            .to_string();
        for feature in ["AVX2", "FMA", "F16C", "BMI2", "source build"] {
            assert!(error.contains(feature));
        }
    }
}

#[cfg(test)]
mod warm_model_cache_tests {
    use super::*;

    fn key(model: WhisperModel, profile: ContextProfile) -> RuntimeKey {
        RuntimeKey { model, profile }
    }

    #[test]
    fn distinguishes_active_cached_and_missing_models() {
        let swedish = key(WhisperModel::KbWhisperBase, ContextProfile::FlashAttention);
        let english = key(WhisperModel::BaseEn, ContextProfile::FlashAttention);
        assert_eq!(
            model_availability(Some(swedish), Some(english), swedish),
            ModelAvailability::Active
        );
        assert_eq!(
            model_availability(Some(swedish), Some(english), english),
            ModelAvailability::Cached
        );
        assert_eq!(
            model_availability(
                Some(swedish),
                Some(english),
                key(WhisperModel::KbWhisperBase, ContextProfile::TokenAlignment,),
            ),
            ModelAvailability::Missing
        );
    }

    #[test]
    fn caches_two_base_models_within_budget() {
        assert!(can_cache_pair(
            key(WhisperModel::KbWhisperBase, ContextProfile::FlashAttention,),
            key(WhisperModel::BaseEn, ContextProfile::FlashAttention),
        ));
    }

    #[test]
    fn refuses_duplicate_or_oversized_cache_pairs() {
        let english = key(WhisperModel::BaseEn, ContextProfile::FlashAttention);
        assert!(!can_cache_pair(english, english));
        assert!(!can_cache_pair(
            key(
                WhisperModel::KbWhisperMedium,
                ContextProfile::FlashAttention,
            ),
            english,
        ));
    }

    #[test]
    fn treats_same_model_with_different_profiles_as_distinct_runtimes() {
        let flash = key(WhisperModel::BaseEn, ContextProfile::FlashAttention);
        let alignment = key(WhisperModel::BaseEn, ContextProfile::TokenAlignment);

        assert_eq!(
            model_availability(Some(flash), None, alignment),
            ModelAvailability::Missing
        );
        assert!(can_cache_pair(flash, alignment));
    }

    #[test]
    fn selects_token_alignment_only_for_diarization() {
        assert_eq!(
            ContextProfile::for_diarization(false),
            ContextProfile::FlashAttention
        );
        assert_eq!(
            ContextProfile::for_diarization(true),
            ContextProfile::TokenAlignment
        );
    }

    #[test]
    fn cached_runtime_switch_preserves_both_warm_payloads() {
        let swedish = key(WhisperModel::KbWhisperBase, ContextProfile::FlashAttention);
        let english = key(WhisperModel::BaseEn, ContextProfile::FlashAttention);
        let mut active_key = Some(swedish);
        let mut active_context = Some("swedish-context");
        let mut active_state = Some("swedish-state");
        let mut cached = Some(CachedWhisperRuntime {
            key: english,
            context: "english-context",
            state: Some("english-state"),
        });

        assert!(activate_cached_runtime(
            english,
            &mut active_key,
            &mut active_context,
            &mut active_state,
            &mut cached,
        ));
        assert_eq!(active_key, Some(english));
        assert_eq!(active_context, Some("english-context"));
        assert_eq!(active_state, Some("english-state"));

        let cached = cached.unwrap();
        assert_eq!(cached.key, swedish);
        assert_eq!(cached.context, "swedish-context");
        assert_eq!(cached.state, Some("swedish-state"));
    }

    #[test]
    fn cache_miss_leaves_active_and_secondary_unchanged() {
        let swedish = key(WhisperModel::KbWhisperBase, ContextProfile::FlashAttention);
        let english = key(WhisperModel::BaseEn, ContextProfile::FlashAttention);
        let mut active_key = Some(swedish);
        let mut active_context = Some("swedish-context");
        let mut active_state = Some("swedish-state");
        let mut cached = Some(CachedWhisperRuntime {
            key: english,
            context: "english-context",
            state: Some("english-state"),
        });

        assert!(!activate_cached_runtime(
            key(WhisperModel::NbWhisperBase, ContextProfile::FlashAttention,),
            &mut active_key,
            &mut active_context,
            &mut active_state,
            &mut cached,
        ));
        assert_eq!(active_key, Some(swedish));
        assert_eq!(active_context, Some("swedish-context"));
        assert_eq!(active_state, Some("swedish-state"));
        assert_eq!(cached.as_ref().unwrap().key, english);
    }
}

/// Tests for the WP2b hardening: the bounded-wait `ModelBusy` guard and the real
/// abort callback. These exercise only the locking/flag machinery — they need no
/// loaded model, because the state mutex holds `Option<WhisperState>` and can be
/// locked while holding `None`.
#[cfg(test)]
mod warm_state_tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;

    /// Acquire and hold the (private) state mutex on a background thread, blocking
    /// until `hold` elapses. Returns once the lock is confirmed held, plus the
    /// join handle so the caller can wait for release.
    fn hold_state_lock(backend: &Arc<WhisperBackend>, hold: Duration) -> thread::JoinHandle<()> {
        let holder = Arc::clone(backend);
        let (tx, rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            let _guard = holder.state.lock().unwrap();
            tx.send(()).expect("signal lock acquired");
            thread::sleep(hold);
        });
        rx.recv()
            .expect("background thread should acquire the lock");
        handle
    }

    #[test]
    fn lock_state_bounded_returns_model_busy_when_held_past_grace() {
        let backend = Arc::new(WhisperBackend::new());
        // Hold the lock well past the grace budget.
        let holder = hold_state_lock(&backend, Duration::from_millis(400));

        let start = Instant::now();
        let res = lock_state_slot_bounded(&backend.state, Duration::from_millis(50));
        let elapsed = start.elapsed();

        assert!(
            matches!(res, Err(DictationError::ModelBusy)),
            "expected ModelBusy while the lock is held past grace, got {res:?}"
        );
        // Must give up near the grace budget, NOT block until the holder releases.
        assert!(
            elapsed < Duration::from_millis(300),
            "should return within ~grace, not block for the full hold; took {elapsed:?}"
        );
        holder.join().unwrap();
    }

    #[test]
    fn lock_state_bounded_acquires_when_lock_freed_within_grace() {
        // Back-to-back safety: a briefly-held lock (released well within grace)
        // must NOT produce a spurious ModelBusy.
        let backend = Arc::new(WhisperBackend::new());
        let holder = hold_state_lock(&backend, Duration::from_millis(60));

        let res = lock_state_slot_bounded(&backend.state, Duration::from_secs(2));
        assert!(
            res.is_ok(),
            "should acquire once the briefly-held lock is released, got {res:?}"
        );
        drop(res); // release before joining
        holder.join().unwrap();
    }

    #[test]
    fn lock_state_bounded_acquires_immediately_when_uncontended() {
        let backend = WhisperBackend::new();
        let res = lock_state_slot_bounded(&backend.state, Duration::from_millis(0));
        assert!(res.is_ok(), "uncontended lock must be acquired immediately");
    }

    #[test]
    fn abort_trampoline_polarity_matches_ggml() {
        // ggml contract: the abort callback returns `true` to ABORT. Verify the
        // trampoline mirrors the AtomicBool and treats null as "don't abort".
        let flag = AtomicBool::new(false);
        let ptr = &flag as *const AtomicBool as *mut c_void;
        unsafe {
            assert!(
                !WhisperBackend::abort_trampoline(ptr),
                "flag=false must NOT abort"
            );
            flag.store(true, Ordering::SeqCst);
            assert!(
                WhisperBackend::abort_trampoline(ptr),
                "flag=true must abort"
            );
            assert!(
                !WhisperBackend::abort_trampoline(std::ptr::null_mut()),
                "null user_data must NOT abort"
            );
        }
    }

    /// Abort-flag ownership: only the call that ACQUIRES the state lock may
    /// touch the flag. A caller that gives up with ModelBusy never owned the
    /// lock, so it must leave a pending abort request (belonging to the current
    /// lock holder's inference) untouched — otherwise it would cancel the abort
    /// of a stuck call and re-introduce the wedge.
    #[test]
    fn model_busy_return_leaves_abort_flag_untouched() {
        let backend = Arc::new(WhisperBackend::new());
        let holder = hold_state_lock(&backend, Duration::from_millis(300));

        // A timed-out caller has requested an abort of the in-flight inference.
        backend.request_abort();
        assert!(backend.abort_flag.load(Ordering::SeqCst));

        // A new call that fails with ModelBusy must NOT clear that request.
        let res = backend.with_warm_state_grace(Duration::from_millis(30), |_state| Ok(()));
        assert!(
            matches!(res, Err(DictationError::ModelBusy)),
            "expected ModelBusy, got {res:?}"
        );
        assert!(
            backend.abort_flag.load(Ordering::SeqCst),
            "ModelBusy path must not clear a pending abort request — that would \
             cancel the abort of the still-running inference and re-wedge the app"
        );
        holder.join().unwrap();
    }

    /// Once a call ACQUIRES the state lock, any prior abort request is stale by
    /// definition (its target inference has finished and released the lock —
    /// the flag lifecycle is owned by the lock holder), so it is cleared before
    /// the new inference starts. Exercised via the ModelNotLoaded early-return,
    /// which happens after lock acquisition but needs no model file.
    #[test]
    fn stale_abort_flag_cleared_once_lock_is_owned() {
        let backend = WhisperBackend::new();
        backend.request_abort();
        assert!(backend.abort_flag.load(Ordering::SeqCst));

        // Uncontended: the call acquires the lock, then fails ModelNotLoaded
        // (no context) — but by then it owns the flag and must have cleared it.
        let res = backend.with_warm_state_grace(Duration::from_millis(0), |_state| Ok(()));
        assert!(
            matches!(res, Err(DictationError::ModelNotLoaded)),
            "expected ModelNotLoaded (no context in test env), got {res:?}"
        );
        assert!(
            !backend.abort_flag.load(Ordering::SeqCst),
            "a call that acquired the state lock must clear the stale abort flag \
             before starting its own inference"
        );
    }

    /// A sticky cancel that lands between inference calls is not lost to the
    /// lock-acquisition flag clear: the next call fails without running.
    #[test]
    fn sticky_cancel_survives_lock_acquisition_and_blocks_new_inference() {
        let backend = WhisperBackend::new();
        backend.request_cancel();
        let mut ran = false;
        let res = backend.with_warm_state_grace(Duration::from_millis(0), |_state| {
            ran = true;
            Ok(())
        });
        assert!(!ran, "no inference may start after a sticky cancel");
        assert!(
            matches!(res, Err(DictationError::Cancelled)),
            "a sticky cancel is the Cancelled error, got {res:?}"
        );
        assert!(backend.is_cancel_requested());
    }

    /// A panic mid-inference poisons the warm-state lock (#359). The next
    /// transcription must get a clean error or result, not a panic, and must
    /// not reuse the interrupted warm state.
    #[test]
    fn warm_state_poisoned_by_a_panic_is_recovered_and_discarded() {
        let backend = Arc::new(WhisperBackend::new());
        let holder = Arc::clone(&backend);
        let panicked = thread::spawn(move || {
            let _state = holder.state.lock().unwrap();
            let _context = holder.context.lock().unwrap();
            panic!("injected panic mid-inference (#359)");
        })
        .join();
        assert!(panicked.is_err());
        assert!(backend.state.is_poisoned() && backend.context.is_poisoned());

        let res = backend.with_warm_state_grace(Duration::from_millis(0), |_state| Ok(()));
        assert!(
            matches!(res, Err(DictationError::ModelNotLoaded)),
            "expected a clean ModelNotLoaded after recovery, got {res:?}"
        );
        assert!(!backend.state.is_poisoned());
        assert!(backend.state.lock().unwrap().is_none());
        assert_eq!(backend.loaded_model(), None);
    }

    /// Pins the documented fail-closed exception (ADR 0005): dictation holds
    /// `load_lock` across inference, so a panic mid-inference poisons it as
    /// well as `state`. Every later dictation then returns the actionable
    /// restart error; it neither panics nor resumes the interrupted runtime.
    #[test]
    fn panic_mid_dictation_keeps_failing_closed_with_the_restart_error() {
        let backend = Arc::new(WhisperBackend::new());
        let holder = Arc::clone(&backend);
        let panicked = thread::spawn(move || {
            let _load = holder.load_lock.lock().unwrap();
            let _state = holder.state.lock().unwrap();
            panic!("injected panic mid-inference (#359)");
        })
        .join();
        assert!(panicked.is_err());
        assert!(backend.load_lock.is_poisoned() && backend.state.is_poisoned());

        for _ in 0..2 {
            let mut timings = DictationTimings::default();
            let result = backend.transcribe_dictation(
                WhisperModel::Base,
                &[0.0; 8_000],
                Language::English,
                &TranscribeOptions::default(),
                &mut timings,
            );
            assert!(
                matches!(result, Err(DictationError::TranscriptionFailed(ref message))
                    if message.contains("restart Sagascript")),
                "expected the fail-closed restart error, got {result:?}"
            );
            assert!(!timings.inference_started);
        }
        assert!(backend.load_lock.is_poisoned(), "fail-closed is sticky");
    }
}

/// Characterisation of the load lock and the abort and cancel flags (#371),
/// pinned before `whisper_backend.rs` is split. Nothing here needs model
/// weights: a runtime key can be published without a context, and every
/// assertion stops at the lock, flag or error that comes before FFI.
#[cfg(test)]
mod lock_protocol_tests {
    use super::*;
    use std::sync::mpsc;
    use std::thread;

    const MODEL: WhisperModel = WhisperModel::Base;

    fn key(profile: ContextProfile) -> RuntimeKey {
        RuntimeKey {
            model: MODEL,
            profile,
        }
    }

    /// A backend whose active runtime is published without a context, so
    /// `ensure_model*` sees the model as already active and never reads a
    /// model file.
    fn backend_with_active_runtime() -> Arc<WhisperBackend> {
        let backend = Arc::new(WhisperBackend::new());
        *backend.loaded_runtime.lock().unwrap() = Some(key(ContextProfile::FlashAttention));
        backend
    }

    /// Hold `load_lock` on a background thread, as a transcription in flight
    /// does, until the returned sender fires or is dropped. The lock is
    /// confirmed held before this returns.
    fn hold_load_lock(backend: &Arc<WhisperBackend>) -> (mpsc::Sender<()>, thread::JoinHandle<()>) {
        let holder = Arc::clone(backend);
        let (locked_tx, locked_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let handle = thread::spawn(move || {
            let _load = holder.load_lock.lock().unwrap();
            locked_tx.send(()).expect("signal the lock is held");
            let _ = release_rx.recv();
        });
        locked_rx.recv().expect("holder should acquire load_lock");
        (release_tx, handle)
    }

    fn poison_lock<T: Send + 'static>(
        backend: &Arc<WhisperBackend>,
        pick: fn(&WhisperBackend) -> &Mutex<T>,
    ) {
        let holder = Arc::clone(backend);
        let panicked = thread::spawn(move || {
            let _guard = pick(&holder).lock().unwrap();
            panic!("injected panic to poison a whisper lock");
        })
        .join();
        assert!(panicked.is_err());
    }

    #[test]
    fn the_bounded_wait_budget_is_three_seconds_polled_in_short_steps() {
        assert_eq!(WARM_STATE_GRACE, Duration::from_secs(3));
        assert_eq!(WARM_STATE_POLL_INTERVAL, Duration::from_millis(25));
    }

    #[test]
    fn abort_request_flips_the_flag_the_ffi_trampoline_reads() {
        let backend = WhisperBackend::new();
        let data = Arc::as_ptr(&backend.abort_flag) as *mut c_void;

        // SAFETY: `data` points at the backend's own flag, which outlives these calls.
        assert!(!unsafe { WhisperBackend::abort_trampoline(data) });
        backend.request_abort();
        assert!(unsafe { WhisperBackend::abort_trampoline(data) });
        assert!(
            !backend.is_cancel_requested(),
            "an abort alone is not a sticky cancel"
        );
    }

    #[test]
    fn cancel_request_aborts_now_and_stays_requested() {
        let backend = WhisperBackend::new();
        assert!(!backend.is_cancel_requested());

        backend.request_cancel();

        assert!(backend.is_cancel_requested());
        assert!(backend.abort_flag.load(Ordering::SeqCst));
        // Reading the flag never clears either of them.
        assert!(backend.is_cancel_requested());
        assert!(backend.abort_flag.load(Ordering::SeqCst));
    }

    /// `ensure_model*`, `load_model*`, `with_model` and `transcribe_dictation`
    /// all wait on `load_lock` for the same bounded budget and then report
    /// `ModelBusy`. They run in parallel (one backend each), so the three
    /// budget-length tests below cost about one budget in wall time.
    #[test]
    fn ensure_model_reports_model_busy_once_the_budget_expires_while_a_load_is_in_flight() {
        let backend = backend_with_active_runtime();
        let (release, holder) = hold_load_lock(&backend);

        let started = Instant::now();
        let result = backend.ensure_model_with_profile(MODEL, ContextProfile::FlashAttention);
        let waited = started.elapsed();

        assert!(
            matches!(result, Err(DictationError::ModelBusy)),
            "expected ModelBusy, got {result:?}"
        );
        assert!(waited >= WARM_STATE_GRACE, "gave up early after {waited:?}");
        release.send(()).unwrap();
        holder.join().unwrap();

        // Once the holder is gone the already-active runtime is accepted
        // without loading anything.
        backend
            .ensure_model_with_profile(MODEL, ContextProfile::FlashAttention)
            .expect("an active runtime needs no load");
    }

    #[test]
    fn load_model_reports_model_busy_before_touching_any_model_file() {
        let backend = Arc::new(WhisperBackend::new());
        let (release, holder) = hold_load_lock(&backend);

        let result = backend.load_model_with_profile(MODEL, ContextProfile::FlashAttention);

        assert!(
            matches!(result, Err(DictationError::ModelBusy)),
            "expected ModelBusy, got {result:?}"
        );
        assert_eq!(
            backend.loaded_model(),
            None,
            "a refused load changes nothing"
        );
        release.send(()).unwrap();
        holder.join().unwrap();
    }

    #[test]
    fn dictation_reports_model_busy_and_marks_only_model_acquisition_as_started() {
        let backend = backend_with_active_runtime();
        let (release, holder) = hold_load_lock(&backend);
        let mut timings = DictationTimings::default();

        let result = backend.transcribe_dictation(
            MODEL,
            &[0.0; 8_000],
            Language::English,
            &TranscribeOptions::default(),
            &mut timings,
        );

        assert!(
            matches!(result, Err(DictationError::ModelBusy)),
            "expected ModelBusy, got {result:?}"
        );
        assert!(timings.model_acquisition_started);
        assert!(!timings.inference_started);
        assert!(timings.model_ready_at.is_none());
        assert!(timings.model_ms >= WARM_STATE_GRACE.as_secs_f64() * 1000.0);
        assert_eq!(timings.inference_ms, 0.0);
        release.send(()).unwrap();
        holder.join().unwrap();
    }

    #[test]
    fn with_model_keeps_the_load_lock_for_the_callback_and_releases_it_afterwards() {
        let backend = backend_with_active_runtime();

        let value = backend
            .with_model(MODEL, ContextProfile::FlashAttention, |pinned| {
                assert!(
                    matches!(pinned.load_lock.try_lock(), Err(TryLockError::WouldBlock)),
                    "the model selection stays pinned while the callback runs"
                );
                Ok(42)
            })
            .unwrap();
        assert_eq!(value, 42);
        assert!(
            backend.load_lock.try_lock().is_ok(),
            "released after the callback"
        );

        let failed: Result<(), _> =
            backend.with_model(MODEL, ContextProfile::FlashAttention, |_| {
                Err(DictationError::NoAudioCaptured)
            });
        assert!(matches!(failed, Err(DictationError::NoAudioCaptured)));
        assert!(
            backend.load_lock.try_lock().is_ok(),
            "released after an error"
        );
    }

    #[test]
    fn a_poisoned_load_lock_refuses_every_entry_that_selects_a_model() {
        let backend = backend_with_active_runtime();
        poison_lock(&backend, |backend| &backend.load_lock);
        let mut callback_ran = false;

        let results = [
            backend.ensure_model_with_profile(MODEL, ContextProfile::FlashAttention),
            backend.load_model_with_profile(MODEL, ContextProfile::FlashAttention),
            backend.with_model(MODEL, ContextProfile::FlashAttention, |_| {
                callback_ran = true;
                Ok(())
            }),
        ];

        assert!(
            !callback_ran,
            "no callback may run on an interrupted engine"
        );
        for result in results {
            assert!(
                matches!(&result, Err(DictationError::TranscriptionFailed(message))
                    if message.contains("restart Sagascript")),
                "expected the fail-closed restart error, got {result:?}"
            );
        }
        assert!(backend.load_lock.is_poisoned(), "fail-closed is sticky");
    }

    #[test]
    fn the_active_runtime_answers_the_lookups_without_a_context() {
        let backend = backend_with_active_runtime();

        assert_eq!(backend.loaded_model(), Some(MODEL));
        assert_eq!(
            backend.loaded_context_profile(),
            Some(ContextProfile::FlashAttention)
        );
        assert_eq!(backend.resident_models(), vec![MODEL]);
        assert!(!backend.needs_reload(MODEL));
        assert!(backend.needs_reload_with_profile(MODEL, ContextProfile::TokenAlignment));
        assert!(backend.needs_reload(WhisperModel::BaseEn));
    }

    #[test]
    fn an_empty_backend_has_nothing_loaded_and_everything_needs_a_load() {
        let backend = WhisperBackend::new();

        assert_eq!(backend.loaded_model(), None);
        assert_eq!(backend.loaded_context_profile(), None);
        assert!(backend.resident_models().is_empty());
        assert!(backend.needs_reload(MODEL));
    }

    #[test]
    fn dictation_with_an_active_runtime_marks_the_model_cached_before_it_fails_without_a_context() {
        let backend = backend_with_active_runtime();
        let mut timings = DictationTimings::default();

        let result = backend.transcribe_dictation(
            MODEL,
            &[0.0; 8_000],
            Language::English,
            &TranscribeOptions::default(),
            &mut timings,
        );

        assert!(matches!(result, Err(DictationError::ModelNotLoaded)));
        assert!(timings.model_cached, "the active runtime counts as cached");
        assert!(timings.model_acquisition_started);
        assert!(timings.inference_started);
        assert!(timings.model_ready_at.is_some());
        assert!(timings.model_ms.is_finite() && timings.model_ms >= 0.0);
        assert!(timings.inference_ms.is_finite() && timings.inference_ms >= 0.0);
    }

    #[test]
    fn language_detection_checks_audio_before_the_context() {
        let backend = backend_with_active_runtime();

        assert!(matches!(
            backend.detect_language(&[]),
            Err(DictationError::NoAudioCaptured)
        ));
        assert!(matches!(
            backend.detect_language(&[0.1; 16_000]),
            Err(DictationError::ModelNotLoaded)
        ));
    }

    /// ADR 0005: `loaded_runtime` holds one `Option` that is only replaced
    /// whole, so a poisoned lock is recovered as it is, not cleared.
    #[test]
    fn a_poisoned_runtime_identity_is_recovered_as_it_is() {
        let backend = backend_with_active_runtime();
        poison_lock(&backend, |backend| &backend.loaded_runtime);
        assert!(backend.loaded_runtime.is_poisoned());

        assert_eq!(backend.loaded_model(), Some(MODEL));
        assert!(
            !backend.loaded_runtime.is_poisoned(),
            "reported once, then cleared"
        );
        assert!(!backend.needs_reload(MODEL));
    }

    #[test]
    fn a_poisoned_context_is_recovered_and_still_reports_no_model() {
        let backend = Arc::new(WhisperBackend::new());
        poison_lock(&backend, |backend| &backend.context);

        assert!(matches!(
            backend.detect_language(&[0.1; 16_000]),
            Err(DictationError::ModelNotLoaded)
        ));
        assert!(!backend.context.is_poisoned());
    }

    #[test]
    fn a_poisoned_secondary_runtime_slot_is_recovered_as_empty() {
        let backend = backend_with_active_runtime();
        poison_lock(&backend, |backend| &backend.cached_runtime);

        assert_eq!(backend.resident_models(), vec![MODEL]);
        assert!(!backend.cached_runtime.is_poisoned());
    }
}
