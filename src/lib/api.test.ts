// The IPC contract of the frontend's command wrappers: which command each
// wrapper invokes and with which argument object. CommandMap is generated from
// the Rust command signatures (src/lib/ipc-contract.generated.ts, #347), so a
// command or argument renamed on either side fails here: the `args` below are
// type-checked against Rust, and each wrapper must send exactly them.
import { describe, expect, test } from "vitest";
import * as api from "./api";
import * as reprocessing from "./meeting-reprocessing-api";
import { PIANISSIMO_ID } from "./models.svelte";
import { COMMAND_NAMES, mockIpc, type CommandArgs, type CommandHandlers, type CommandName, type CommandResult } from "../test/ipc";
import type { CorrectionFile, MeetingReview, MeetingTranscript } from "./meeting-types";
import type { MeetingReprocessingProposal, SelectedReprocessingPlan } from "./meeting-reprocessing-types";
import type { UpdateRecoveryPayload } from "./update-recovery";

const transcript: MeetingTranscript = {
  schema_version: 1,
  source_sha256: "source",
  language: "sv",
  model: PIANISSIMO_ID,
  duration_seconds: 2,
  segments: [{ id: "seg-1", start: 0, end: 2, text: "Hej", speaker: "s1" }],
  speakers: [{ id: "s1", label: "Talare 1" }],
};
const review: MeetingReview = {
  schema_version: 1,
  original: transcript,
  original_revision: "original-1",
  generation: 1,
  batches: [],
  revision: "review-1",
};
const corrections: CorrectionFile = {
  schema_version: 1,
  source_sha256: "source",
  original_revision: "original-1",
  expected_revision: "review-1",
  operations: [{ kind: "rename_speaker", speaker_id: "s1", label: "Alice" }],
};
const selectedPlan: SelectedReprocessingPlan = {
  plan: {
    schema_version: 1,
    mode: "recluster",
    context: {
      source_sha256: "source",
      previous_revision: "review-1",
      transcription_context_sha256: "transcription",
      analysis_context_sha256: "analysis",
      cache_sha256: null,
    },
    threshold: 0.5,
    required_work: {
      decode_audio: false,
      transcription: false,
      language_detection: false,
      segmentation: false,
      embeddings: false,
      clustering: true,
    },
    revision: "plan-1",
  },
  file_path: "/audio/meeting.wav",
  cache_path: "/cache/meeting.bin",
  cache_output: null,
};
const proposal: MeetingReprocessingProposal = {
  schema_version: 1,
  previous: review,
  proposed: transcript,
  generation: 2,
  resolutions: [],
  revision: "proposal-1",
};
const recoveryPayload: UpdateRecoveryPayload = {
  schema_version: 1,
  saved_at: "2026-10-12T10:00:00.000Z",
  dictation: { text: "unsaved" },
  files: [],
  meetings: [],
};
const profile: api.HotkeyProfile = { id: "default", name: "Default", shortcut: "Super+S", language: "sv" };

interface Contract {
  name: string;
  /** Identity only: which exported wrapper this row covers. */
  wrapper: object;
  call: () => Promise<unknown>;
  command: CommandName;
  args: unknown;
}

/**
 * `wrapper(...params)` must invoke exactly `command` with `args` (checked at
 * run time). The wrapper must also resolve with the command's CommandMap
 * result type: a CommandMap row that names the wrong wrapper fails
 * type-checking when the two wrappers' result types differ (e.g. for
 * `engine_status`, whose wrapper is `getEngineStatus`). Wrappers with the same
 * result type, such as the many `Promise<void>` setters, are told apart only
 * by the run-time command and argument check.
 */
function contract<P extends unknown[], C extends CommandName>(
  wrapper: (...args: P) => Promise<CommandResult<C>>,
  params: P,
  command: C,
  args: CommandArgs<C>,
): Contract {
  return { name: wrapper.name, wrapper, call: () => wrapper(...params), command, args };
}

