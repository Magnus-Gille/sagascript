// The recorder window's logic, without the DOM or Svelte (SAGA-564): a reducer
// over the backend's `RecordingSessionInfo` and the user's actions, the
// first-use steps that gate the first recording, and the text and numbers the
// window shows. Record.svelte renders from it.
import { isBackendError, toErrorMessage } from "./errors";
import type { RecordingFinished, RecordingQuality, RecordingSessionInfo, RecordingWarning, RecoveryNote } from "./record-types";

export type RecorderPhase = "idle" | "starting" | "recording" | "stopping" | "finished" | "error";

/** The failure the window shows: what a rejected command carries, or a session's `error_code`. */
export interface RecorderError {
  code: string;
  /** English text from the backend, or null when only a code is known. */
  message: string | null;
  retryable: boolean;
}

/** The two things a user confirms once before the first recording (design: consent reminder, folder choice). */
export interface FirstUse {
  /** False until the settings were read: nothing may start before. */
  loaded: boolean;
  folderConfirmed: boolean;
  consentAcknowledged: boolean;
}

export interface RecorderState {
  phase: RecorderPhase;
  firstUse: FirstUse;
  /** The latest session the backend reported (also while idle: the last one). */
  session: RecordingSessionInfo | null;
  /** The recording just stopped, while `phase` is "finished". */
  finished: RecordingFinished | null;
  error: RecorderError | null;
  /**
   * True while this window's own stop command is in flight: its answer
   * (`stop-succeeded` or `stop-failed`) is still to come and carries the
   * result, so a terminal session report does not end "stopping" before it.
   */
  stopPending: boolean;
}

export const initialRecorderState: RecorderState = {
  phase: "idle",
  firstUse: { loaded: false, folderConfirmed: false, consentAcknowledged: false },
  session: null,
  finished: null,
  error: null,
  stopPending: false,
};

export type RecorderAction =
  | { type: "first-use-loaded"; folderConfirmed: boolean; consentAcknowledged: boolean }
  | { type: "folder-confirmed" }
  | { type: "consent-acknowledged" }
  | { type: "start-requested" }
  | { type: "start-succeeded"; info: RecordingSessionInfo }
  | { type: "start-failed"; error: RecorderError }
  | { type: "session"; info: RecordingSessionInfo }
  | { type: "stop-requested" }
  | { type: "stop-succeeded"; finished: RecordingFinished }
  | { type: "stop-failed"; error: RecorderError }
  /** Leave "finished" or "error" for "idle". */
  | { type: "dismiss" };

/** What still has to happen before the first recording. */
export type FirstUseStep = "loading" | "folder" | "consent" | "done";

export function firstUseStep(firstUse: FirstUse): FirstUseStep {
  if (!firstUse.loaded) return "loading";
  if (!firstUse.folderConfirmed) return "folder";
  if (!firstUse.consentAcknowledged) return "consent";
  return "done";
}

/** A new recording may start from these phases, once the first-use steps are done. */
export function canStart(state: RecorderState): boolean {
  return (state.phase === "idle" || state.phase === "finished" || state.phase === "error") && firstUseStep(state.firstUse) === "done";
}

export function canStop(state: RecorderState): boolean {
  return state.phase === "recording";
}

/** True while a session owns the microphone: the quality and the folder cannot change. */
export function isSessionActive(phase: RecorderPhase): boolean {
  return phase === "starting" || phase === "recording" || phase === "stopping";
}

/** The failure of a session that ended in `failed`, or null. */
export function sessionError(info: RecordingSessionInfo): RecorderError | null {
  if (info.state !== "failed") return null;
  return { code: info.error_code ?? "recording_session_failed", message: null, retryable: true };
}

/** A rejected command (or anything thrown) as the error the window shows. */
export function toRecorderError(error: unknown): RecorderError {
  if (isBackendError(error)) return { code: error.code, message: error.message, retryable: error.retryable };
  return { code: "unknown", message: toErrorMessage(error, "") || null, retryable: false };
}

