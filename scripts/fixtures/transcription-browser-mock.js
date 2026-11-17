// Synthetic developer fixture only. Loaded by qa-transcription-tabs.mjs, never by the app.
import { mockIPC, mockWindows, mockConvertFileSrc } from "/node_modules/@tauri-apps/api/mocks.js";
import { emit, TauriEvent } from "/node_modules/@tauri-apps/api/event.js";
mockWindows("main");
mockConvertFileSrc("macos");
const pending = new Map();
const meetings = new Map();
const calls = [];
function waitForRecoveryGate(kind) {
  const gate = window.qaRecoveryGates?.[kind];
  if (!gate?.blocked) return Promise.resolve();
  gate.started = true;
  return new Promise(resolve => { gate.resolve = resolve; });
}
function waitForQaGate(kind) {
  const gate = window.qaGates?.[kind];
  if (!gate?.blocked) return Promise.resolve();
  gate.started = true;
  return new Promise(resolve => { gate.resolve = resolve; });
}
let active = 0;
const profileModels = { swedish: "auto", english: "base.en" };
let pianissimoDownloaded = false;
let pianissimoSizeMb = 0;
const engineSettings = { engine_prewarm: "on_key_down", engine_idle_unload_minutes: 10 };
let holdPianissimoDownload = false;
let releasePianissimoDownload = null;
let maximum = 0;
let sequence = 0;
let strictPreparationNonce = false;
let activePreparationNonce = null;
const expiredPreparationNonces = new Set();
if (window.qaInitialRecoveryGate) {
  window.qaRecoveryGates ??= {};
  window.qaRecoveryGates[window.qaInitialRecoveryGate] = { blocked: true, started: false, resolve: null };
}
if (window.qaInitialNativeGate) {
  window.qaGates ??= {};
  window.qaGates["last-transcription"] = { blocked: true, started: false, resolve: null };
}
function transcript(path) {
  return { schema_version: 1, source_sha256: path, language: "en", model: "fixture", duration_seconds: 4,
    segments: [{ id: "seg-1", start: 0, end: 4, text: `Meeting ${path}`, speaker: "spk-1" }],
    speakers: [{ id: "spk-1", label: "Speaker 1" }] };
}
function meetingTask(path) {
  return [...meetings.values()].reverse().find(task => task.path === path && !task.released)
    || [...meetings.values()].reverse().find(task => task.path === path);
}
function reprocessingPlan(previous, mode, threshold) {
  return {
    plan: {
      schema_version: 1,
      mode,
      context: {
        source_sha256: previous.original.source_sha256,
        previous_revision: previous.revision,
        transcription_context_sha256: "fixture-transcription-context",
        analysis_context_sha256: "fixture-analysis-context",
        cache_sha256: null,
      },
      threshold,
      required_work: {
        decode_audio: true,
        transcription: true,
        language_detection: true,
        segmentation: true,
        embeddings: true,
        clustering: true,
      },
      revision: `plan-${++sequence}`,
    },
    file_path: previous.original.source_sha256,
    cache_path: null,
    cache_output: null,
  };
}
function reprocessingResult(task) {
  const previous = task.previous;
  const proposal = {
    schema_version: 1,
    previous,
    proposed: transcript(task.path),
    generation: 1,
    resolutions: [],
    revision: `proposal-${++sequence}`,
  };
  return {
    proposal,
    required_work: task.plan.required_work,
    timings: {
      validation_seconds: 0,
      decode_seconds: 0,
      analysis_seconds: 0,
      clustering_seconds: 0,
      full_pipeline_seconds: 0,
      proposal_seconds: 0,
      total_seconds: 0,
    },
  };
}
// Recorder (SAGA-564): the Record window opens at /?window=record. window.qa.recordingSessionChanged(...)
// emits recording-session-changed; window.qa.recorder holds the fixture state.
const recorderSettings = { recording_quality: "lossless", recording_directory: "", recording_channel_pair: [1, 2],
  recording_folder_confirmed: true, recording_consent_acknowledged: true };
