use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, TryLockError};
use std::time::{Duration, Instant};

use tracing::{info, warn};
use whisper_rs::{
    DtwMode, DtwParameters, FullParams, WhisperContext, WhisperContextParameters, WhisperState,
};

use crate::error::DictationError;
use crate::models::WhisperModel;
use crate::sync::{lock_or_recover, recover_poisoned};
use crate::transcription::model;

use super::words::dtw_preset;
use super::{
    CachedWhisperRuntime, ContextProfile, Loaded, RuntimeKey, WhisperBackend, CACHED_RUNTIME_LOCK,
    LOADED_RUNTIME_LOCK, STATE_LOCK, WARM_MODEL_CACHE_BUDGET_MB,
};

#[cfg(any(test, all(target_os = "windows", target_arch = "x86_64")))]
fn require_packaged_x64_cpu(
    baseline: Option<&str>,
    features: [bool; 4],
) -> Result<(), DictationError> {
    match baseline {
        None => Ok(()), // Local source builds retain their own compiler policy.
        Some("avx2") if features.into_iter().all(|supported| supported) => Ok(()),
        Some("avx2") => Err(DictationError::TranscriptionFailed(
            "This Windows x64 build requires AVX2, FMA, F16C and BMI2 CPU support. Use a compatible computer or a source build configured for your CPU.".into(),
        )),
        Some(_) => Err(DictationError::TranscriptionFailed(
            "Unrecognized Windows x64 build CPU policy; install a verified Sagascript build.".into(),
        )),
    }
}

/// The parameters a context profile fixes when its model is loaded.
///
/// Token alignment (DTW) is a whisper.cpp context option, and whisper.cpp is
/// linked into every build, so every build can load either profile. Only the
/// diarization pipeline asks for `TokenAlignment`; the `diarization` feature
/// gates that pipeline and its ONNX dependencies, not this.
fn context_parameters(
    model: WhisperModel,
    profile: ContextProfile,
) -> WhisperContextParameters<'static> {
    let mut ctx_params = WhisperContextParameters::default();
    match profile {
        ContextProfile::FlashAttention => {
            ctx_params.flash_attn(true);
        }
        ContextProfile::TokenAlignment => {
            ctx_params.dtw_parameters(DtwParameters {
                mode: DtwMode::ModelPreset {
                    model_preset: dtw_preset(model),
                },
                ..DtwParameters::default()
            });
        }
    }
    ctx_params
}

/// How long `with_warm_state` waits for the state mutex before giving up with
/// [`DictationError::ModelBusy`] instead of blocking a caller forever.
///
/// A normally-finishing dictation releases the state lock in well under this
/// budget (short utterances transcribe in sub-second to a couple of seconds),
/// so legitimate back-to-back dictations only ever *briefly* wait and never
/// spuriously fail. But it is bounded, so a stuck/aborting inference can no
/// longer wedge every future transcription behind an un-releasable lock — the
/// caller gets a clear "engine busy" error instead of hanging. 3s is the
/// deliberate middle of the 2–5s design range (see WP2b).
const WARM_STATE_GRACE: Duration = Duration::from_secs(3);

/// Poll interval while waiting for the state mutex during the grace budget.
const WARM_STATE_POLL_INTERVAL: Duration = Duration::from_millis(25);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelAvailability {
    Active,
    Cached,
    Missing,
}

fn model_availability(
    active: Option<RuntimeKey>,
    cached: Option<RuntimeKey>,
    desired: RuntimeKey,
) -> ModelAvailability {
    if active == Some(desired) {
        ModelAvailability::Active
    } else if cached == Some(desired) {
        ModelAvailability::Cached
    } else {
        ModelAvailability::Missing
    }
}

fn can_cache_pair(active: RuntimeKey, secondary: RuntimeKey) -> bool {
    active != secondary
        && active
            .model
            .size_mb()
            .saturating_add(secondary.model.size_mb())
            <= WARM_MODEL_CACHE_BUDGET_MB
}

