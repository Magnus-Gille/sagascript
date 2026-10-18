//! Backend → frontend events: the catalogue (#347).
//!
//! Every event the app emits is declared once in [`event`], with its wire
//! name, its payload type, who produces it and who listens. Emit one with
//! [`EmitEvent::emit_event`] (or [`EmitEvent::emit_event_to`] for one window):
//! the compiler then checks the payload type at every emit site. Calling
//! `tauri::Emitter::emit*` directly is a clippy error (`disallowed-methods` in
//! src-tauri/clippy.toml), so a raw event name cannot reach the wire.
//!
//! The frontend's half of the catalogue is `EventMap` in src/lib/events.ts.
//! Both halves are tested against the shared fixture
//! contracts/transport/events.json, which the tests below write from this
//! catalogue and real sample payloads (see contracts/README.md).

use serde::{Deserialize, Serialize};
use tauri::{Emitter, Runtime};

/// One backend → frontend event.
pub trait Event: Copy {
    /// The name the frontend `listen()`s for.
    const NAME: &'static str;
    /// What every emit site sends.
    type Payload<'a>: Serialize + Clone;
}

/// Emits catalogue events: `app.emit_event(event::STATE_CHANGED, UiState::Idle)`.
pub trait EmitEvent<R: Runtime>: Emitter<R> {
    /// Broadcasts to every webview (Settings, overlay, onboarding).
    // The sanctioned call: the name comes from the catalogue.
    #[allow(clippy::disallowed_methods)]
    fn emit_event<E: Event>(&self, _event: E, payload: E::Payload<'_>) -> tauri::Result<()> {
        self.emit(E::NAME, payload)
    }

    /// Sends to the webview window with this label only (`"settings"`).
    // The macOS signed updater is its only caller (updater/signed.rs).
    #[cfg(target_os = "macos")]
    #[allow(clippy::disallowed_methods)]
    fn emit_event_to<E: Event>(
        &self,
        window: &str,
        _event: E,
        payload: E::Payload<'_>,
    ) -> tauri::Result<()> {
        self.emit_to(window, E::NAME, payload)
    }
}

impl<R: Runtime, T: Emitter<R>> EmitEvent<R> for T {}

/// The `state-changed` payload: the dictation state that the overlay, the
/// tray and the Dictate tab show, plus one legacy signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UiState {
    Idle,
    Recording,
    LoadingModel,
    Transcribing,
    /// The overlay shows the error briefly; `idle` follows.
    Error,
    /// Not a dictation state: the settings watcher reloaded the settings file
    /// and Settings re-reads it. A named legacy exception (#347); a new
    /// concern gets its own event instead.
    SettingsReloaded,
}

/// The phase vocabulary of plain file transcription
/// ([`PlainTranscriptionProgress`]) and of the legacy `transcription-phase`
/// event. The frontend maps phases to stages in src/lib/transcribe-stages.ts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlainPhase {
    /// Measured: `percent` 0–100.
    Decoding,
    /// Measured: `percent` 0–100.
    Resampling,
    Loading,
    Preparing,
    Encoding,
    /// Measured: `percent` 0–100.
    Transcribing,
    Finalizing,
}

/// The `plain-transcription-progress` payload: progress of one plain
/// (non-meeting) file transcription. Scoped by `runId`, the run id the
/// frontend passed to `transcribe_file`; listeners ignore other runs.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlainTranscriptionProgress<'a> {
    pub run_id: &'a str,
    pub phase: PlainPhase,
    /// 0–100 in the measured phases, `null` in the others.
    pub percent: Option<u8>,
}

/// The `model-download-progress` payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelDownloadProgress {
    /// The model being downloaded. Legacy quirk (#347): Whisper downloads send
    /// the Rust variant name (`KbWhisperLarge`), Pianissimo its model id
    /// (`pianissimo-sv`). The frontend reads only `progress`.
    pub model: String,
    pub downloaded: u64,
    pub total: u64,
    /// Whole percent; 0 while the total is unknown.
    pub progress: u32,
}

