// The recorder's IPC types (SAGA-564, docs/recorder-design.md §4.5-4.7): what the
// recording commands return and the `recording-session-changed` and
// `recordings-recovered` events carry. They mirror the Rust payloads in
// src-tauri (see docs/handoffs/saga-564-spec.md until that file is deleted).
//
// TODO(SAGA-564): once the Rust side regenerates `CommandMap`, move the wrappers
// of record-api.ts into api.ts and add the two events to `EventMap`
// (src/lib/events.ts) and `eventPayloads` (src/test/transport-schemas.ts, where
// the schemas of these types already exist).

export type RecordingSessionState = "idle" | "starting" | "recording" | "finalizing" | "done" | "failed";

export type RecordingQuality = "lossless" | "compact";

/** A condition of the recording folder or the disk, shown as a warning (never blocks recording). */
export type RecordingWarning = "low_disk" | "fat32" | "exfat" | "network_volume" | "cloud_synced";

/** Why a session ended before the user stopped it (`ended_early`, `ended_reason`). */
export type RecordingEndedEarly = "device_changed" | "format_changed" | "system_sleep" | "disk_full";

/** The live state of the recording session (`get_recording_session`, `recording-session-changed`). */
export interface RecordingSessionInfo {
  state: RecordingSessionState;
  recording_id: string | null;
  folder_name: string | null;
  title: string | null;
  quality: RecordingQuality | null;
  elapsed_ms: number;
  /** Peak level 0..1, the maximum over the channels. */
  level: number;
  clipped: boolean;
  gap_count: number;
  gap_ms: number;
  /** Audio stopped arriving from the device. */
  stalled: boolean;
  device_lost: boolean;
  bytes_written: number;
  free_disk_bytes: number | null;
  /** Folder and disk conditions, as `RecordingWarning` values (a newer backend may send more). */
  warnings: string[];
  /** A `RecordingEndedEarly` value (a newer backend may send more). */
  ended_early: string | null;
  error_code: string | null;
}

/** One recording folder (`list_recordings`). The backend also sends `codec`, `sample_rate` and `channels`; nothing reads them. */
export interface RecordingSummary {
  folder_name: string;
  recording_id: string;
  title: string;
  state: "recording" | "complete";
  recovered: boolean;
  started_at: string;
  quality: RecordingQuality;
  duration_seconds: number;
  bytes: number;
  track_count: number;
  gap_count: number;
  gap_seconds: number;
  ended_reason: string | null;
  needs_recovery: boolean;
  /** A completed recording whose mic track can be played. */
  playable: boolean;
}

/** What `stop_recording_session` resolves with. */
export interface RecordingFinished {
  summary: RecordingSummary;
  ended_early: string | null;
}

export type RecoveryOutcome = "recovered" | "already_complete" | "skipped_locked" | "failed";

/** What recovery did to one recording (`recover_recordings`, `recordings-recovered`). */
export interface RecoveryNote {
  folder_name: string;
  title: string;
  outcome: RecoveryOutcome;
  frames_kept: number;
  tail_bytes: number;
  message: string;
}

/** The recording folder and what is known about its volume (`get_recording_folder_info`). */
export interface RecordingFolderInfo {
  path: string;
  exists: boolean;
  free_bytes: number | null;
  /** `RecordingWarning` values without `low_disk`. */
  warnings: string[];
  is_default: boolean;
}

/**
 * The error codes the recorder window branches on (ADR 0006). Any other code
 * gets the generic text of `describeRecordingError`.
 */
export const RECORDING_ERROR_CODES = [
  "recording_permission_denied",
  "recording_source_unavailable",
  "recording_disk_full",
  "recording_device_changed",
  "recording_recovery_needed",
] as const;

export type RecordingErrorCode = (typeof RECORDING_ERROR_CODES)[number];

/** The recorder settings the window reads from `get_settings` (fields Rust adds for SAGA-564). */
export interface RecorderSettingsFields {
  recording_quality?: RecordingQuality;
  recording_directory?: string;
  recording_channel_pair?: [number, number];
  recording_folder_confirmed?: boolean;
  recording_consent_acknowledged?: boolean;
}
