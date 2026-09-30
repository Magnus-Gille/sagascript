// Synthetic developer fixture only. Loaded by qa-transcription-tabs.mjs, never by the app.
import { mockIPC, mockWindows, mockConvertFileSrc } from "/node_modules/@tauri-apps/api/mocks.js";
import { emit, TauriEvent } from "/node_modules/@tauri-apps/api/event.js";
mockWindows("main");
mockConvertFileSrc("macos");
const pending = new Map();
const meetings = new Map();
const calls = [];
let active = 0;
const profileModels = { swedish: "auto", english: "base.en" };
let pianissimoDownloaded = false;
let pianissimoSizeMb = 0;
const engineSettings = { engine_prewarm: "on_key_down", engine_idle_unload_minutes: 10 };
let holdPianissimoDownload = false;
let releasePianissimoDownload = null;
let maximum = 0;
let sequence = 0;
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
window.qa = {
  calls,
  prepareUpdate: (nonce) => emit("update-preparing", nonce),
  abortUpdate: () => emit("update-aborted", "Synthetic install failure"),
  dictationResult: (text) => emit("transcription-result", text),
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
  maximum: () => maximum,
};
mockIPC(async (cmd, args = {}) => {
  calls.push({ cmd, args });
  switch (cmd) {
    case "load_update_recovery":
      if (window.qaRecoveryDelayMs) await new Promise((resolve) => setTimeout(resolve, window.qaRecoveryDelayMs));
      if (window.qaRecoveryLoadError) throw new Error("Synthetic recovery read failure");
      return window.qaRecovery ?? null;
    case "save_update_recovery": window.qaRecovery = args.payload; return null;
    case "clear_update_recovery": window.qaRecovery = null; return null;
    case "complete_update_preparation": return null;
    case "get_build_info": return { version: "test", git_hash: "synthetic-qa", build_date: "fixture" };
    case "get_last_transcription":
      if (window.qaLastNativeDelayMs) await new Promise((resolve) => setTimeout(resolve, window.qaLastNativeDelayMs));
      return window.qaLastNativeDictation ?? null;
    case "get_update_result_pending": return window.qaNativePending ?? false;
    case "set_update_result_pending":
      if (args.resultId === "live-dictation") window.qaNativePending = args.pending;
      return null;
    case "acknowledge_update_result":
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
      temperature_fallback: true, vad_enabled: false, ...engineSettings, has_completed_onboarding: true };
    case "get_model_info": return [{ id: "base.en", display_name: "Base English", description: "Fixture",
      size_mb: 0, downloaded: true, active: true }];
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
    case "hotkey_status": return { ok: true, error: null, shortcut: "Control+Shift+Space", shortcuts: [] };
    case "transcribe_file":
      active++; maximum = Math.max(maximum, active);
      return new Promise((resolve, reject) => pending.set(args.filePath, { resolve, reject, runId: args.runId }));
    // Deliberately keep the native result pending: Stop is a request, not a
    // terminal status, and finish() may still produce authoritative success.
    case "cancel_file_transcription": return false; // completion won the Stop race in this scenario
    case "save_transcription_text": return true;
    case "copy_transcription_text": return null;
    case "begin_meeting_file": {
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
      if (task.failPoll) { task.failPoll = false; throw new Error("Synthetic poll failure"); }
      if (task.status !== "running" && !task.released) { active--; task.released = true; }
      return { id: args.jobId, status: task.status, phase: task.status, error: null,
        transcript: task.status === "completed" && !task.reprocessing ? transcript(task.path) : null,
        reprocessing: task.status === "completed" && task.reprocessing ? reprocessingResult(task) : null };
    }
    case "cancel_meeting_job": meetings.get(args.jobId).status = "cancelled"; return true;
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
    case "plugin:dialog|open": return ["/fixtures/picked.wav"];
    default: return null;
  }
}, { shouldMockEvents: true });
await import("/src/main.ts");
