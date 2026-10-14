// Typed Tauri IPC mock for frontend tests (#361).
//
//   const ipc = mockIpc({
//     get_settings: () => settingsFixture,
//     set_language: ({ language }) => (language === "fi" ? ["notice"] : []),
//   });
//   await setLanguage("fi");
//   expect(ipc.calls).toEqual([{ command: "set_language", args: { language: "fi" } }]);
//   ipc.emit("state-changed", "recording");
//
// Handlers are keyed by the command names the frontend invokes, with argument
// and result types taken from the wrappers in src/lib/api.ts and
// src/lib/meeting-reprocessing-api.ts (src/lib/api.test.ts checks every
// wrapper against this map at run time). Every call is recorded in order. A
// command without a handler rejects, and src/test/setup.ts fails the test even
// if the code under test swallowed that rejection.
import { clearMocks, mockIPC } from "@tauri-apps/api/mocks";
import { SERIALIZE_TO_IPC_FN, type InvokeArgs } from "@tauri-apps/api/core";
import type * as api from "../lib/api";
import type * as reprocessing from "../lib/meeting-reprocessing-api";
import type { EventMap, EventName } from "../lib/tauri-events.svelte";

declare global {
  interface Window {
    /** Set by `mockIPC`; the real one is injected by the Tauri webview. */
    __TAURI_INTERNALS__?: { runCallback?: (id: number, data: unknown) => void };
  }
}

type AsyncFunction = (...args: never[]) => Promise<unknown>;
/** The value a wrapper resolves with: what the command returns over IPC. */
type ResultOf<F extends AsyncFunction> = Awaited<ReturnType<F>>;
/** The type of a wrapper's parameter, reused for the IPC argument it forwards. */
type Arg<F extends AsyncFunction, I extends number = 0> = Parameters<F>[I];
type NoArgs = Record<string, never>;
interface Command<F extends AsyncFunction, A = NoArgs> {
  args: A;
  result: ResultOf<F>;
}

/**
 * Every command the frontend invokes through its IPC wrappers, with the
 * argument object it sends and the result it expects.
 *
 * TODO(#347): replace with the CommandMap generated from the Rust command
 * signatures. Until then the argument keys here are hand-written from the
 * wrappers (and pinned by src/lib/api.test.ts); plugin commands such as
 * `plugin:dialog|open` are not covered yet and fail as unmocked.
 */
export interface CommandMap {
  // Dictation state
  get_engine_load_state: Command<typeof api.getEngineLoadState>;
  get_state: Command<typeof api.getState>;
  get_last_error: Command<typeof api.getLastError>;
  get_last_transcription: Command<typeof api.getLastTranscription>;
  start_recording: Command<typeof api.startRecording>;
  start_training_recording: Command<typeof api.startTrainingRecording, { profileId: Arg<typeof api.startTrainingRecording> }>;
  cancel_recording: Command<typeof api.cancelRecording>;
  stop_and_transcribe: Command<typeof api.stopAndTranscribe>;
  stop_and_transcribe_training: Command<typeof api.stopAndTranscribeTraining>;
  transcribe_training_file: Command<
    typeof api.transcribeTrainingFile,
    {
      filePath: Arg<typeof api.transcribeTrainingFile, 0>;
      profileId: Arg<typeof api.transcribeTrainingFile, 1>;
    }
  >;
  suggest_training_glossary: Command<
    typeof api.suggestTrainingGlossary,
    {
      heard: Arg<typeof api.suggestTrainingGlossary, 0>;
      corrected: Arg<typeof api.suggestTrainingGlossary, 1>;
      profileId: Arg<typeof api.suggestTrainingGlossary, 2>;
    }
  >;
  apply_training_glossary: Command<
    typeof api.applyTrainingGlossary,
    {
      heard: Arg<typeof api.applyTrainingGlossary, 0>;
      corrected: Arg<typeof api.applyTrainingGlossary, 1>;
      profileId: Arg<typeof api.applyTrainingGlossary, 2>;
      accepted: Arg<typeof api.applyTrainingGlossary, 3>;
    }
  >;

