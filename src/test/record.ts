// Fixtures for the recorder window's tests (SAGA-564): payloads shaped as the
// backend sends them (docs/handoffs/saga-564-spec.md).
import type { RecorderEventMap, RecorderEventName } from "../lib/record-events";
import type { RecordingFinished, RecordingFolderInfo, RecordingSessionInfo, RecordingSummary, RecoveryNote } from "../lib/record-types";
import { mockIpc, type CommandHandlers, type EventName, type IpcMock } from "./ipc";
import { settingsValue } from "./settings";

export function sessionInfo(overrides: Partial<RecordingSessionInfo> = {}): RecordingSessionInfo {
  return {
    state: "recording",
    recording_id: "0d2c1f3a-1111-4222-8333-444455556666",
    folder_name: "2026-11-06 10.00 Interview",
    title: "Interview",
    quality: "lossless",
    elapsed_ms: 0,
    level: 0,
    clipped: false,
    gap_count: 0,
    gap_ms: 0,
    stalled: false,
    device_lost: false,
    bytes_written: 0,
    free_disk_bytes: 50_000_000_000,
    warnings: [],
    ended_early: null,
    error_code: null,
    ...overrides,
  };
}

export function summary(overrides: Partial<RecordingSummary> = {}): RecordingSummary {
  return { ...finishedRecording.summary, ...overrides };
}

export const finishedRecording: RecordingFinished = {
  ended_early: null,
  summary: {
    folder_name: "2026-11-06 10.00 Interview",
    recording_id: "0d2c1f3a-1111-4222-8333-444455556666",
    title: "Interview",
    state: "complete",
    recovered: false,
    started_at: "2026-11-06T10:00:00Z",
    quality: "lossless",
    duration_seconds: 62,
    bytes: 5_100_000,
    track_count: 1,
    gap_count: 0,
    gap_seconds: 0,
    ended_reason: null,
    needs_recovery: false,
    playable: true,
  },
};

export function recoveryNote(overrides: Partial<RecoveryNote> = {}): RecoveryNote {
  return {
    folder_name: "2026-11-05 15.30 Lecture",
    title: "Lecture",
    outcome: "recovered",
    frames_kept: 48_000 * 600,
    tail_bytes: 0,
    message: "Kept 10 minutes of audio; the last second was lost.",
    ...overrides,
  };
}

// -- A backend for rendering the recorder window -------------------------------
//
// The recorder commands are not in the generated `CommandMap` yet, so
// `mockIpc`'s typed handlers cannot name them and `ipc.calls` does not record
// them. `recorderBackend` hands the mock untyped handlers and keeps its own
// ordered call log.
// TODO(SAGA-564): use `mockIpc` handlers and `ipc.callsTo` directly once the
// commands are in `CommandMap`.

export type RecorderHandler = (args: Record<string, unknown>) => unknown;

export interface RecorderBackend {
  ipc: IpcMock;
  /** Every recorder command invoked, in order, with its arguments. */
  calls: { command: string; args: Record<string, unknown> }[];
  callsTo(command: string): Record<string, unknown>[];
  /** How many listeners a recorder event has. */
  listenerCount(event: RecorderEventName): number;
  /** Deliver a recorder event to the window. */
  emit<E extends keyof RecorderEventMap>(event: E, payload: RecorderEventMap[E]): void;
}

export const defaultFolder: RecordingFolderInfo = {
  path: "/Users/test/Music/Sagascript",
  exists: true,
  free_bytes: 120_000_000_000,
  warnings: [],
  is_default: true,
};

export function recorderBackend(overrides: Record<string, RecorderHandler> = {}, settings: Record<string, unknown> = {}): RecorderBackend {
  const calls: RecorderBackend["calls"] = [];
  const defaults: Record<string, RecorderHandler> = {
    get_settings: () => ({
      ...settingsValue(),
      recording_quality: "lossless",
      recording_folder_confirmed: true,
      recording_consent_acknowledged: true,
      ...settings,
    }),
    get_platform: () => "macos",
    get_recording_session: () => sessionInfo({ state: "idle" }),
    get_recording_folder_info: () => defaultFolder,
    list_recordings: () => [],
    recover_recordings: () => [],
    start_recording_session: () => sessionInfo({ state: "recording" }),
    stop_recording_session: () => finishedRecording,
    set_recording_quality: () => null,
    set_recording_directory: () => null,
    confirm_recording_folder: () => null,
    acknowledge_recording_consent: () => null,
    reveal_recording: () => null,
    delete_recording: () => null,
    transcribe_recording: () => "Hello from the recording.",
  };
  const handlers: Record<string, RecorderHandler> = { ...defaults, ...overrides };
  const wrapped = Object.fromEntries(
    Object.entries(handlers).map(([command, handler]) => [
      command,
      (args: Record<string, unknown>) => {
        if (command !== "get_settings" && command !== "get_platform") calls.push({ command, args });
        return handler(args);
      },
    ]),
  );
  const ipc = mockIpc(wrapped as unknown as CommandHandlers);
  return {
    ipc,
    calls,
    listenerCount: (event) => ipc.listenerCount(event as unknown as EventName),
    callsTo: (command) => calls.filter((call) => call.command === command).map((call) => call.args),
    emit: (event, payload) => {
      ipc.emit(event as unknown as EventName, payload as never);
    },
  };
}