/// Swap the active runtime (and its warm state) with the secondary one when
/// the secondary is the `desired` runtime. Returns `false`, changing nothing,
/// when there is no secondary, it is a different runtime, or nothing is
/// active to put in its place.
fn activate_cached_runtime<C, S>(
    desired: RuntimeKey,
    active: &mut Option<Loaded<C>>,
    active_state: &mut Option<S>,
    cached: &mut Option<CachedWhisperRuntime<C, S>>,
) -> bool {
    let Some(cached_runtime) = cached.take() else {
        return false;
    };
    match active.take() {
        Some(active_runtime) if cached_runtime.runtime.key == desired => {
            let CachedWhisperRuntime { runtime, state } = cached_runtime;
            *cached = Some(CachedWhisperRuntime {
                runtime: active_runtime,
                state: active_state.take(),
            });
            *active = Some(runtime);
            *active_state = state;
            true
        }
        unchanged => {
            *active = unchanged;
            *cached = Some(cached_runtime);
            false
        }
    }
}

/// The three mutexes a runtime swap takes together, in this order (the
/// backend's `load_lock` is held around them by the caller), borrowed so that
/// the swap and the warm-state protocol are generic over their payloads: the
/// backend passes the native `WhisperContext` and `WhisperState`, tests pass
/// fakes.
struct RuntimeSlots<'a, C, S> {
    state: &'a Mutex<Option<S>>,
    loaded_runtime: &'a Mutex<Option<Loaded<C>>>,
    cached_runtime: &'a Mutex<Option<CachedWhisperRuntime<C, S>>>,
}

impl<C, S> RuntimeSlots<'_, C, S> {
    fn active_key(&self) -> Option<RuntimeKey> {
        lock_or_recover(self.loaded_runtime, LOADED_RUNTIME_LOCK)
            .as_ref()
            .map(|loaded| loaded.key)
    }

    fn cached_key(&self) -> Option<RuntimeKey> {
        lock_or_recover(self.cached_runtime, CACHED_RUNTIME_LOCK)
            .as_ref()
            .map(|runtime| runtime.runtime.key)
    }

    fn loaded_model(&self) -> Option<WhisperModel> {
        self.active_key().map(|key| key.model)
    }

    fn loaded_context_profile(&self) -> Option<ContextProfile> {
        self.active_key().map(|key| key.profile)
    }

    /// Models resident in memory, active first.
    fn resident_models(&self) -> Vec<WhisperModel> {
        let active = self.loaded_model();
        let cached = self.cached_key().map(|key| key.model);
        active.into_iter().chain(cached).collect()
    }

    fn availability(&self, desired: RuntimeKey) -> ModelAvailability {
        let active = self.active_key();
        let cached = self.cached_key();
        model_availability(active, cached, desired)
    }

    /// Whether selecting `desired` requires opening model weights.
    fn needs_reload(&self, desired: RuntimeKey) -> bool {
        self.availability(desired) == ModelAvailability::Missing
    }

    /// A handle to the active context, taken under the lock and used after it
    /// is released: `ModelNotLoaded` when nothing is loaded.
    ///
    /// Building a state from a context can take a while (kernel compilation
    /// and GPU buffers). Done under `loaded_runtime`, every lookup of the
    /// active model (`loaded_model`, `needs_reload`, the UI's model query)
    /// would wait for it. The callers that build a state hold the state lock,
    /// which keeps a swap out, so the context cannot change underneath them.
    fn active_context(&self) -> Result<Arc<C>, DictationError> {
        lock_or_recover(self.loaded_runtime, LOADED_RUNTIME_LOCK)
            .as_ref()
            .map(|loaded| Arc::clone(&loaded.ctx))
            .ok_or(DictationError::ModelNotLoaded)
    }

    /// The active model and the context handle, read together.
    fn active_model_and_context(&self) -> Result<(WhisperModel, Arc<C>), DictationError> {
        lock_or_recover(self.loaded_runtime, LOADED_RUNTIME_LOCK)
            .as_ref()
            .map(|loaded| (loaded.key.model, Arc::clone(&loaded.ctx)))
            .ok_or(DictationError::ModelNotLoaded)
    }
}