function withSession(state: RecorderState, info: RecordingSessionInfo): RecorderState {
  switch (info.state) {
    case "starting":
      return state.phase === "stopping" ? { ...state, session: info } : { ...state, phase: "starting", session: info, error: null };
    case "recording":
      // Also a session started elsewhere (the tray): the window follows the backend.
      return state.phase === "stopping"
        ? { ...state, session: info }
        : { ...state, phase: "recording", session: info, finished: null, error: null };
    case "finalizing":
      return { ...state, phase: "stopping", session: info };
    case "done":
      return terminal(state, info);
    case "failed":
      return { ...state, phase: "error", session: info, finished: null, error: sessionError(info), stopPending: false };
    case "idle":
      // An idle report is stale while a start command is in flight.
      return state.phase === "recording" || state.phase === "stopping" ? terminal(state, info) : { ...state, session: info };
  }
}

/**
 * A report that the session is over (`done`, or `idle` after it). The result
 * of a stop this window asked for arrives with `stop-succeeded`, so it keeps
 * "stopping" until then. A session that ended by itself (a lost device, a full
 * disk) or was stopped elsewhere has no such answer coming: the window goes
 * back to idle, with nothing to show but the list.
 */
function terminal(state: RecorderState, info: RecordingSessionInfo): RecorderState {
  if (state.phase === "stopping" && state.stopPending) return { ...state, session: info };
  return state.phase === "recording" || state.phase === "starting" || state.phase === "stopping"
    ? { ...state, phase: "idle", session: info }
    : { ...state, session: info };
}

export function reduceRecorder(state: RecorderState, action: RecorderAction): RecorderState {
  switch (action.type) {
    case "first-use-loaded":
      return {
        ...state,
        firstUse: { loaded: true, folderConfirmed: action.folderConfirmed, consentAcknowledged: action.consentAcknowledged },
      };
    case "folder-confirmed":
      return { ...state, firstUse: { ...state.firstUse, folderConfirmed: true } };
    case "consent-acknowledged":
      return { ...state, firstUse: { ...state.firstUse, consentAcknowledged: true } };
    case "start-requested":
      return canStart(state) ? { ...state, phase: "starting", finished: null, error: null, stopPending: false } : state;
    case "start-succeeded":
      // Events may have moved on (a stop pressed at once): only a start still waiting takes the answer.
      return state.phase === "starting" ? withSession(state, action.info) : state;
    case "start-failed":
      return state.phase === "starting" ? { ...state, phase: "error", error: action.error } : state;
    case "session":
      return withSession(state, action.info);
    case "stop-requested":
      return canStop(state) ? { ...state, phase: "stopping", stopPending: true } : state;
    case "stop-succeeded":
      return state.phase === "stopping" || state.phase === "recording"
        ? { ...state, phase: "finished", finished: action.finished, error: null, stopPending: false }
        : state;
    case "stop-failed":
      return { ...state, phase: "error", error: action.error, stopPending: false };
    case "dismiss":
      return state.phase === "finished" || state.phase === "error"
        ? { ...state, phase: "idle", finished: null, error: null, stopPending: false }
        : state;
  }
}

// -- Numbers ------------------------------------------------------------------

/** The quietest level the meter shows (dBFS); everything below is an empty bar. */
export const METER_FLOOR_DB = -60;

/** Peak level (0..1, linear) in dBFS; -Infinity for silence or an invalid value. */
export function levelToDb(peak: number): number {
  if (!Number.isFinite(peak) || peak <= 0) return -Infinity;
  return 20 * Math.log10(Math.min(peak, 1));
}

/** The meter bar, 0..100: -60 dBFS (and below) is 0, full scale is 100. */
export function levelToPercent(peak: number): number {
  const db = levelToDb(peak);
  if (db === -Infinity) return 0;
  return Math.max(0, Math.min(100, Math.round(((db - METER_FLOOR_DB) / -METER_FLOOR_DB) * 100)));
}