  // Settings
  get_settings: Command<typeof api.getSettings>;
  set_language: Command<typeof api.setLanguage, { language: Arg<typeof api.setLanguage> }>;
  set_whisper_model: Command<typeof api.setWhisperModel, { model: Arg<typeof api.setWhisperModel> }>;
  set_file_transcription_model: Command<typeof api.setFileTranscriptionModel, { modelId: Arg<typeof api.setFileTranscriptionModel> }>;
  set_auto_select_model: Command<typeof api.setAutoSelectModel, { enabled: Arg<typeof api.setAutoSelectModel> }>;
  set_hotkey_mode: Command<typeof api.setHotkeyMode, { mode: Arg<typeof api.setHotkeyMode> }>;
  set_hotkey: Command<typeof api.setHotkey, { shortcut: Arg<typeof api.setHotkey> }>;
  set_hotkey_profiles: Command<typeof api.setHotkeyProfiles, { profiles: Arg<typeof api.setHotkeyProfiles> }>;
  set_profile_model: Command<
    typeof api.setProfileModel,
    { profileId: Arg<typeof api.setProfileModel, 0>; modelId: Arg<typeof api.setProfileModel, 1> }
  >;
  get_profile_model_info: Command<typeof api.getProfileModelInfo, { profileId: Arg<typeof api.getProfileModelInfo> }>;
  get_active_hotkey_profile: Command<typeof api.getActiveHotkeyProfile>;
  hotkey_status: Command<typeof api.hotkeyStatus>;
  retry_hotkey_registration: Command<typeof api.retryHotkeyRegistration>;
  set_auto_paste: Command<typeof api.setAutoPaste, { enabled: Arg<typeof api.setAutoPaste> }>;
  set_initial_prompt: Command<
    typeof api.setInitialPrompt,
    { prompt: Arg<typeof api.setInitialPrompt, 0>; expectedSource?: Arg<typeof api.setInitialPrompt, 1> }
  >;
  set_profile_glossary: Command<
    typeof api.setProfileGlossary,
    {
      profileId: Arg<typeof api.setProfileGlossary, 0>;
      source: Arg<typeof api.setProfileGlossary, 1>;
      expectedSource?: Arg<typeof api.setProfileGlossary, 2>;
    }
  >;
  set_show_overlay: Command<typeof api.setShowOverlay, { enabled: Arg<typeof api.setShowOverlay> }>;
  set_beam_size: Command<typeof api.setBeamSize, { beamSize: Arg<typeof api.setBeamSize> }>;
  set_temperature_fallback: Command<typeof api.setTemperatureFallback, { enabled: Arg<typeof api.setTemperatureFallback> }>;
  set_engine_prewarm: Command<typeof api.setEnginePrewarm, { mode: Arg<typeof api.setEnginePrewarm> }>;
  set_engine_idle_unload_minutes: Command<typeof api.setEngineIdleUnloadMinutes, { minutes: Arg<typeof api.setEngineIdleUnloadMinutes> }>;
  set_vad_enabled: Command<typeof api.setVadEnabled, { enabled: Arg<typeof api.setVadEnabled> }>;
  set_pianissimo_dictation: Command<typeof api.setPianissimoDictation, { enabled: Arg<typeof api.setPianissimoDictation> }>;
  set_onboarding_completed: Command<typeof api.setOnboardingCompleted>;