impl ModelDownloadProgress {
    pub fn new(model: String, downloaded: u64, total: u64) -> Self {
        let progress = if total > 0 {
            (downloaded as f64 / total as f64 * 100.0) as u32
        } else {
            0
        };
        Self {
            model,
            downloaded,
            total,
            progress,
        }
    }
}

/// Who listens for an event.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Consumer {
    /// Webview code listens; src/lib/events.ts types the payload.
    Gui,
    /// No frontend code listens, for the stated reason: a named legacy
    /// exception (#347). Such an event needs no GUI listener and is not part
    /// of the frontend's `EventMap`.
    Internal(&'static str),
}

/// One catalogue row, for the contract tests.
#[cfg(test)]
#[derive(Debug, Clone, Copy)]
pub struct EventInfo {
    pub name: &'static str,
    pub consumer: Consumer,
    /// The payload field that scopes the event to one job or run, if any.
    pub scope: Option<&'static str>,
}

/// Declares the catalogue: one marker type and constant per event, and
/// `ALL` (tests only), so no event can be missing from the checks.
macro_rules! catalogue {
    ($(
        $(#[$meta:meta])*
        $constant:ident: $marker:ident = $name:literal,
        payload<$lt:lifetime> = $payload:ty,
        consumer = $consumer:expr,
        scope = $scope:expr;
    )+) => {
        $(
            $(#[$meta])*
            #[derive(Debug, Clone, Copy)]
            pub struct $marker;

            impl $crate::events::Event for $marker {
                const NAME: &'static str = $name;
                type Payload<$lt> = $payload;
            }

            $(#[$meta])*
            pub const $constant: $marker = $marker;
        )+

        /// Every event, in declaration order.
        #[cfg(test)]
        pub const ALL: &[$crate::events::EventInfo] = &[$(
            $crate::events::EventInfo { name: $name, consumer: $consumer, scope: $scope },
        )+];
    };
}

/// The events. Names are kebab-case; `navigate_tab` is the one named
/// exception (see `KEBAB_CASE_EXCEPTIONS` in the tests).
pub mod event {
    #[cfg(test)]
    use super::Consumer;
    use super::{ModelDownloadProgress, PlainPhase, PlainTranscriptionProgress, UiState};
    use crate::hotkey::HotkeyStatus;
    use sagascript_core::settings::HotkeyProfile;

    catalogue! {
        /// Dictation state changed. Producers: the hotkey pipeline (dictation/pipeline.rs),
        /// the Dictate test recording, training and file transcription
        /// (commands/), meeting jobs, and the settings watcher
        /// ([`UiState::SettingsReloaded`]). Consumers: Overlay.svelte and the
        /// Dictate tab (Settings.svelte).
        STATE_CHANGED: StateChanged = "state-changed",
            payload<'a> = UiState,
            consumer = Consumer::Gui,
            scope = None;

        /// A live dictation's text, also when pasting failed. Producer: the
        /// hotkey pipeline. Consumer: Settings.svelte (Dictate tab).
        TRANSCRIPTION_RESULT: TranscriptionResult = "transcription-result",
            payload<'a> = &'a str,
            consumer = Consumer::Gui,
            scope = None;

        /// A user-facing dictation error message. Producer: the hotkey
        /// pipeline. Consumer: Settings.svelte.
        ERROR: Error = "error",
            payload<'a> = &'a str,
            consumer = Consumer::Gui,
            scope = None;

        /// Model download progress. Producers: `download_model`,
        /// `download_pianissimo_model`. Consumers: Settings.svelte,
        /// Onboarding.svelte.
        MODEL_DOWNLOAD_PROGRESS: ModelDownloadProgressEvent = "model-download-progress",
            payload<'a> = ModelDownloadProgress,
            consumer = Consumer::Gui,
            scope = None;

        /// A model download finished. Payload `null`. Producers and consumers
        /// as for `model-download-progress`.
        MODEL_READY: ModelReady = "model-ready",
            payload<'a> = (),
            consumer = Consumer::Gui,
            scope = None;

        /// Whisper inference percent of a training transcription or of the
        /// legacy diarized `transcribe_file` path.
        TRANSCRIPTION_PROGRESS: TranscriptionProgress = "transcription-progress",
            payload<'a> = i32,
            consumer = Consumer::Internal(
                "legacy: training and the legacy diarized transcribe_file path still \
                 report it; plain imports use plain-transcription-progress and meetings \
                 are polled"
            ),
            scope = None;

        /// Plain file-transcription progress, scoped by run id. Producer:
        /// plain_file_jobs.rs. Consumer: FileTranscription.svelte through
        /// `acceptRunProgress` (src/lib/transcribe-stages.ts).
        PLAIN_TRANSCRIPTION_PROGRESS: PlainTranscriptionProgressEvent = "plain-transcription-progress",
            payload<'a> = PlainTranscriptionProgress<'a>,
            consumer = Consumer::Gui,
            scope = Some("runId");

        /// Silent pre-inference phase of the legacy diarized `transcribe_file`
        /// path (#237).
        TRANSCRIPTION_PHASE: TranscriptionPhase = "transcription-phase",
            payload<'a> = PlainPhase,
            consumer = Consumer::Internal(
                "legacy: only the legacy diarized transcribe_file path sends it; the GUI \
                 imports meetings through the meeting-job API"
            ),
            scope = None;

        /// Hotkey registration health changed. Producers: hotkey changes
        /// (commands/hotkey.rs) and the settings watcher. Consumer: Settings.svelte.
        HOTKEY_REGISTRATION_CHANGED: HotkeyRegistrationChanged = "hotkey-registration-changed",
            payload<'a> = &'a HotkeyStatus,
            consumer = Consumer::Gui,
            scope = None;

        /// The profile whose shortcut started the recording. Producer: the
        /// hotkey pipeline. Consumer: Overlay.svelte.
        ACTIVE_HOTKEY_PROFILE_CHANGED: ActiveHotkeyProfileChanged = "active-hotkey-profile-changed",
            payload<'a> = &'a HotkeyProfile,
            consumer = Consumer::Gui,
            scope = None;

        /// Pianissimo engine load lifecycle, built by
        /// `pianissimo_backend::load_state_payload`: `{ state, source,
        /// loadMs?, message? }`. Producer: dictation/engine.rs. Consumer: Overlay.svelte.
        ENGINE_LOAD_STATE: EngineLoadState = "engine-load-state",
            payload<'a> = serde_json::Value,
            consumer = Consumer::Gui,
            scope = None;

        /// Sent to the Settings window only: save recovery drafts, then call
        /// `complete_update_preparation` with this nonce. Producer: the macOS
        /// signed updater (updater/signed.rs). Consumer: Settings.svelte.
        // Built only by the macOS updater; the catalogue (and the IPC contract) lists it on every platform.
        #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
        UPDATE_PREPARING: UpdatePreparing = "update-preparing",
            payload<'a> = &'a str,
            consumer = Consumer::Gui,
            scope = None;

        /// Sent to the Settings window only: the update stopped after
        /// preparation; the payload is the error message. Producer and
        /// consumer as for `update-preparing`.
        // Same as `update-preparing`: built only by the macOS updater.
        #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
        UPDATE_ABORTED: UpdateAborted = "update-aborted",
            payload<'a> = &'a str,
            consumer = Consumer::Gui,
            scope = None;

        /// Switch the open Settings window to a tab (`dictate`, `transcribe`).
        /// Producer: the tray menu, through launch/window.rs. Consumer: Settings.svelte.
        NAVIGATE_TAB: NavigateTab = "navigate_tab",
            payload<'a> = &'a str,
            consumer = Consumer::Gui,
            scope = None;
    }
}

#[cfg(test)]
#[path = "events_tests.rs"]
mod tests;