/// What a runtime swap pushed out of the slots and nothing can use again.
///
/// Dropping a native context or state frees model weights and GPU buffers,
/// which takes long enough to matter. A swap therefore hands these back
/// instead of dropping them under its locks, and the caller drops them once
/// the state, runtime and cache locks are released. Lookups of the active
/// model (`loaded_model`, `needs_reload`, `resident_models`) and callers that
/// take only the state lock (a transcription on the loaded model, as the CLI
/// makes with `transcribe_text` and `detect_language`) are not held up by
/// the teardown.
///
/// What this does not change: a load, and with it the publish, runs under
/// `load_lock`, so everything that goes through that lock (the dictation
/// transaction, `with_model` and `pinned`, `ensure_model*`, `load_model*`)
/// still waits out the teardown and reports `ModelBusy` after the grace
/// budget.
///
/// Memory: for a moment the peak can be the old context and its state plus
/// the new context and its state, because a caller that takes only the state
/// lock can build a state on the new runtime while the old context is still
/// being freed on the loading thread. The bound of two resident contexts
/// still holds.
#[must_use = "drop the displaced payloads only after the locks are released"]
struct Displaced<C, S> {
    /// A warm state that belonged to no runtime.
    orphan_state: Option<S>,
    /// The previous active runtime, when it does not fit the cache budget
    /// next to the new one.
    rejected: Option<CachedWhisperRuntime<C, S>>,
    /// The previous secondary runtime, replaced by the previous active one.
    replaced_secondary: Option<CachedWhisperRuntime<C, S>>,
}

impl<C, S> Displaced<C, S> {
    /// Free the payloads, in the order they were displaced.
    fn release(self) {
        drop(self.orphan_state);
        drop(self.rejected);
        drop(self.replaced_secondary);
    }
}

/// Publish a freshly loaded context as the active runtime.
fn publish_runtime<C, S>(
    slots: &RuntimeSlots<'_, C, S>,
    ctx: C,
    desired_key: RuntimeKey,
    grace: Duration,
) -> Result<(), DictationError> {
    // Publish the new context atomically with respect to warm-state users.
    // When the old/new pair fits the explicit cache budget, move the old
    // context and its reusable warm state into the secondary slot. The next
    // switch can then restore both without touching disk or rebuilding the
    // state. Lock order is state -> loaded_runtime -> cache, matching
    // activate_cached_slots().
    //
    // The state lock is acquired with the same bounded policy as
    // with_warm_state: if a stuck inference pins it past the grace budget,
    // return ModelBusy (dropping the freshly loaded `ctx` cleanly and
    // letting ensure_model release load_lock) instead of blocking a model
    // switch forever behind a wedged transcription.
    //
    // What the swap displaces is released after the locks, see `Displaced`.
    let displaced = {
        let mut state = lock_state_slot_bounded(slots.state, grace)?;
        let mut loaded_runtime = lock_or_recover(slots.loaded_runtime, LOADED_RUNTIME_LOCK);
        let previous = loaded_runtime
            .replace(Loaded {
                key: desired_key,
                ctx: Arc::new(ctx),
            })
            .map(|runtime| CachedWhisperRuntime {
                runtime,
                state: state.take(),
            });
        // A warm state left without a runtime belongs to no context.
        let orphan_state = state.take();

        let mut cached = lock_or_recover(slots.cached_runtime, CACHED_RUNTIME_LOCK);
        let (kept, rejected) = match previous {
            Some(runtime) if can_cache_pair(desired_key, runtime.runtime.key) => {
                (Some(runtime), None)
            }
            other => (None, other),
        };
        let replaced_secondary = std::mem::replace(&mut *cached, kept);
        Displaced {
            orphan_state,
            rejected,
            replaced_secondary,
        }
    };
    displaced.release();
    Ok(())
}

/// Empty the secondary slot, releasing what it held after its lock is let go
/// (see [`Displaced`]).
fn discard_secondary<C, S>(slots: &RuntimeSlots<'_, C, S>) {
    let discarded = lock_or_recover(slots.cached_runtime, CACHED_RUNTIME_LOCK).take();
    drop(discarded);
}