const recorderFolder = { path: "/Users/fixture/Music/Sagascript", exists: true, free_bytes: 120_000_000_000, warnings: [], is_default: true };
function recordingSession(overrides = {}) {
  return { state: "idle", recording_id: null, folder_name: null, title: null, quality: null, elapsed_ms: 0, level: 0, clipped: false,
    gap_count: 0, gap_ms: 0, stalled: false, device_lost: false, bytes_written: 0, free_disk_bytes: 120_000_000_000, warnings: [],
    ended_early: null, error_code: null, ...overrides };
}
function recordingSummary(overrides = {}) {
  return { folder_name: "2026-11-05 15.30 Lecture", recording_id: "9b1f7c52-3d0e-4a8b-9c61-0f2a7d4e5b13", title: "Lecture", state: "complete",
    recovered: false, started_at: "2026-11-05T15:30:00Z", quality: "lossless", duration_seconds: 2712, bytes: 222_000_000, track_count: 1,
    gap_count: 0, gap_seconds: 0, ended_reason: null, needs_recovery: false, playable: true, codec: "flac", sample_rate: 48000, channels: 1,
    ...overrides };
}
const recorder = {
  session: recordingSession(),
  recordings: [
    recordingSummary(),
    recordingSummary({ folder_name: "2026-11-04 09.00 Interview", recording_id: "4c8e2a10-77d1-4b3f-a2e6-5d9b0c1f8a34", title: "Interview",
      recovered: true, duration_seconds: 614, bytes: 50_900_000, gap_count: 1, gap_seconds: 0.4, ended_reason: "device_changed" }),
  ],
  recoveryNotes: [],
  folder: recorderFolder,
};
window.qa = {
  calls,
  prepareUpdate: (nonce) => {
    if (strictPreparationNonce) activePreparationNonce = nonce;
    return emit("update-preparing", nonce);
  },
  abortUpdate: () => {
    if (strictPreparationNonce && activePreparationNonce !== null) {
      expiredPreparationNonces.add(activePreparationNonce);
      activePreparationNonce = null;
    }
    return emit("update-aborted", "Synthetic install failure");
  },
  strictPreparationNonces: (enabled) => {
    strictPreparationNonce = Boolean(enabled);
    activePreparationNonce = null;
    expiredPreparationNonces.clear();
  },
  activePreparationNonce: () => activePreparationNonce,
  dictationResult: (text) => emit("transcription-result", text),
  recorder,
  recordingSessionChanged: (overrides = {}) => {
    recorder.session = recordingSession(overrides);
    return emit("recording-session-changed", recorder.session);
  },
  recordingsRecovered: (notes) => {
    recorder.recoveryNotes = notes;
    return emit("recordings-recovered", notes);
  },
  // The `error` event is the typed { code, message, retryable } of a failed command (ADR 0006).
  dictationError: (text, code = "transcription_failed", retryable = false) => emit("error", { code, message: text, retryable }),
  blockRecovery: (kind) => {
    window.qaRecoveryGates ??= {};
    window.qaRecoveryGates[kind] = { blocked: true, started: false, resolve: null };
  },
  releaseRecovery: (kind) => {
    const gate = window.qaRecoveryGates?.[kind];
    if (!gate) return;
    gate.blocked = false;
    gate.resolve?.();
    gate.resolve = null;
  },
  recoveryGateStarted: (kind) => Boolean(window.qaRecoveryGates?.[kind]?.started),
  blockGate: (kind) => {
    window.qaGates ??= {};
    window.qaGates[kind] = { blocked: true, started: false, resolve: null };
  },
  releaseGate: (kind) => {
    const gate = window.qaGates?.[kind];
    if (!gate) return;
    gate.blocked = false;
    gate.resolve?.();
    gate.resolve = null;
  },
  gateStarted: (kind) => Boolean(window.qaGates?.[kind]?.started),
  setSaveResult: (result) => { window.qaSaveResult = result; },
  holdNextPianissimoDownload: () => { holdPianissimoDownload = true; },
  releasePianissimo: () => { releasePianissimoDownload?.(); },
  setPianissimoSize: (mb) => { pianissimoSizeMb = mb; },
  // Simulate an upgraded install whose Swedish profile still uses a retired model.
  pinSwedishProfile: (id) => { profileModels.swedish = id; },
  removePianissimo: () => {
    pianissimoDownloaded = false;
    emit("model-ready", {});
  },
  progress: (runId, phase, percent) => emit("plain-transcription-progress", { runId, phase, percent }),
  drop: (paths) => emit(TauriEvent.DRAG_DROP, { paths, position: { x: 20, y: 20 } }),
  finish: (path, error = null) => {
    const task = pending.get(path);
    if (!task) throw new Error(`No pending file: ${path}`);
    pending.delete(path); active--;
    if (error) task.reject(error); else task.resolve(`Transcript for ${path}`);
  },
  finishMeeting: (path, status = "completed") => {
    const task = meetingTask(path);
    if (!task) throw new Error(`No pending meeting: ${path}`);
    task.status = status;
  },
  failPoll: (path) => {
    const task = meetingTask(path);
    if (!task) throw new Error(`No meeting to fail: ${path}`);
    task.failPoll = true;
  },
  meetingProgress: (path, phase, elapsedMs, phaseElapsedMs) => {
    const task = meetingTask(path);
    if (!task) throw new Error(`No meeting to update: ${path}`);
    Object.assign(task, { phase, elapsedMs, phaseElapsedMs });
  },
  holdPoll: (path) => {
    const task = meetingTask(path);
    task.pollHeld = new Promise(resolve => { task.releasePoll = resolve; });
  },
  releasePoll: (path) => {
    const task = meetingTask(path);
    task.releasePoll?.();
    task.pollHeld = null;
  },
  deferCancellation: (path) => { meetingTask(path).deferCancellation = true; },
  maximum: () => maximum,
};
mockIPC(async (cmd, args = {}) => {
  calls.push({ cmd, args });
  switch (cmd) {
    case "load_update_recovery":
      await waitForRecoveryGate("load");
      if (window.qaRecoveryDelayMs) await new Promise((resolve) => setTimeout(resolve, window.qaRecoveryDelayMs));
      if (window.qaRecoveryLoadError) throw new Error("Synthetic recovery read failure");
      return window.qaRecovery ?? null;
    case "save_update_recovery":
      await waitForRecoveryGate("save");
      window.qaRecovery = args.payload;
      return null;
    case "clear_update_recovery":
      await waitForRecoveryGate("clear");
      window.qaRecovery = null;
      return null;
    case "complete_update_preparation":
      await waitForQaGate(`complete-preparation:${args.nonce}`);
      await waitForQaGate("complete-preparation");
      if (strictPreparationNonce && (args.nonce !== activePreparationNonce || expiredPreparationNonces.has(args.nonce))) {
        throw new Error("Synthetic expired update preparation nonce");
      }
      if (strictPreparationNonce) activePreparationNonce = null;
      return null;
    case "get_build_info": return { version: "test", git_hash: "synthetic-qa", build_date: "fixture" };
    case "get_last_transcription":
      await waitForQaGate("last-transcription");
      if (window.qaLastNativeDelayMs) await new Promise((resolve) => setTimeout(resolve, window.qaLastNativeDelayMs));
      return window.qaLastNativeDictation ?? null;
    case "get_update_result_pending": {
      return window.qaNativePending ?? false;
    }
    case "set_update_result_pending":
      if (args.resultId === "live-dictation") window.qaNativePending = args.pending;
      return null;
    case "acknowledge_update_result":
      await waitForQaGate("acknowledge");
      if (window.qaNativePending && window.qaLastNativeDictation === args.expectedText) {
        window.qaNativePending = false;
        return true;
      }
      return false;
    case "get_settings": return { language: "sv", whisper_model: "kb-whisper-medium", file_transcription_model: "auto", pianissimo_dictation: false, hotkey_mode: "toggle",
      show_overlay: true, auto_paste: false, auto_select_model: true, hotkey: "Control+Shift+Space",
      hotkey_profiles: [
        { id: "swedish", name: "Swedish", language: "sv", shortcut: "Control+Shift+S" },
        { id: "english", name: "English", language: "en", shortcut: "Control+Shift+E" },
      ], profile_models: { ...profileModels }, profile_glossaries: {}, profile_glossary_migrated: true, initial_prompt: "", beam_size: 0,
      temperature_fallback: true, vad_enabled: false, ...engineSettings, has_completed_onboarding: true, ...recorderSettings };
    // Mirrors the backend lineup where Pianissimo is supported: Pianissimo and
    // KB-Whisper Large only. `include` keeps a selected retired model listed.
    case "get_file_model_options": {
      if (args.language === "en") {
        return [{ id: "base.en", display_name: "Base English", description: "Fixture", size_mb: 0, downloaded: true, active: false }];
      }
      const lineup = [
        { id: "pianissimo-sv", display_name: "Pianissimo", description: "Fixture",
          size_mb: pianissimoSizeMb, downloaded: pianissimoDownloaded, active: false },
        { id: "kb-whisper-large", display_name: "KB-Whisper Large", description: "Most accurate, slower",
          size_mb: 1031, downloaded: true, active: false },
      ];
      if (args.include === "kb-whisper-base") {
        lineup.push({ id: "kb-whisper-base", display_name: "KB-Whisper Base", description: "Fixture",
          size_mb: 60, downloaded: true, active: false });
      }
      return lineup;
    }
    case "set_profile_model": profileModels[args.profileId] = args.modelId; return null;
    case "get_profile_model_info": return profileModels[args.profileId] === "pianissimo-sv"
      ? { id: "pianissimo-sv", display_name: "Pianissimo", description: "Fixture", size_mb: pianissimoSizeMb, downloaded: pianissimoDownloaded, active: true }
      : args.profileId === "english"
        ? { id: "base.en", display_name: "Base English", description: "Fixture", size_mb: 0, downloaded: true, active: true }
      : profileModels[args.profileId] === "kb-whisper-base"
        ? { id: "kb-whisper-base", display_name: "KB-Whisper Base", description: "Fixture", size_mb: 60, downloaded: true, active: true }
      : profileModels[args.profileId] === "kb-whisper-large"
        ? { id: "kb-whisper-large", display_name: "KB-Whisper Large", description: "Fixture", size_mb: 1031, downloaded: true, active: true }
      // "auto" resolves to the recommendation: Pianissimo where supported.
      : { id: "pianissimo-sv", display_name: "Pianissimo", description: "Fixture", size_mb: pianissimoSizeMb, downloaded: pianissimoDownloaded, active: true };
    case "download_pianissimo_model": {
      pianissimoDownloaded = true;
      if (holdPianissimoDownload) {
        holdPianissimoDownload = false;
        emit("model-ready", {});
        await new Promise(resolve => { releasePianissimoDownload = resolve; });
        releasePianissimoDownload = null;
      }
      return null;
    }
    case "get_auto_file_model_info":
    case "get_effective_model_info": return args.language === "sv"
      ? { id: "pianissimo-sv", display_name: "Pianissimo", description: "Fixture", size_mb: pianissimoSizeMb, downloaded: pianissimoDownloaded, active: true }
      : { id: "base.en", display_name: "Base English", description: "Fixture", size_mb: 0, downloaded: true, active: true };
    case "engine_status": return { installed: pianissimoDownloaded, supported: true, host_path: "/fixture/sagascript-engine-host",
      host_version: "1.3.2", host_git_sha: "abcdef0123456789", warm: false };
    case "set_engine_prewarm": engineSettings.engine_prewarm = args.mode; return null;
    case "set_engine_idle_unload_minutes": engineSettings.engine_idle_unload_minutes = args.minutes; return null;
    case "get_platform": return "macos";
    case "check_accessibility_permission": return true;
    case "get_supported_formats": return ["wav", "mp3", "m4a"];
    case "hotkey_status": return { ok: true, error: null, shortcut: "Control+Shift+Space", shortcuts: [], settings_watcher: { running: true, recovered_panics: 0 } };
    case "transcribe_file":
      active++; maximum = Math.max(maximum, active);
      return new Promise((resolve, reject) => pending.set(args.filePath, { resolve, reject, runId: args.runId }));
    // Deliberately keep the native result pending: Stop is a request, not a
    // terminal status, and finish() may still produce authoritative success.
    case "cancel_file_transcription": return false; // completion won the Stop race in this scenario
    case "save_transcription_text":
      await waitForQaGate("save-transcription");
      return window.qaSaveResult ?? true;
    case "copy_transcription_text":
      await waitForQaGate("copy-transcription");
      return null;
    case "begin_meeting_file": {
      if (window.qaFailMeetingStart) throw new Error("Finish the current dictation before importing a meeting.");
      active++; maximum = Math.max(maximum, active);
      const id = `meeting-${++sequence}`;
      meetings.set(id, { path: args.filePath, status: "running", released: false });
      return id;
    }
    case "plan_meeting_reprocessing":
      return reprocessingPlan(args.previous, args.mode, args.threshold);
    case "begin_meeting_reprocessing": {
      active++; maximum = Math.max(maximum, active);
      const id = `meeting-${++sequence}`;
      meetings.set(id, {
        path: args.filePath,
        status: "running",
        released: false,
        reprocessing: true,
        previous: args.request.previous,
        plan: args.request.plan,
      });
      return id;
    }
    case "get_meeting_job": {
      const task = meetings.get(args.jobId);
      if (task.pollHeld) await task.pollHeld;
      if (task.failPoll) { task.failPoll = false; throw new Error("Synthetic poll failure"); }
      if (["completed", "cancelled", "failed"].includes(task.status) && !task.released) { active--; task.released = true; }
      return { id: args.jobId, status: task.status, phase: task.phase ?? "preparing", error: null,
        elapsed_ms: task.elapsedMs ?? 0, phase_elapsed_ms: task.phaseElapsedMs ?? 0,
        transcript: task.status === "completed" && !task.reprocessing ? transcript(task.path) : null,
        reprocessing: task.status === "completed" && task.reprocessing ? reprocessingResult(task) : null };
    }
    case "cancel_meeting_job": {
      const task = meetings.get(args.jobId);
      task.status = task.deferCancellation ? "cancelling" : "cancelled";
      return true;
    }
    case "create_meeting_review": return {
      review: { schema_version: 1, original: args.transcript, original_revision: "original", generation: 0,
        batches: [], revision: "revision-0" }, transcript: args.transcript,
    };
    case "preview_meeting_proposal": {
      const proposal = args.proposal;
      return {
        proposal,
        preview: {
          candidate: { ...proposal.previous, original: proposal.proposed, revision: "candidate-revision" },
          steps: [],
        },
        candidate: proposal.proposed,
      };
    }
    case "get_recording_session": return recorder.session;
    case "start_recording_session":
      recorder.session = recordingSession({ state: "recording", recording_id: "7a2d9e64-1b3c-4f58-8e07-c4d2a91b6f30",
        folder_name: "2026-11-06 10.00 Recording", title: args.title ?? "Recording", quality: recorderSettings.recording_quality });
      return recorder.session;
    case "stop_recording_session": {
      const summary = recordingSummary({ folder_name: "2026-11-06 10.00 Recording", recording_id: recorder.session.recording_id ?? "fixture",
        title: recorder.session.title ?? "Recording", started_at: "2026-11-06T10:00:00Z", duration_seconds: Math.round(recorder.session.elapsed_ms / 1000),
        quality: recorderSettings.recording_quality });
      recorder.recordings = [summary, ...recorder.recordings];
      recorder.session = recordingSession({ state: "done" });
      return { summary, ended_early: recorder.session.ended_early };
    }
    case "list_recordings": return recorder.recordings;
    case "delete_recording":
      recorder.recordings = recorder.recordings.filter(recording => recording.folder_name !== args.folderName);
      return null;
    case "reveal_recording": return null;
    case "recover_recordings": return recorder.recoveryNotes;
    case "transcribe_recording": return `Transcript of ${args.folderName}`;
    case "get_recording_folder_info": return recorder.folder;
    case "set_recording_quality": recorderSettings.recording_quality = args.quality; return null;
    case "set_recording_directory":
      recorderSettings.recording_directory = args.path;
      recorder.folder = { ...recorderFolder, path: args.path || recorderFolder.path, is_default: !args.path };
      return null;
    case "set_recording_channel_pair": recorderSettings.recording_channel_pair = [args.first, args.second]; return null;
    case "confirm_recording_folder": recorderSettings.recording_folder_confirmed = true; return null;
    case "acknowledge_recording_consent": recorderSettings.recording_consent_acknowledged = true; return null;
    case "plugin:dialog|open": return args.options?.directory ? "/fixtures/Recordings" : ["/fixtures/picked.wav"];
    default: return null;
  }
}, { shouldMockEvents: true });
await import("/src/main.ts");