const contracts: Contract[] = [
  contract(api.getEngineLoadState, [], "get_engine_load_state", {}),
  contract(api.getState, [], "get_state", {}),
  contract(api.getLastError, [], "get_last_error", {}),
  contract(api.getLastTranscription, [], "get_last_transcription", {}),
  contract(api.getSettings, [], "get_settings", {}),
  contract(api.setLanguage, ["fi"], "set_language", { language: "fi" }),
  contract(api.setFileTranscriptionModel, ["auto"], "set_file_transcription_model", { modelId: "auto" }),
  contract(api.getFileModelOptions, ["sv"], "get_file_model_options", { language: "sv", include: null }),
  contract(api.getFileModelOptions, ["sv", "kb-whisper-medium"], "get_file_model_options", {
    language: "sv",
    include: "kb-whisper-medium",
  }),
  contract(api.downloadPianissimoModel, [], "download_pianissimo_model", {}),
  contract(api.setHotkeyProfiles, [[profile]], "set_hotkey_profiles", { profiles: [profile] }),
  contract(api.setProfileModel, ["default", "auto"], "set_profile_model", { profileId: "default", modelId: "auto" }),
  contract(api.getProfileModelInfo, ["default"], "get_profile_model_info", { profileId: "default" }),
  contract(api.getActiveHotkeyProfile, [], "get_active_hotkey_profile", {}),
  contract(api.hotkeyStatus, [], "hotkey_status", {}),
  contract(api.retryHotkeyRegistration, [], "retry_hotkey_registration", {}),
  contract(api.setAutoPaste, [true], "set_auto_paste", { enabled: true }),
  contract(api.setProfileGlossary, ["default", "a = b", "old"], "set_profile_glossary", {
    profileId: "default",
    source: "a = b",
    expectedSource: "old",
  }),
  contract(api.setShowOverlay, [false], "set_show_overlay", { enabled: false }),
  contract(api.setBeamSize, [5], "set_beam_size", { beamSize: 5 }),
  contract(api.setTemperatureFallback, [true], "set_temperature_fallback", { enabled: true }),
  contract(api.setEnginePrewarm, ["on_app_start"], "set_engine_prewarm", { mode: "on_app_start" }),
  contract(api.setEngineIdleUnloadMinutes, [30], "set_engine_idle_unload_minutes", { minutes: 30 }),
  contract(api.getEngineStatus, [], "engine_status", {}),
  contract(api.setVadEnabled, [false], "set_vad_enabled", { enabled: false }),
  contract(api.getEffectiveModelInfo, ["no"], "get_effective_model_info", { language: "no" }),
  contract(api.getAutoFileModelInfo, ["en"], "get_auto_file_model_info", { language: "en" }),
  contract(api.downloadModel, ["base"], "download_model", { whisperModel: "base" }),
  contract(api.getBuildInfo, [], "get_build_info", {}),
  contract(api.transcribeFile, ["/audio/a.wav"], "transcribe_file", {
    filePath: "/audio/a.wav",
    prompt: null,
    diarize: false,
    profileId: null,
    runId: null,
    autoPaste: false,
  }),
  contract(
    api.transcribeFile,
    ["/audio/a.wav", { prompt: "Astrid", diarize: true, profileId: "default", runId: "run-1" }],
    "transcribe_file",
    { filePath: "/audio/a.wav", prompt: "Astrid", diarize: true, profileId: "default", runId: "run-1", autoPaste: false },
  ),
  contract(api.cancelFileTranscription, ["run-1"], "cancel_file_transcription", { runId: "run-1" }),
  contract(api.saveTranscriptionText, ["text", "a.txt", "/audio"], "save_transcription_text", {
    text: "text",
    fileName: "a.txt",
    directory: "/audio",
  }),
  contract(api.copyTranscriptionText, ["text"], "copy_transcription_text", { text: "text" }),
  contract(api.setUpdateResultPending, ["live-dictation", true], "set_update_result_pending", {
    resultId: "live-dictation",
    pending: true,
  }),
  contract(api.getUpdateResultPending, ["live-dictation"], "get_update_result_pending", {
    resultId: "live-dictation",
  }),
  contract(api.acknowledgeUpdateResult, ["text"], "acknowledge_update_result", { expectedText: "text" }),
  contract(api.saveUpdateRecovery, [recoveryPayload], "save_update_recovery", { payload: recoveryPayload }),
  contract(api.loadUpdateRecovery, [], "load_update_recovery", {}),
  contract(api.clearUpdateRecovery, [], "clear_update_recovery", {}),
  contract(api.completeUpdatePreparation, ["nonce-1", null], "complete_update_preparation", {
    nonce: "nonce-1",
    error: null,
  }),
  contract(api.beginMeetingFile, ["/audio/m.wav", null, "default"], "begin_meeting_file", {
    filePath: "/audio/m.wav",
    prompt: null,
    profileId: "default",
  }),
  contract(api.getMeetingJob, ["job-1"], "get_meeting_job", { jobId: "job-1" }),
  contract(api.cancelMeetingJob, ["job-1"], "cancel_meeting_job", { jobId: "job-1" }),
  contract(api.createMeetingReview, [transcript], "create_meeting_review", { transcript }),
  contract(api.applyMeetingCorrections, [review, corrections], "apply_meeting_corrections", { review, corrections }),
  contract(api.undoMeetingReview, [review, "review-1"], "undo_meeting_review", {
    review,
    expectedRevision: "review-1",
  }),
  contract(api.resetMeetingReview, [review, "review-1"], "reset_meeting_review", {
    review,
    expectedRevision: "review-1",
  }),
  contract(api.openMeetingReview, [], "open_meeting_review", {}),
  contract(api.saveMeetingReview, [review, "json"], "save_meeting_review", { review, format: "json" }),
  contract(api.attachMeetingAudio, ["sha"], "attach_meeting_audio", { sourceSha256: "sha" }),
  contract(api.detachMeetingAudio, ["token"], "detach_meeting_audio", { token: "token" }),
  contract(api.getSupportedFormats, [], "get_supported_formats", {}),
  contract(api.startRecording, [], "start_recording", {}),
  contract(api.stopAndTranscribe, [], "stop_and_transcribe", {}),
  contract(api.checkAccessibilityPermission, [], "check_accessibility_permission", {}),
  contract(api.requestAccessibilityPermission, [], "request_accessibility_permission", {}),
  contract(api.openAccessibilitySettings, [], "open_accessibility_settings", {}),
  contract(api.microphoneStatus, [], "microphone_status", {}),
  contract(api.requestMicrophoneAccess, [], "request_microphone_access", {}),
  contract(api.openMicrophoneSettings, [], "open_microphone_settings", {}),
  contract(api.getPlatform, [], "get_platform", {}),
  contract(api.setOnboardingCompleted, [], "set_onboarding_completed", {}),
  contract(api.liveMeetingSupported, [], "live_meeting_supported", {}),
  contract(api.beginLiveMeeting, [], "begin_live_meeting", {}),
  contract(api.getLiveMeeting, [], "get_live_meeting", {}),
  contract(api.stopLiveMeeting, ["session-1"], "stop_live_meeting", { sessionId: "session-1" }),
  contract(api.discardLiveMeeting, ["session-1"], "discard_live_meeting", { sessionId: "session-1" }),
  contract(api.saveLiveMeetingText, ["session-1", "markdown"], "save_live_meeting_text", {
    sessionId: "session-1",
    format: "markdown",
  }),
  contract(api.saveLiveMeetingAudio, ["session-1"], "save_live_meeting_audio", { sessionId: "session-1" }),
  contract(reprocessing.planMeetingReprocessing, [review, "recluster", 0.5, true, "names", "default"], "plan_meeting_reprocessing", {
    previous: review,
    mode: "recluster",
    threshold: 0.5,
    saveCache: true,
    prompt: "names",
    profileId: "default",
  }),
  contract(reprocessing.beginMeetingReprocessing, [selectedPlan, review, null, null], "begin_meeting_reprocessing", {
    filePath: "/audio/meeting.wav",
    prompt: null,
    profileId: null,
    request: {
      plan: selectedPlan.plan,
      expected_revision: "plan-1",
      previous: review,
      cache_path: "/cache/meeting.bin",
      cache_output: null,
    },
  }),
  contract(reprocessing.previewMeetingProposal, [proposal], "preview_meeting_proposal", { proposal }),
  contract(reprocessing.resolveMeetingProposal, [proposal, 0, corrections.operations], "resolve_meeting_proposal", {
    proposal,
    expectedRevision: "proposal-1",
    index: 0,
    operations: corrections.operations,
  }),
  contract(reprocessing.acceptMeetingProposal, [proposal, review], "accept_meeting_proposal", {
    proposal,
    currentReview: review,
  }),
  contract(reprocessing.saveMeetingProposal, [proposal], "save_meeting_proposal", { proposal }),
  contract(reprocessing.openMeetingProposal, [], "open_meeting_proposal", {}),
];