  // Models and engine
  engine_status: Command<typeof api.getEngineStatus>;
  get_model_info: Command<typeof api.getModelInfo>;
  get_effective_model_info: Command<typeof api.getEffectiveModelInfo, { language: Arg<typeof api.getEffectiveModelInfo> }>;
  get_auto_file_model_info: Command<typeof api.getAutoFileModelInfo, { language: Arg<typeof api.getAutoFileModelInfo> }>;
  get_dictation_model_info: Command<typeof api.getDictationModelInfo, { language: Arg<typeof api.getDictationModelInfo> }>;
  get_file_model_options: Command<
    typeof api.getFileModelOptions,
    {
      language: Arg<typeof api.getFileModelOptions, 0>;
      include: NonNullable<Arg<typeof api.getFileModelOptions, 1>> | null;
    }
  >;
  get_loaded_model: Command<typeof api.getLoadedModel>;
  is_model_ready: Command<typeof api.isModelReady>;
  is_model_downloaded: Command<typeof api.isModelDownloaded, { whisperModel: Arg<typeof api.isModelDownloaded> }>;
  download_model: Command<typeof api.downloadModel, { whisperModel: Arg<typeof api.downloadModel> }>;
  download_pianissimo_model: Command<typeof api.downloadPianissimoModel>;
  get_build_info: Command<typeof api.getBuildInfo>;
  get_supported_formats: Command<typeof api.getSupportedFormats>;
  get_platform: Command<typeof api.getPlatform>;

  // File transcription
  transcribe_file: Command<
    typeof api.transcribeFile,
    {
      filePath: Arg<typeof api.transcribeFile, 0>;
      prompt: string | null;
      diarize: boolean;
      profileId: string | null;
      runId: string | null;
      autoPaste: false;
    }
  >;
  cancel_file_transcription: Command<typeof api.cancelFileTranscription, { runId: Arg<typeof api.cancelFileTranscription> }>;
  save_transcription_text: Command<
    typeof api.saveTranscriptionText,
    {
      text: Arg<typeof api.saveTranscriptionText, 0>;
      fileName: Arg<typeof api.saveTranscriptionText, 1>;
      directory: Arg<typeof api.saveTranscriptionText, 2>;
    }
  >;
  copy_transcription_text: Command<typeof api.copyTranscriptionText, { text: Arg<typeof api.copyTranscriptionText> }>;

  // Updater recovery
  set_update_result_pending: Command<
    typeof api.setUpdateResultPending,
    {
      resultId: Arg<typeof api.setUpdateResultPending, 0>;
      pending: Arg<typeof api.setUpdateResultPending, 1>;
    }
  >;
  get_update_result_pending: Command<typeof api.getUpdateResultPending, { resultId: Arg<typeof api.getUpdateResultPending> }>;
  acknowledge_update_result: Command<typeof api.acknowledgeUpdateResult, { expectedText: Arg<typeof api.acknowledgeUpdateResult> }>;
  save_update_recovery: Command<typeof api.saveUpdateRecovery, { payload: Arg<typeof api.saveUpdateRecovery> }>;
  load_update_recovery: Command<typeof api.loadUpdateRecovery>;
  clear_update_recovery: Command<typeof api.clearUpdateRecovery>;
  complete_update_preparation: Command<
    typeof api.completeUpdatePreparation,
    {
      nonce: Arg<typeof api.completeUpdatePreparation, 0>;
      error: Arg<typeof api.completeUpdatePreparation, 1>;
    }
  >;

