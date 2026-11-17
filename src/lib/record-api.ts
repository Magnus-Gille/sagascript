// `invoke` wrappers for the recorder commands (SAGA-564). Argument keys are the
// camelCase names Tauri derives from the Rust parameters; src/lib/api.test.ts
// checks each wrapper against the generated `CommandMap`.
import { invoke } from "@tauri-apps/api/core";
import { getSettings, type Settings } from "./api";
import type {
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
 * first-use markers. The markers are false until the user has answered, so the
 * first-use steps show rather than being skipped.
 */
export async function readRecorderSettings(): Promise<
  Pick<Settings, "recording_quality" | "recording_folder_confirmed" | "recording_consent_acknowledged">
> {
  const settings = await getSettings();
  return {
    recording_quality: settings.recording_quality === "compact" ? "compact" : "lossless",
    recording_folder_confirmed: settings.recording_folder_confirmed === true,
    recording_consent_acknowledged: settings.recording_consent_acknowledged === true,
  };
}