/// Swap the active and secondary runtimes while preserving both warm
/// states. The caller holds load_lock, so cache membership cannot change
/// concurrently with this operation.
fn activate_cached_slots<C, S>(
    slots: &RuntimeSlots<'_, C, S>,
    desired: RuntimeKey,
    grace: Duration,
) -> Result<(), DictationError> {
    let mut state = lock_state_slot_bounded(slots.state, grace)?;
    let mut loaded_runtime = lock_or_recover(slots.loaded_runtime, LOADED_RUNTIME_LOCK);
    let mut cached = lock_or_recover(slots.cached_runtime, CACHED_RUNTIME_LOCK);

    if !activate_cached_runtime(desired, &mut loaded_runtime, &mut state, &mut cached) {
        return Err(DictationError::ModelNotLoaded);
    }

    info!(
        "Activated cached whisper model: {} ({:?})",
        desired.model.display_name(),
        desired.profile
    );
    Ok(())
}

/// Acquire the state mutex, polling `try_lock` on a short interval until the
/// `grace` budget elapses. Returns [`DictationError::ModelBusy`] if the lock
/// is still held when the budget runs out (a previous transcription is stuck
/// or genuinely still running), rather than blocking forever like a plain
/// `lock()` would.
///
/// A mutex poisoned by a panic mid-inference is recovered with its warm
/// state discarded (ADR 0005): the interrupted `WhisperState` may be
/// mid-compute, exactly like an aborted one, and is rebuilt lazily from the
/// active context. The recovery is logged, not masked.
fn lock_state_slot_bounded<S>(
    slot: &Mutex<Option<S>>,
    grace: Duration,
) -> Result<MutexGuard<'_, Option<S>>, DictationError> {
    let deadline = Instant::now() + grace;
    loop {
        match slot.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(TryLockError::Poisoned(poisoned)) => {
                let mut guard = recover_poisoned(slot, STATE_LOCK, poisoned);
                *guard = None;
                return Ok(guard);
            }
            Err(TryLockError::WouldBlock) => {
                if Instant::now() >= deadline {
                    warn!(
                        "Whisper state mutex still held after {:?} grace — returning ModelBusy \
                         (a previous transcription may still be running or stuck)",
                        grace
                    );
                    return Err(DictationError::ModelBusy);
                }
                std::thread::sleep(WARM_STATE_POLL_INTERVAL);
            }
        }
    }
}