  // Meetings
  begin_meeting_file: Command<
    typeof api.beginMeetingFile,
    {
      filePath: Arg<typeof api.beginMeetingFile, 0>;
      prompt: Arg<typeof api.beginMeetingFile, 1>;
      profileId: Arg<typeof api.beginMeetingFile, 2>;
    }
  >;
  get_meeting_job: Command<typeof api.getMeetingJob, { jobId: Arg<typeof api.getMeetingJob> }>;
  cancel_meeting_job: Command<typeof api.cancelMeetingJob, { jobId: Arg<typeof api.cancelMeetingJob> }>;
  rename_meeting_speaker: Command<
    typeof api.renameMeetingSpeaker,
    {
      transcript: Arg<typeof api.renameMeetingSpeaker, 0>;
      speakerId: Arg<typeof api.renameMeetingSpeaker, 1>;
      label: Arg<typeof api.renameMeetingSpeaker, 2>;
    }
  >;
  merge_meeting_speakers: Command<
    typeof api.mergeMeetingSpeakers,
    {
      transcript: Arg<typeof api.mergeMeetingSpeakers, 0>;
      fromId: Arg<typeof api.mergeMeetingSpeakers, 1>;
      intoId: Arg<typeof api.mergeMeetingSpeakers, 2>;
    }
  >;
  save_meeting_export: Command<
    typeof api.saveMeetingExport,
    { transcript: Arg<typeof api.saveMeetingExport, 0>; format: Arg<typeof api.saveMeetingExport, 1> }
  >;
  create_meeting_review: Command<typeof api.createMeetingReview, { transcript: Arg<typeof api.createMeetingReview> }>;
  apply_meeting_corrections: Command<
    typeof api.applyMeetingCorrections,
    {
      review: Arg<typeof api.applyMeetingCorrections, 0>;
      corrections: Arg<typeof api.applyMeetingCorrections, 1>;
    }
  >;
  undo_meeting_review: Command<
    typeof api.undoMeetingReview,
    {
      review: Arg<typeof api.undoMeetingReview, 0>;
      expectedRevision: Arg<typeof api.undoMeetingReview, 1>;
    }
  >;
  reset_meeting_review: Command<
    typeof api.resetMeetingReview,
    {
      review: Arg<typeof api.resetMeetingReview, 0>;
      expectedRevision: Arg<typeof api.resetMeetingReview, 1>;
    }
  >;
  open_meeting_review: Command<typeof api.openMeetingReview>;
  save_meeting_review: Command<
    typeof api.saveMeetingReview,
    { review: Arg<typeof api.saveMeetingReview, 0>; format: Arg<typeof api.saveMeetingReview, 1> }
  >;
  attach_meeting_audio: Command<typeof api.attachMeetingAudio, { sourceSha256: Arg<typeof api.attachMeetingAudio> }>;
  detach_meeting_audio: Command<typeof api.detachMeetingAudio, { token: Arg<typeof api.detachMeetingAudio> }>;

  // Meeting reprocessing (src/lib/meeting-reprocessing-api.ts)
  plan_meeting_reprocessing: Command<
    typeof reprocessing.planMeetingReprocessing,
    {
      previous: Arg<typeof reprocessing.planMeetingReprocessing, 0>;
      mode: Arg<typeof reprocessing.planMeetingReprocessing, 1>;
      threshold: Arg<typeof reprocessing.planMeetingReprocessing, 2>;
      saveCache: Arg<typeof reprocessing.planMeetingReprocessing, 3>;
      prompt: Arg<typeof reprocessing.planMeetingReprocessing, 4>;
      profileId: Arg<typeof reprocessing.planMeetingReprocessing, 5>;
    }
  >;
  begin_meeting_reprocessing: Command<
    typeof reprocessing.beginMeetingReprocessing,
    {
      filePath: string;
      prompt: Arg<typeof reprocessing.beginMeetingReprocessing, 2>;
      profileId: Arg<typeof reprocessing.beginMeetingReprocessing, 3>;
      request: {
        plan: Arg<typeof reprocessing.beginMeetingReprocessing, 0>["plan"];
        expected_revision: string;
        previous: Arg<typeof reprocessing.beginMeetingReprocessing, 1>;
        cache_path: string | null;
        cache_output: string | null;
      };
    }
  >;
  preview_meeting_proposal: Command<
    typeof reprocessing.previewMeetingProposal,
    { proposal: Arg<typeof reprocessing.previewMeetingProposal> }
  >;
  resolve_meeting_proposal: Command<
    typeof reprocessing.resolveMeetingProposal,
    {
      proposal: Arg<typeof reprocessing.resolveMeetingProposal, 0>;
      expectedRevision: string;
      index: Arg<typeof reprocessing.resolveMeetingProposal, 1>;
      operations: Arg<typeof reprocessing.resolveMeetingProposal, 2>;
    }
  >;
  accept_meeting_proposal: Command<
    typeof reprocessing.acceptMeetingProposal,
    {
      proposal: Arg<typeof reprocessing.acceptMeetingProposal, 0>;
      currentReview: Arg<typeof reprocessing.acceptMeetingProposal, 1>;
    }
  >;
  save_meeting_proposal: Command<typeof reprocessing.saveMeetingProposal, { proposal: Arg<typeof reprocessing.saveMeetingProposal> }>;
  open_meeting_proposal: Command<typeof reprocessing.openMeetingProposal>;

