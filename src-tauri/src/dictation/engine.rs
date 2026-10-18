use sagascript_core::transcription::{WARM_MODEL_CACHE_BUDGET_MB, WARM_MODEL_CACHE_MAX_MODELS};
use tauri::Manager;
use tracing::{info, warn};

use crate::commands::{self, lock_controller, SharedController, SharedWhisper};
use crate::events::{self, EmitEvent};
use crate::focus_diag;
#[cfg(target_os = "macos")]
use crate::platform;

/// Pre-warm the engine on its own thread. The thread reads the settings file
/// (`machine::load`) when it attaches to the engine, not the caller, which is
/// the hotkey path or a wake callback.
fn warm_in_background(reason: &'static str) {
    sagascript_core::transcription::pianissimo_backend::warm_in_background(
        reason,
        sagascript_core::machine::load,
    );
}

/// Key-down pre-warm: start loading the Pianissimo model while the user is
/// still speaking (also after an idle unload under `on_app_start`). Never
/// blocks the hotkey path: the work runs on its own thread and this only reads
/// settings.
pub(super) fn prewarm_engine_on_key_down(ctrl: &tauri::State<'_, SharedController>) {
    use sagascript_core::transcription::pianissimo_backend::{warm_on, WarmTrigger};
    let c = lock_controller(ctrl);
    let profile_id = c.active_hotkey_profile().map(|profile| profile.id.clone());
    warm_on(
        c.settings(),
        WarmTrigger::KeyDown,
        profile_id.as_deref(),
        warm_in_background,
    );
}

/// Tell the overlay when the engine starts loading, is ready, failed or was
/// cancelled. The last event is cached in core for late-created overlays.
fn forward_engine_load_state(app: &tauri::AppHandle) {
    use sagascript_core::transcription::pianissimo_backend as backend;
    let app = app.clone();
    backend::set_load_listener(move |event, source| {
        let payload = backend::load_state_payload(Some(&(event, source)));
        focus_diag::log(
            "engine_load_state_event",
            serde_json::json!({ "origin": "load_listener" }),
        );
        let _ = app.emit_event(events::event::ENGINE_LOAD_STATE, payload);
    });
}

/// Re-send the current engine load state (the overlay may have just been
/// created or shown and missed earlier events).
pub(super) fn reemit_engine_load_state(app: &tauri::AppHandle) {
    let payload = sagascript_core::transcription::pianissimo_backend::current_load_state_payload();
    focus_diag::log(
        "engine_load_state_event",
        serde_json::json!({ "origin": "reemit_after_overlay" }),
    );
    let _ = app.emit_event(events::event::ENGINE_LOAD_STATE, payload);
}

/// Warm the engine for `trigger` (wake, app start) when settings call for it.
fn prewarm_engine_for(
    app: &tauri::AppHandle,
    trigger: sagascript_core::transcription::pianissimo_backend::WarmTrigger,
) {
    use sagascript_core::transcription::pianissimo_backend::warm_on;
    let ctrl: tauri::State<'_, SharedController> = app.state();
    // Wake callbacks run on the main thread inside an ObjC block: never block
    // on the controller there. Skip this warm when it is busy; a poisoned
    // controller is recovered like everywhere else (ADR 0005), not "busy".
    let c = match ctrl.try_lock() {
        Ok(c) => c,
        Err(std::sync::TryLockError::Poisoned(poisoned)) => {
            sagascript_core::sync::recover_poisoned(&ctrl, commands::CONTROLLER_LOCK, poisoned)
        }
        Err(std::sync::TryLockError::WouldBlock) => {
            tracing::debug!(
                reason = trigger.reason(),
                "pre-warm skipped: controller busy"
            );
            return;
        }
    };
    warm_on(c.settings(), trigger, None, warm_in_background);
}

/// Preload and warm the models and the engine selected by the settings.
pub(crate) fn warm_models_at_startup(app: &tauri::App) {
    let whisper: tauri::State<'_, SharedWhisper> = app.state();
    let whisper = whisper.inner().clone();
    let (warm_plan, vad_enabled) = {
        let ctrl: tauri::State<'_, SharedController> = app.state();
        let c = lock_controller(&ctrl);
        (
            c.settings()
                .warm_model_plan(WARM_MODEL_CACHE_MAX_MODELS, WARM_MODEL_CACHE_BUDGET_MB),
            c.settings().vad_enabled,
        )
    };
    {
        // `engine_prewarm = on_app_start`: load the Pianissimo engine in
        // the background when a dictation profile uses it.
        let handle = app.handle().clone();
        forward_engine_load_state(&handle);
        // Dictation audio a killed earlier run left in the temp dir.
        sagascript_core::transcription::engine_host::pipeline::sweep_stale_pcm_dirs_at_startup();
        prewarm_engine_for(
            &handle,
            sagascript_core::transcription::pianissimo_backend::WarmTrigger::AppStart,
        );
        #[cfg(target_os = "macos")]
        {
            platform::macos::observe_memory_pressure(|| {
                sagascript_core::transcription::pianissimo_backend::unload_if_idle_for_memory_pressure();
            });
            let wake_app = handle.clone();
            platform::macos::observe_system_wake(move || {
                prewarm_engine_for(
                    &wake_app,
                    sagascript_core::transcription::pianissimo_backend::WarmTrigger::Wake,
                );
            });
        }
    }
    std::thread::spawn(move || {
        let Some(&(primary_model, primary_language)) = warm_plan.first() else {
            return;
        };

        if let Err(e) = whisper.ensure_model(primary_model) {
            warn!("Primary model preload skipped: {e}");
            return;
        }
        if let Err(e) = whisper.warmup_model(primary_model, primary_language) {
            warn!("Primary model warmup failed: {e}");
        } else {
            info!(
                "Primary model preloaded and warmed: {}",
                primary_model.display_name()
            );
        }

        for &(model, language) in warm_plan.iter().skip(1) {
            if let Err(e) = whisper.ensure_model(model) {
                warn!("Secondary model preload skipped: {e}");
                continue;
            }
            if let Err(e) = whisper.warmup_model(model, language) {
                warn!("Secondary model warmup failed: {e}");
            } else {
                info!(
                    "Secondary model preloaded and warmed: {}",
                    model.display_name()
                );
            }
        }

        if whisper.loaded_model() != Some(primary_model) {
            if let Err(e) = whisper.ensure_model(primary_model) {
                warn!("Could not restore primary model after warmup: {e}");
            }
        }

        info!(
            "Warm resident models ready: {:?}",
            whisper.resident_models()
        );
    });

    // Startup is verification-only: model repair/download remains
    // tied to an explicit GUI enable action or CLI transcription.
    if vad_enabled {
        let vad_path = sagascript_core::transcription::model::vad_model_path();
        if vad_path.exists() {
            if let Err(e) = sagascript_core::transcription::model::verify_vad_model(&vad_path) {
                warn!(
                    "VAD model startup verification failed; re-enable VAD or use the CLI to repair it: {e}"
                );
            }
        } else {
            warn!(
                "VAD is enabled but its model is missing; re-enable VAD or use the CLI to download it"
            );
        }
    }

    // Model and accelerator assets are downloaded only from an
    // explicit model-selection/download action. Startup never
    // performs a silent CoreML network backfill.
}