/** "-12 dB", or "silent" below the meter's floor. */
export function formatLevelDb(peak: number): string {
  const db = levelToDb(peak);
  if (db < METER_FLOOR_DB) return "silent";
  return `${Math.round(db)} dB`;
}

/** h:mm:ss (hours are always shown), from milliseconds. Invalid or negative values are 0:00:00. */
export function formatElapsed(ms: number): string {
  const total = Number.isFinite(ms) && ms > 0 ? Math.floor(ms / 1000) : 0;
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = total % 60;
  return `${hours}:${String(minutes).padStart(2, "0")}:${String(seconds).padStart(2, "0")}`;
}

/** A duration in seconds for a list ("1:05:09" or "4:02"). */
export function formatDuration(seconds: number): string {
  const text = formatElapsed(seconds * 1000);
  const [hours, minutes, secs] = text.split(":");
  return hours === "0" ? `${Number(minutes)}:${secs}` : text;
}

/** 48 kHz, 24-bit mono as FLAC: about 295 MB per hour per channel. */
export const LOSSLESS_BYTES_PER_HOUR_PER_CHANNEL = 295_000_000;
/** The compact codec: about 46 MB per hour, whatever the channel count. */
export const COMPACT_BYTES_PER_HOUR = 46_000_000;

/** The channel count of a 1-based channel pair: `[n, n]` is mono. */
export function channelCount(pair: readonly [number, number]): 1 | 2 {
  return pair[0] === pair[1] ? 1 : 2;
}

/** Estimated size of an hour of recording. */
export function estimateBytesPerHour(quality: RecordingQuality, channels = 1): number {
  return quality === "lossless" ? LOSSLESS_BYTES_PER_HOUR_PER_CHANNEL * Math.max(1, channels) : COMPACT_BYTES_PER_HOUR;
}

/** "295 MB", "1.2 GB", "340 kB": decimal units, as the file manager shows them. */
export function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "kB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit++;
  }
  const digits = unit === 0 || value >= 100 ? 0 : 1;
  return `${value.toFixed(digits)} ${units[unit]}`;
}

export function formatSizePerHour(quality: RecordingQuality, channels = 1): string {
  return `about ${formatBytes(estimateBytesPerHour(quality, channels))} per hour`;
}

// -- Text ---------------------------------------------------------------------

export interface RecordingErrorText {
  /** What happened. */
  text: string;
  /** What the user can do, or null. */
  hint: string | null;
}

/**
 * The text for a recorder failure, from its code (never from the backend's
 * message, which is only the fallback for a code this window does not know).
 */
export function describeRecordingError(error: Pick<RecorderError, "code"> & Partial<RecorderError>): RecordingErrorText {
  switch (error.code) {
    case "recording_permission_denied":
      return {
        text: "Sagascript is not allowed to use the microphone.",
        hint: "Allow microphone access for Sagascript in your system's privacy settings, then try again.",
      };
    case "recording_source_unavailable":
      return {
        text: "No microphone is available.",
        hint: "Connect or enable a microphone and check the input device in your system settings, then try again.",
      };
    case "recording_disk_full":
      return {
        text: "The disk is full. The recording stopped and what was captured so far is kept.",
        hint: "Free up space or choose another folder, then record again.",
      };
    case "recording_device_changed":
      return {
        text: "The microphone changed or was unplugged. The recording stopped and what was captured so far is kept.",
        hint: "Check the input device, then start a new recording.",
      };
    case "recording_recovery_needed":
      return {
        text: "This recording was interrupted and has to be recovered before it can be used.",
        hint: "Choose Recover to keep what was saved.",
      };
    default:
      return {
        text: error.message?.trim() || "The recording failed.",
        hint: error.retryable ? "Try again." : null,
      };
  }
}

/** Why a session ended before the user stopped it. */
export function endedEarlyText(reason: string | null): string | null {
  switch (reason) {
    case null:
      return null;
    case "device_changed":
      return "The recording ended early because the microphone changed or was unplugged. What was captured is saved.";
    case "format_changed":
      return "The recording ended early because the microphone's audio format changed. What was captured is saved.";
    case "system_sleep":
      return "The recording ended early because the computer went to sleep. What was captured is saved.";
    case "disk_full":
      return "The recording ended early because the disk is full. What was captured is saved.";
    default:
      return "The recording ended early. What was captured is saved.";
  }
}