  // Onboarding permissions
  check_accessibility_permission: Command<typeof api.checkAccessibilityPermission>;
  request_accessibility_permission: Command<typeof api.requestAccessibilityPermission>;
  open_accessibility_settings: Command<typeof api.openAccessibilitySettings>;
  microphone_status: Command<typeof api.microphoneStatus>;
  request_microphone_access: Command<typeof api.requestMicrophoneAccess>;
  open_microphone_settings: Command<typeof api.openMicrophoneSettings>;
}

export type CommandName = keyof CommandMap;
export type CommandArgs<C extends CommandName> = CommandMap[C]["args"];
export type CommandResult<C extends CommandName> = CommandMap[C]["result"];
/** Throw a string to mimic a Rust `Err(String)`; the wrapper rejects with it. */
export type CommandHandler<C extends CommandName> = (args: CommandArgs<C>) => CommandResult<C> | Promise<CommandResult<C>>;
export type CommandHandlers = { [C in CommandName]?: CommandHandler<C> };
/** One recorded invocation; narrow on `command` to get typed `args`. */
export type IpcCall = { [C in CommandName]: { command: C; args: CommandArgs<C> } }[CommandName];

// A runtime list of the names; the Record type makes it exhaustive.
const COMMANDS: Record<CommandName, true> = {
  get_engine_load_state: true,
  get_state: true,
  get_last_error: true,
  get_last_transcription: true,
  start_recording: true,
  start_training_recording: true,
  cancel_recording: true,
  stop_and_transcribe: true,
  stop_and_transcribe_training: true,
  transcribe_training_file: true,
  suggest_training_glossary: true,
  apply_training_glossary: true,
  get_settings: true,
  set_language: true,
  set_whisper_model: true,
  set_file_transcription_model: true,
  set_auto_select_model: true,
  set_hotkey_mode: true,
  set_hotkey: true,
  set_hotkey_profiles: true,
  set_profile_model: true,
  get_profile_model_info: true,
  get_active_hotkey_profile: true,
  hotkey_status: true,
  retry_hotkey_registration: true,
  set_auto_paste: true,
  set_initial_prompt: true,
  set_profile_glossary: true,
  set_show_overlay: true,
  set_beam_size: true,
  set_temperature_fallback: true,
  set_engine_prewarm: true,
  set_engine_idle_unload_minutes: true,
  set_vad_enabled: true,
  set_pianissimo_dictation: true,
  set_onboarding_completed: true,
  engine_status: true,
  get_model_info: true,
  get_effective_model_info: true,
  get_auto_file_model_info: true,
  get_dictation_model_info: true,
  get_file_model_options: true,
  get_loaded_model: true,
  is_model_ready: true,
  is_model_downloaded: true,
  download_model: true,
  download_pianissimo_model: true,
  get_build_info: true,
  get_supported_formats: true,
  get_platform: true,
  transcribe_file: true,
  cancel_file_transcription: true,
  save_transcription_text: true,
  copy_transcription_text: true,
  set_update_result_pending: true,
  get_update_result_pending: true,
  acknowledge_update_result: true,
  save_update_recovery: true,
  load_update_recovery: true,
  clear_update_recovery: true,
  complete_update_preparation: true,
  begin_meeting_file: true,
  get_meeting_job: true,
  cancel_meeting_job: true,
  rename_meeting_speaker: true,
  merge_meeting_speakers: true,
  save_meeting_export: true,
  create_meeting_review: true,
  apply_meeting_corrections: true,
  undo_meeting_review: true,
  reset_meeting_review: true,
  open_meeting_review: true,
  save_meeting_review: true,
  attach_meeting_audio: true,
  detach_meeting_audio: true,
  plan_meeting_reprocessing: true,
  begin_meeting_reprocessing: true,
  preview_meeting_proposal: true,
  resolve_meeting_proposal: true,
  accept_meeting_proposal: true,
  save_meeting_proposal: true,
  open_meeting_proposal: true,
  check_accessibility_permission: true,
  request_accessibility_permission: true,
  open_accessibility_settings: true,
  microphone_status: true,
  request_microphone_access: true,
  open_microphone_settings: true,
};

