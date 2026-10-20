// The frontend's half of the backend → frontend event catalogue (#347).
//
// Every event the frontend listens for, with the payload its listeners read.
// The Rust half is src-tauri/src/events.rs, which also names the producer and
// consumer of each event. Both halves are tested against the shared fixture
// contracts/transport/events.json (src/test/transport.test.ts), so a renamed
// event, field or enum value on either side fails a test. Events that no
// frontend code listens for are `Internal` in the Rust catalogue and are not
// listed here.
import type { EngineLoadState, HotkeyProfile, HotkeyStatus } from "./api";
import type { BackendError } from "./errors";

/**
 * The `state-changed` payload: the dictation state, plus `settings_reloaded`,
 * a legacy signal that is not a state (the settings file was reloaded from
 * disk; Settings re-reads it).
 */
export type UiState = "idle" | "recording" | "loading_model" | "transcribing" | "error" | "settings_reloaded";

/** Plain file-transcription phases; src/lib/transcribe-stages.ts maps them to stages. */
export type PlainTranscriptionPhase = "decoding" | "resampling" | "loading" | "preparing" | "encoding" | "transcribing" | "finalizing";

export interface PlainTranscriptionProgress {
  /** The run id the frontend passed to `transcribe_file`; listeners ignore other runs. */
  runId: string;
  phase: PlainTranscriptionPhase;
  /** 0–100 while decoding, resampling and transcribing; `null` in the other phases. */
  percent: number | null;
}

/** The backend also sends `model`, `downloaded` and `total`; nothing reads them. */
export interface ModelDownloadProgress {
  /** Whole percent; 0 while the total is unknown. */
  progress: number;
}

/** Each event the frontend listens for, by name, with its payload. */
export interface EventMap {
  "state-changed": UiState;
  "transcription-result": string;
  /** A dictation error: what a failed command rejects with (ADR 0006), so the window decides by `code` and shows `message`. */
  error: BackendError;
  "model-download-progress": ModelDownloadProgress;
  "model-ready": null;
  "plain-transcription-progress": PlainTranscriptionProgress;
  "hotkey-registration-changed": HotkeyStatus;
  "active-hotkey-profile-changed": HotkeyProfile;
  "engine-load-state": EngineLoadState;
  /** Sent to Settings only: the preparation nonce to pass to `complete_update_preparation`. */
  "update-preparing": string;
  /** Sent to Settings only: the error message (Settings needs only the signal). */
  "update-aborted": string;
  /** A Settings tab (`dictate`, `transcribe`). The snake_case name is a named legacy exception. */
  navigate_tab: string;
}

export type EventName = keyof EventMap;
