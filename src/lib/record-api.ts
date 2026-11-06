// `invoke` wrappers for the recorder commands (SAGA-564).
//
// TODO(SAGA-564): these commands are not in the generated `CommandMap` yet, so
// the wrappers cannot live in the typed api.ts and src/lib/api.test.ts cannot
// check them. Once `SAGASCRIPT_BLESS_CONTRACTS=1 cargo test -p sagascript
// contract` has regenerated src/lib/ipc-contract.generated.ts, move them into
// api.ts, add their rows to api.test.ts and delete this file. Argument keys are
// the camelCase names Tauri derives from the Rust parameters.
import { invoke } from "@tauri-apps/api/core";
import { getSettings } from "./api";
import type {
  RecorderSettingsFields,
  RecordingFinished,
  RecordingFolderInfo,
  RecordingQuality,
  RecordingSessionInfo,
  RecordingSummary,
  RecoveryNote,
} from "./record-types";

/** Refuses while dictation is active or a session runs; checks the microphone permission. */
export async function startRecordingSession(title: string | null = null): Promise<RecordingSessionInfo> {
  return invoke("start_recording_session", { title });
}

export async function stopRecordingSession(): Promise<RecordingFinished> {
  return invoke("stop_recording_session");
}

export async function getRecordingSession(): Promise<RecordingSessionInfo> {
  return invoke("get_recording_session");
}

/** Newest first. */
export async function listRecordings(): Promise<RecordingSummary[]> {
  return invoke("list_recordings");
}

/** Moves the recording folder to the Trash or Recycle Bin. */
export async function deleteRecording(folderName: string): Promise<void> {
  return invoke("delete_recording", { folderName });
}

/** Shows the recording folder in Finder or Explorer. */
export async function revealRecording(folderName: string): Promise<void> {
  return invoke("reveal_recording", { folderName });
}

/** Recovers interrupted recordings and resolves with what was kept of each. */
export async function recoverRecordings(): Promise<RecoveryNote[]> {
  return invoke("recover_recordings");
}

/**
 * Hands the recording to file transcription. Resolves with the text when the
 * backend returns it; the window shows any other answer as "finished".
 */
export async function transcribeRecording(folderName: string): Promise<string | null> {
  return invoke("transcribe_recording", { folderName });
}

/** `path` null asks about the configured folder. */
export async function getRecordingFolderInfo(path: string | null = null): Promise<RecordingFolderInfo> {
  return invoke("get_recording_folder_info", { path });
}

export async function setRecordingQuality(quality: RecordingQuality): Promise<void> {
  return invoke("set_recording_quality", { quality });
}

/** An empty path selects the default folder. */
export async function setRecordingDirectory(path: string): Promise<void> {
  return invoke("set_recording_directory", { path });
}

/** 1-based channels; `[n, n]` records channel n as mono. */
export async function setRecordingChannelPair(first: number, second: number): Promise<void> {
  return invoke("set_recording_channel_pair", { first, second });
}

export async function confirmRecordingFolder(): Promise<void> {
  return invoke("confirm_recording_folder");
}

export async function acknowledgeRecordingConsent(): Promise<void> {
  return invoke("acknowledge_recording_consent");
}

/**
 * The recorder's settings, read from `get_settings`: the quality and the two
 * first-use markers. A backend that does not send a field gets its default (the
 * markers false, so the first-use steps show rather than being skipped).
 *
 * TODO(SAGA-564): the spec gives no dedicated command for these; this assumes
 * Rust serializes the fields into `Settings`. Replace if it does not.
 */
export async function readRecorderSettings(): Promise<
  Required<Pick<RecorderSettingsFields, "recording_quality" | "recording_folder_confirmed" | "recording_consent_acknowledged">>
> {
  // `Settings` (api.ts) does not declare the recorder fields yet.
  const settings = (await getSettings()) as unknown as RecorderSettingsFields;
  return {
    recording_quality: settings.recording_quality === "compact" ? "compact" : "lossless",
    recording_folder_confirmed: settings.recording_folder_confirmed === true,
    recording_consent_acknowledged: settings.recording_consent_acknowledged === true,
  };
}