/** Every command name in {@link CommandMap}, in declaration order. */
export const COMMAND_NAMES = Object.keys(COMMANDS) as CommandName[];

export function isCommandName(name: string): name is CommandName {
  return Object.hasOwn(COMMANDS, name);
}

// The event map is app code (src/lib/tauri-events.svelte.ts): the listener helper and
// this mock type their payloads from the same declaration.
export type { EventMap, EventName };

export interface IpcMock {
  /** Every command invoked so far, in order, with a copy of its arguments. */
  readonly calls: readonly IpcCall[];
  /** The arguments of each call to one command, in order. */
  callsTo<C extends CommandName>(command: C): CommandArgs<C>[];
  /** Commands invoked without a handler (each one also failed its caller). */
  readonly unmocked: readonly string[];
  /** Add or replace a handler, e.g. to change a result between steps. */
  handle<C extends CommandName>(command: C, handler: CommandHandler<C>): void;
  /**
   * Deliver an event to the listeners registered with `listen()`, as the
   * backend would. Throws when nothing listens (usually a test that emits
   * before an asynchronous `listen` resolved). Returns the listener count.
   */
  emit<E extends EventName>(event: E, payload: EventMap[E]): number;
  /** How many `listen()` registrations for an event are active. */
  listenerCount(event: EventName): number;
  /**
   * The event of every active registration, sorted (an event listened to
   * twice appears twice), including events the frontend does not map, such as
   * the webview's `tauri://drag-*`. `[]` after a component unmounted shows it
   * unlistened everything.
   */
  listeners(): string[];
}

interface Listener {
  event: string;
  handler: number;
}

interface ActiveMock {
  calls: IpcCall[];
  unmocked: string[];
  handlers: Map<string, (args: unknown) => unknown>;
  listeners: Listener[];
}

let active: ActiveMock | null = null;

// Arguments, results and event payloads cross a JSON boundary in the real app:
// the webview serializes invoke arguments with JSON.stringify and Rust answers
// in JSON. Copy them the same way, so a test's fixtures and recorded calls stay
// independent, an `undefined` field disappears as it does over IPC, and Svelte
// `$state` proxies (which components pass as arguments, and which
// structuredClone rejects) are serialized like plain objects.
function copy<T>(value: T): T {
  return value === undefined ? value : (JSON.parse(JSON.stringify(value, ipcReplacer)) as T);
}

// The replacer of Tauri's injected IPC serializer (tauri 2.10.2,
// scripts/process-ipc-message-fn.js): a Map becomes an object, a Uint8Array or
// ArrayBuffer an array of numbers, and an object with a SERIALIZE_TO_IPC_FN
// method (a Channel, a Resource) serializes itself. Not modelled: Tauri sends
// a top-level ArrayBuffer, ArrayBuffer view or Array payload as raw
// application/octet-stream (scripts/ipc-protocol.js), which the typed wrappers
// never do.
function ipcReplacer(_key: string, value: unknown): unknown {
  if (value instanceof Map) return Object.fromEntries(value.entries());
  if (value instanceof Uint8Array) return Array.from(value);
  if (value instanceof ArrayBuffer) return Array.from(new Uint8Array(value));
  if (typeof value === "object" && value !== null && SERIALIZE_TO_IPC_FN in value) {
    return (value as { [SERIALIZE_TO_IPC_FN]: () => unknown })[SERIALIZE_TO_IPC_FN]();
  }
  return value;
}