impl WhisperBackend {
    fn runtime_slots(&self) -> RuntimeSlots<'_, WhisperContext, WhisperState> {
        RuntimeSlots {
            state: &self.state,
            loaded_runtime: &self.loaded_runtime,
            cached_runtime: &self.cached_runtime,
        }
    }

    /// Signal any in-progress inference to abort at the next whisper.cpp compute
    /// step.
    ///
    /// A *real* abort callback (wired by [`Self::install_abort_callback`]) reads
    /// this flag before every ggml graph compute (the encoder pass and each
    /// decoder step). Setting it makes the current `full()` FFI call return
    /// promptly — whisper.cpp reports error `-6` ("failed to encode") when a
    /// compute is aborted — so the blocking task exits and releases the warm
    /// [`Self::state`] mutex instead of running to completion. This un-wedges the
    /// transcription pipeline on the timeout path (see WP2b).
    pub fn request_abort(&self) {
        warn!(
            "Transcription abort requested — signalling whisper to stop at the next compute step"
        );
        self.abort_flag.store(true, Ordering::SeqCst);
    }

    /// Persistent cancellation: aborts the running inference like
    /// [`Self::request_abort`], and makes every later inference call on this
    /// backend fail immediately with a "Cancelled" error. Never cleared; intended
    /// for one-shot CLI runs that exit after cancelling.
    pub fn request_cancel(&self) {
        self.cancel_sticky.store(true, Ordering::SeqCst);
        self.request_abort();
    }

    pub fn is_cancel_requested(&self) -> bool {
        self.cancel_sticky.load(Ordering::SeqCst)
    }

    pub(super) fn cancelled_error() -> DictationError {
        DictationError::Cancelled
    }

    /// FFI abort callback. whisper.cpp invokes this before each ggml graph
    /// compute (encoder + every decoder step); returning `true` aborts the
    /// computation (`whisper_full` then returns error `-6`). `data` points at the
    /// backend's [`Self::abort_flag`] `AtomicBool`, which lives for the whole
    /// process.
    ///
    /// This raw, correctly-typed trampoline deliberately bypasses whisper-rs
    /// 0.15.1's [`FullParams::set_abort_callback_safe`], which is unsound in that
    /// release: its trampoline is instantiated as `trampoline::<F>` (the caller's
    /// closure type) but the `user_data` it stores is a *double-boxed*
    /// `Box<dyn FnMut() -> bool>`. The closure is therefore invoked through a
    /// mismatched type and returns garbage — in practice `true`, aborting every
    /// inference immediately. That is the historical "error -6 on all models" the
    /// old code worked around by leaving abort disabled. A hand-written trampoline
    /// over an `AtomicBool` avoids the buggy generic entirely.
    unsafe extern "C" fn abort_trampoline(data: *mut c_void) -> bool {
        if data.is_null() {
            return false;
        }
        // Safety: `data` is the address of the `AtomicBool` inside this backend's
        // `abort_flag` Arc, which outlives the `full()` call. We only ever read it
        // (atomically), from whichever thread whisper.cpp runs its compute on.
        let flag = unsafe { &*(data as *const AtomicBool) };
        flag.load(Ordering::Relaxed)
    }

    /// Wire the real abort callback into `params`, pointing it at `abort_flag`.
    /// The flag's lifecycle (clear-when-stale, discard-state-on-abort) is owned
    /// exclusively by the state-lock holder inside [`Self::with_warm_state_grace`]
    /// — see the ownership rules documented there.
    pub(super) fn install_abort_callback(&self, params: &mut FullParams) {
        let flag_ptr = Arc::as_ptr(&self.abort_flag) as *mut c_void;
        // Safety: `flag_ptr` targets the `AtomicBool` owned by `self.abort_flag`,
        // which the backend keeps alive for its entire lifetime (well past the
        // `full()` call these params drive). The trampoline only reads the flag
        // and never touches the whisper context, satisfying the FFI contract.
        unsafe {
            params.set_abort_callback(Some(Self::abort_trampoline));
            params.set_abort_callback_user_data(flag_ptr);
        }
    }

    /// Load a model for the default low-latency transcription profile.
    pub fn load_model(&self, whisper_model: WhisperModel) -> Result<(), DictationError> {
        self.load_model_with_profile(whisper_model, ContextProfile::FlashAttention)
    }

    /// Load a specific model and immutable context profile, replacing the
    /// active runtime. DTW alignment and flash attention require different
    /// whisper contexts and therefore cannot be toggled per transcription.
    pub fn load_model_with_profile(
        &self,
        whisper_model: WhisperModel,
        profile: ContextProfile,
    ) -> Result<(), DictationError> {
        let _load = self.lock_load_bounded()?;
        self.load_model_inner(whisper_model, profile)
    }

    fn load_model_inner(
        &self,
        whisper_model: WhisperModel,
        profile: ContextProfile,
    ) -> Result<(), DictationError> {
        // Check before model-loading FFI, including default context parameters.
        // The candidate build hook pins its native library to this same policy.
        #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
        require_packaged_x64_cpu(
            option_env!("SAGASCRIPT_WINDOWS_X64_BASELINE"),
            [
                std::is_x86_feature_detected!("avx2"),
                std::is_x86_feature_detected!("fma"),
                std::is_x86_feature_detected!("f16c"),
                std::is_x86_feature_detected!("bmi2"),
            ],
        )?;
        let desired_key = RuntimeKey {
            model: whisper_model,
            profile,
        };
        let model_path = model::model_path(whisper_model);

        if !model_path.exists() {
            return Err(DictationError::TranscriptionFailed(format!(
                "Model '{}' not downloaded. Please download it from Settings first.",
                whisper_model.display_name()
            )));
        }

        // Never hand an unverified GGML file to whisper.cpp's native parser.
        // This also performs a one-time compatibility check for files saved by
        // versions released before download integrity was enforced.
        crate::download::verify_file(&model_path, whisper_model.download_integrity())?;
        model::quarantine_unverified_coreml_encoder(whisper_model)?;
        // Fine-tunes have no Core ML encoder by design; whisper.cpp still
        // probes for one and logs an ERROR. Mark exactly that probe as expected.
        if let Some(dirname) = whisper_model.unsupported_coreml_probe_dirname() {
            crate::transcription::native_log::expect_missing_coreml_encoder(&dirname);
        }

        info!(
            "Loading whisper model: {} ({profile:?}) from {}",
            whisper_model.display_name(),
            model_path.display()
        );

        let ctx_params = context_parameters(whisper_model, profile);

        // whisper.cpp's Metal backend registration assumes that macOS returns
        // a GPU device and crashes on a null device before it can return an
        // error. Guard that FFI boundary so headless/restricted processes fail
        // cleanly while normal CLI and GUI execution remain unchanged.
        #[cfg(target_os = "macos")]
        crate::transcription::metal_preflight::ensure_available()?;

        // A third model must never overlap two already-resident runtimes while
        // its weights are being opened. Keep the active runtime available for
        // rollback, but discard the secondary before allocating the new
        // context so transient memory remains bounded to two model contexts.
        discard_secondary(&self.runtime_slots());

        let ctx = WhisperContext::new_with_params(
            model_path.to_str().ok_or_else(|| {
                DictationError::TranscriptionFailed("Invalid model path".to_string())
            })?,
            ctx_params,
        )
        .map_err(|e| DictationError::TranscriptionFailed(format!("Failed to load model: {e}")))?;

        // Publish the new context atomically with respect to warm-state users,
        // moving the previous runtime into the secondary slot when the pair fits
        // the cache budget; see publish_runtime for the lock order.
        publish_runtime(&self.runtime_slots(), ctx, desired_key, WARM_STATE_GRACE)?;

        info!(
            "Model loaded: {} ({profile:?})",
            whisper_model.display_name()
        );
        Ok(())
    }

    /// Get the currently loaded model
    pub fn loaded_model(&self) -> Option<WhisperModel> {
        self.runtime_slots().loaded_model()
    }

    /// Get the immutable context profile of the active runtime.
    pub fn loaded_context_profile(&self) -> Option<ContextProfile> {
        self.runtime_slots().loaded_context_profile()
    }

    /// Models currently resident in memory, active first.
    pub fn resident_models(&self) -> Vec<WhisperModel> {
        self.runtime_slots().resident_models()
    }

    /// A handle to the active context; see [`RuntimeSlots::active_context`].
    ///
    /// Pairing invariant: a state stored in `self.state` must be built from
    /// the context this returns UNDER the state lock, as `with_warm_state_grace`
    /// and the parallel path do. A runtime swap needs that lock, so the state
    /// and its context cannot be separated. A state built from a handle taken
    /// without the state lock could end up next to a different context.
    pub(super) fn active_context(&self) -> Result<Arc<WhisperContext>, DictationError> {
        self.runtime_slots().active_context()
    }

    /// The active model and the end-of-text token id of its context, read
    /// under one lock. The context handle is released again before this
    /// returns: nothing keeps a displaced context alive.
    pub(super) fn active_model_and_eot(&self) -> Result<(WhisperModel, i32), DictationError> {
        let (model, context) = self.runtime_slots().active_model_and_context()?;
        Ok((model, context.token_eot()))
    }

    /// Check whether selecting this model in the default flash-attention
    /// profile requires opening model weights.
    pub fn needs_reload(&self, desired_model: WhisperModel) -> bool {
        self.needs_reload_with_profile(desired_model, ContextProfile::FlashAttention)
    }

    /// Check whether selecting this model/profile runtime requires opening
    /// model weights. A warm cached runtime returns false.
    pub fn needs_reload_with_profile(
        &self,
        desired_model: WhisperModel,
        profile: ContextProfile,
    ) -> bool {
        let desired = RuntimeKey {
            model: desired_model,
            profile,
        };
        self.runtime_slots().needs_reload(desired)
    }

    /// Ensure the correct model is active in the default flash-attention
    /// profile.
    pub fn ensure_model(&self, desired_model: WhisperModel) -> Result<(), DictationError> {
        self.ensure_model_with_profile(desired_model, ContextProfile::FlashAttention)
    }

    /// Ensure the correct model/profile runtime is active. Serialized via
    /// `load_lock` so concurrent callers cannot race a load or cache switch.
    pub fn ensure_model_with_profile(
        &self,
        desired_model: WhisperModel,
        profile: ContextProfile,
    ) -> Result<(), DictationError> {
        let _load = self.lock_load_bounded()?;
        self.ensure_model_inner(desired_model, profile)
    }

    /// Ensure a model/profile is active and run a callback while the model
    /// selection remains pinned. This keeps model selection, context access,
    /// and inference in one transaction for callers that need to use one of
    /// the lower-level transcription entry points.
    ///
    /// The callback must not call `ensure_model*` or `load_model*` on this
    /// backend, because the bounded load lock is held until it returns.
    pub fn with_model<R>(
        &self,
        model: WhisperModel,
        profile: ContextProfile,
        callback: impl FnOnce(&Self) -> Result<R, DictationError>,
    ) -> Result<R, DictationError> {
        self.pinned(
            || self.ensure_model_inner(model, profile),
            || callback(self),
        )
    }

    /// Run `callback` with `load_lock` held, after `select` has made the
    /// wanted runtime active. Split out of [`Self::with_model`] so the pinning
    /// can be tested without a loadable model.
    pub(super) fn pinned<R>(
        &self,
        select: impl FnOnce() -> Result<(), DictationError>,
        callback: impl FnOnce() -> Result<R, DictationError>,
    ) -> Result<R, DictationError> {
        let _load = self.lock_load_bounded()?;
        select()?;
        callback()
    }

    pub(super) fn ensure_model_inner(
        &self,
        desired_model: WhisperModel,
        profile: ContextProfile,
    ) -> Result<(), DictationError> {
        let desired = RuntimeKey {
            model: desired_model,
            profile,
        };
        match self.runtime_slots().availability(desired) {
            ModelAvailability::Active => {}
            ModelAvailability::Cached => {
                self.activate_cached_profile(desired)?;
            }
            ModelAvailability::Missing => {
                info!("Loading model runtime: {desired:?}");
                self.load_model_inner(desired_model, profile)?;
            }
        }
        Ok(())
    }

    pub(super) fn lock_load_bounded(&self) -> Result<MutexGuard<'_, ()>, DictationError> {
        let deadline = Instant::now() + WARM_STATE_GRACE;
        loop {
            match self.load_lock.try_lock() {
                Ok(guard) => return Ok(guard),
                Err(TryLockError::Poisoned(_)) => {
                    // This mutex protects the multi-field runtime transaction,
                    // not merely its unit payload. A panic can leave the active
                    // runtime, the secondary runtime or the warm state partially
                    // updated or poisoned.
                    // Do not recover the guard and reuse potentially invalid
                    // native state, or misreport a permanent fault as contention
                    // (the fail-closed exception in ADR 0005).
                    return Err(DictationError::TranscriptionFailed(
                        "The transcription engine was interrupted and cannot safely resume; restart Sagascript before trying again.".into(),
                    ));
                }
                Err(TryLockError::WouldBlock) if Instant::now() >= deadline => {
                    return Err(DictationError::ModelBusy);
                }
                Err(TryLockError::WouldBlock) => std::thread::sleep(WARM_STATE_POLL_INTERVAL),
            }
        }
    }

    /// Swap the active and secondary runtimes while preserving both warm
    /// states; see [`activate_cached_slots`].
    fn activate_cached_profile(&self, desired: RuntimeKey) -> Result<(), DictationError> {
        activate_cached_slots(&self.runtime_slots(), desired, WARM_STATE_GRACE)
    }

    /// Run a closure with the reusable warm state, creating it lazily and
    /// taking the context handle only briefly to do so. No runtime lock is held
    /// across the closure, so a long-running inference no longer blocks lookups
    /// of the active model or model (re)load. The state lock serializes concurrent transcriptions, which is
    /// required anyway — a single whisper state cannot run two `full()` calls at
    /// once.
    ///
    /// The state lock is acquired with a *bounded* wait ([`WARM_STATE_GRACE`]):
    /// if a previous transcription is stuck holding it past the budget, this
    /// returns [`DictationError::ModelBusy`] instead of blocking the caller
    /// forever. That converts the old "permanent wedge" (a hung/aborting call
    /// pinning the lock forced every future transcription to block) into a clear,
    /// recoverable error.
    pub(super) fn with_warm_state<R>(
        &self,
        f: impl FnOnce(&mut WhisperState) -> Result<R, DictationError>,
    ) -> Result<R, DictationError> {
        self.with_warm_state_grace(WARM_STATE_GRACE, f)
    }

    /// [`Self::with_warm_state`] with an explicit grace budget. Split out so
    /// tests can shrink the budget and exercise the `ModelBusy` path quickly.
    ///
    /// Runs [`Self::with_state_slot`] on this backend's own state mutex, whose
    /// state is created from the loaded context.
    fn with_warm_state_grace<R>(
        &self,
        grace: Duration,
        f: impl FnOnce(&mut WhisperState) -> Result<R, DictationError>,
    ) -> Result<R, DictationError> {
        self.with_state_slot(
            &self.state,
            grace,
            || {
                // Lock order state -> loaded_runtime, matching the runtime
                // swap. The lock is held only to take the context handle.
                let ctx = self.active_context()?;
                ctx.create_state().map_err(|e| {
                    DictationError::TranscriptionFailed(format!(
                        "Failed to create whisper state: {e}"
                    ))
                })
            },
            f,
        )
    }

    /// The warm-state protocol, generic over the state payload `S` so that it
    /// can be exercised without a native `WhisperState`.
    ///
    /// This method is the SOLE owner of the abort-flag lifecycle. Ownership rule:
    /// only the call that holds the state lock may touch `abort_flag`, because a
    /// set flag always belongs to the current lock holder's inference:
    ///
    /// 1. On failing to acquire the lock (`ModelBusy`), NOTHING is touched — a
    ///    pending abort request targets the still-running holder, and clearing it
    ///    here would cancel that abort and re-wedge the pipeline.
    /// 2. Once the lock is acquired, any prior abort request is stale by
    ///    definition (its target released the lock), so it is cleared before the
    ///    closure runs.
    /// 3. After the closure returns — STILL holding the guard — if an abort was
    ///    requested during this call, the warm state is discarded (an aborted
    ///    `full()` leaves whisper mid-compute) and the flag cleared, before any
    ///    other call can acquire the lock. No gap exists in which a new call
    ///    could observe the aborted state or race the flag.
    fn with_state_slot<S, R>(
        &self,
        slot: &Mutex<Option<S>>,
        grace: Duration,
        create_state: impl FnOnce() -> Result<S, DictationError>,
        f: impl FnOnce(&mut S) -> Result<R, DictationError>,
    ) -> Result<R, DictationError> {
        let mut state_guard = lock_state_slot_bounded(slot, grace)?;

        // (2) We own the lock, so we own the flag: clear any stale abort left
        // over from a previous, already-finished inference.
        self.abort_flag.store(false, Ordering::SeqCst);
        if self.is_cancel_requested() {
            // Sticky cancel survives the stale-flag clear above: never start
            // new inference after a cancel.
            return Err(Self::cancelled_error());
        }

        // No Drop guard is needed for the abort cleanup below: if `f` panics,
        // the unwind poisons the state mutex and the next lock_state_slot_bounded
        // recovers it by discarding the warm state (the same cleanup), so a
        // plain post-call check suffices.
        let result = (|| {
            if state_guard.is_none() {
                *state_guard = Some(create_state()?);
            }
            f(state_guard.as_mut().unwrap())
        })();

        // (3) Abort cleanup under the guard: if this call's inference was
        // aborted, drop the warm state and clear the flag before releasing the
        // lock, so the next caller starts from a clean slate.
        if self.abort_flag.swap(false, Ordering::SeqCst) {
            warn!(
                "Inference was aborted — discarding warm whisper state; next transcription will rebuild it"
            );
            *state_guard = None;
        }

        result
    }
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "runtime_context_tests.rs"]
mod context_tests;

#[cfg(test)]
#[path = "runtime_protocol_tests.rs"]
mod protocol_tests;

#[cfg(test)]
#[path = "runtime_release_tests.rs"]
mod release_tests;