/** Every command answers with a marker so the test can follow its result. */
function recordAll() {
  const handlers: Partial<Record<CommandName, () => { resultOf: string }>> = {};
  for (const name of COMMAND_NAMES) handlers[name] = () => ({ resultOf: name });
  // The wrappers only forward results; their types do not matter here.
  return mockIpc(handlers as CommandHandlers);
}

describe("each wrapper invokes its command with the documented arguments", () => {
  test.each(contracts)("$name → $command", async ({ call, command, args }) => {
    const ipc = recordAll();
    expect(await call()).toEqual({ resultOf: command });
    expect(ipc.calls).toStrictEqual([{ command, args }]);
  });
});

test("every exported wrapper has a contract", () => {
  const exported = [...Object.values(api), ...Object.values(reprocessing)].filter((value) => typeof value === "function");
  const covered = new Set(contracts.map((entry) => entry.wrapper));
  expect(exported.filter((wrapper) => !covered.has(wrapper)).map((wrapper) => wrapper.name)).toEqual([]);
});

/**
 * Registered commands the frontend never calls, classified on a69d5d1 (#347).
 * They keep their Rust implementation (and CLI equivalents) until removing
 * them is decided; a wrapper is added only with a caller.
 */
const COMMANDS_WITHOUT_A_CALLER: Partial<Record<CommandName, string>> = {
  // Settings now set per profile (set_hotkey_profiles, set_profile_model,
  // set_profile_glossary); `sagascript config set` still covers these keys.
  set_whisper_model: "superseded by per-profile models",
  set_auto_select_model: "superseded by per-profile models",
  set_pianissimo_dictation: "superseded by per-profile models",
  set_hotkey_mode: "superseded by per-profile push-to-talk and toggle shortcuts",
  set_hotkey: "superseded by set_hotkey_profiles",
  set_initial_prompt: "superseded by set_profile_glossary",
  // Model queries no view shows.
  get_model_info: "no view lists every model",
  get_loaded_model: "no view shows the loaded model",
  is_model_ready: "always false: nothing sets the controller's model_ready",
  is_model_downloaded: "views read ModelInfo.downloaded",
  get_dictation_model_info: "views use get_profile_model_info",
  // Meeting edits before the review document existed.
  rename_meeting_speaker: "superseded by apply_meeting_corrections",
  merge_meeting_speakers: "superseded by apply_meeting_corrections",
  save_meeting_export: "superseded by save_meeting_review",
  // The dictionary training flow has no view.
  start_training_recording: "training flow without a view",
  stop_and_transcribe_training: "training flow without a view",
  transcribe_training_file: "training flow without a view",
  suggest_training_glossary: "training flow without a view",
  apply_training_glossary: "training flow without a view",
  cancel_recording: "no view cancels a test or training recording",
};

test("every registered command has a wrapper, or is named as having no caller", () => {
  const invoked = new Set(contracts.map((entry) => entry.command));
  const uncalled = Object.keys(COMMANDS_WITHOUT_A_CALLER).sort();
  expect(COMMAND_NAMES.filter((name) => !invoked.has(name))).toEqual(uncalled);
});