const WARNING_TEXT: Record<RecordingWarning, string> = {
  low_disk: "Low disk space: the recording may stop when the disk is full.",
  fat32: "This folder is on a FAT32 volume: files larger than 4 GB cannot be saved, so long recordings may stop.",
  exfat: "This folder is on an exFAT volume: it is not crash-safe, so a power loss or unplugged drive can lose a recording.",
  network_volume: "This folder is on a network volume: a dropped connection can interrupt or lose the recording.",
  cloud_synced: "This folder is synced to the cloud: syncing while recording can slow the disk and upload unfinished files.",
};

/** Text for a warning; a code from a newer backend gets a generic line. */
export function warningText(warning: string): string {
  return Object.hasOwn(WARNING_TEXT, warning) ? WARNING_TEXT[warning as RecordingWarning] : "There is a problem with the recording folder.";
}

/** "1 gap in the audio (0.4 s of silence)", "3 gaps in the audio (2.1 s of silence)"; null when there are none. */
export function gapText(count: number, seconds: number): string | null {
  if (count <= 0) return null;
  const noun = count === 1 ? "gap" : "gaps";
  const shown = seconds >= 10 ? Math.round(seconds).toString() : seconds.toFixed(1);
  return `${count} ${noun} in the audio (${shown} s of silence)`;
}

/** What to tell the user about a running session beyond its time and level, most urgent first. */
export function sessionNotices(info: RecordingSessionInfo): string[] {
  const notices: string[] = [];
  if (info.device_lost) notices.push("The microphone was disconnected. Reconnect it.");
  else if (info.stalled) notices.push("No audio is arriving from the microphone.");
  if (info.clipped) notices.push("The input is clipping: lower the microphone gain.");
  const gaps = gapText(info.gap_count, info.gap_ms / 1000);
  if (gaps) notices.push(`${gaps[0].toUpperCase()}${gaps.slice(1)}.`);
  for (const warning of info.warnings) notices.push(warningText(warning));
  return notices;
}

/** The heading over the recovery notes, or null when recovery had nothing to report (everything was complete already). */
export function recoveryHeading(notes: readonly RecoveryNote[]): string | null {
  const recovered = notes.filter((note) => note.outcome === "recovered").length;
  const failed = notes.filter((note) => note.outcome === "failed").length;
  const skipped = notes.filter((note) => note.outcome === "skipped_locked").length;
  const parts: string[] = [];
  if (recovered > 0) parts.push(`Recovered ${recovered} ${recovered === 1 ? "recording" : "recordings"}`);
  if (failed > 0) parts.push(`could not recover ${failed}`);
  if (skipped > 0) parts.push(`skipped ${skipped} in use`);
  if (parts.length === 0) return null;
  const heading = parts.join(", ");
  return heading[0].toUpperCase() + heading.slice(1);
}

/** The notes worth showing: those that did something or went wrong. */
export function noteworthyRecoveries(notes: readonly RecoveryNote[]): RecoveryNote[] {
  return notes.filter((note) => note.outcome !== "already_complete");
}

/** The file manager for a platform id (`get_platform`). */
export function fileManagerName(platform: string): string {
  return platform === "macos" ? "Finder" : platform === "windows" ? "Explorer" : "file manager";
}

/** Where "Move to Trash" puts a recording. */
export function trashName(platform: string): string {
  return platform === "windows" ? "Recycle Bin" : "Trash";
}

/** A recording's start time for a list; the raw text when it is not a date. */
export function formatStartedAt(startedAt: string, locale?: string): string {
  const date = new Date(startedAt);
  if (Number.isNaN(date.getTime())) return startedAt;
  return date.toLocaleString(locale, { dateStyle: "medium", timeStyle: "short" });
}