function record(state: ActiveMock, command: CommandName, args: unknown): void {
  // The command name and the argument shape belong together: the wrapper
  // that sent them is typed by CommandMap.
  state.calls.push({ command, args: copy(args) } as IpcCall);
}

function dispatch(state: ActiveMock, event: string, payload: unknown): number {
  const run = window.__TAURI_INTERNALS__?.runCallback;
  const listeners = state.listeners.filter((listener) => listener.event === event);
  for (const listener of listeners) {
    run?.(listener.handler, { event, id: listener.handler, payload: copy(payload) });
  }
  return listeners.length;
}

function eventPlugin(state: ActiveMock, command: string, args: InvokeArgs | undefined): unknown {
  const fields = (args ?? {}) as Record<string, unknown>;
  switch (command) {
    case "plugin:event|listen": {
      const handler = Number(fields.handler);
      state.listeners.push({ event: String(fields.event), handler });
      // The event id doubles as the callback id, so `unlisten()` (which
      // unregisters the callback by event id) releases the right one.
      return handler;
    }
    case "plugin:event|unlisten":
      state.listeners = state.listeners.filter((listener) => !(listener.event === fields.event && listener.handler === fields.eventId));
      return null;
    case "plugin:event|emit":
      dispatch(state, String(fields.event), fields.payload);
      return null;
    default:
      throw new Error(`The IPC mock does not support ${command} yet.`);
  }
}

/**
 * Install the IPC mock for the current test. `src/test/setup.ts` removes it
 * after each test and fails the test if an unmocked command was invoked.
 */
export function mockIpc(handlers: CommandHandlers = {}): IpcMock {
  resetIpcMock();
  const state: ActiveMock = { calls: [], unmocked: [], handlers: new Map(), listeners: [] };
  for (const [command, handler] of Object.entries(handlers)) {
    if (handler) state.handlers.set(command, handler as (args: unknown) => unknown);
  }
  active = state;

  mockIPC(async (command, args) => {
    if (command.startsWith("plugin:event|")) return eventPlugin(state, command, args);
    if (isCommandName(command)) record(state, command, args);
    const handler = state.handlers.get(command);
    if (!handler) {
      state.unmocked.push(command);
      throw new Error(
        `IPC command "${command}" has no mock handler. Pass one to mockIpc({ ${command}: … }) ` +
          `or add the command to CommandMap in src/test/ipc.ts.`,
      );
    }
    return copy(await handler(copy(args ?? {})));
  });

  return {
    get calls() {
      return state.calls;
    },
    callsTo<C extends CommandName>(command: C): CommandArgs<C>[] {
      return state.calls.flatMap((call) => (call.command === command ? [call.args as CommandArgs<C>] : []));
    },
    get unmocked() {
      return state.unmocked;
    },
    handle(command, handler) {
      state.handlers.set(command, handler as (args: unknown) => unknown);
    },
    emit(event, payload) {
      const delivered = dispatch(state, event, payload);
      if (delivered === 0) {
        throw new Error(`No listener for "${event}": await the component's listen() before emitting.`);
      }
      return delivered;
    },
    listenerCount(event) {
      return state.listeners.filter((listener) => listener.event === event).length;
    },
    listeners() {
      return state.listeners.map((listener) => listener.event).sort();
    },
  };
}

/**
 * Remove the active mock (called after every test by src/test/setup.ts).
 * Throws when the test invoked a command without a handler.
 */
export function resetIpcMock(): void {
  const state = active;
  active = null;
  if (typeof window !== "undefined") clearMocks();
  if (state && state.unmocked.length > 0) {
    throw new Error(`Unmocked IPC commands were invoked: ${[...new Set(state.unmocked)].join(", ")}`);
  }
}
