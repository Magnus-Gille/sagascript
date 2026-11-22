pub mod log_events;
pub mod service;

pub use service::{log_global, LoggingService};

/// Log why the speech engine was unloaded (`engine_unloaded`, category
/// "Engine"), so a cold load on the next dictation can be explained from
/// `sagascript.log`. Does nothing when `models` is empty. Idle-timeout
/// unloads happen inside the engine host and are not logged here.
pub fn log_engine_unloaded(reason: &'static str, models: &[&'static str]) {
    if models.is_empty() {
        return;
    }
    log_global(
        "info",
        "Engine",
        log_events::engine::UNLOADED,
        serde_json::json!({ "reason": reason, "models": models }),
    );
}
