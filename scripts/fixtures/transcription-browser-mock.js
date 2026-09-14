// Synthetic developer fixture only. Loaded by qa-transcription-tabs.mjs, never by the app.
import { mockIPC, mockWindows, mockConvertFileSrc } from "/node_modules/@tauri-apps/api/mocks.js";
import { emit, TauriEvent } from "/node_modules/@tauri-apps/api/event.js";
mockWindows("main");
mockConvertFileSrc("macos");
const pending = new Map();
const meetings = new Map();
const calls = [];
let active = 0;
let maximum = 0;
let sequence = 0;
function transcript(path) {
  return { schema_version: 1, source_sha256: path, language: "en", model: "fixture", duration_seconds: 4,
    segments: [{ id: "seg-1", start: 0, end: 4, text: `Meeting ${path}`, speaker: "spk-1" }],
    speakers: [{ id: "spk-1", label: "Speaker 1" }] };
}
window.qa = {
  calls,
  progress: (runId, phase, percent) => emit("plain-transcription-progress", { runId, phase, percent }),
  drop: (paths) => emit(TauriEvent.DRAG_DROP, { paths, position: { x: 20, y: 20 } }),
  finish: (path, error = null) => {
    const task = pending.get(path);
    if (!task) throw new Error(`No pending file: ${path}`);
    pending.delete(path); active--;
    if (error) task.reject(error); else task.resolve(`Transcript for ${path}`);
  },
  finishMeeting: (path, status = "completed") => {
    const task = [...meetings.values()].find(task => task.path === path);
    if (!task) throw new Error(`No pending meeting: ${path}`);
    task.status = status;
  },
  failPoll: (path) => { [...meetings.values()].find(task => task.path === path).failPoll = true; },
  maximum: () => maximum,
};
mockIPC(async (cmd, args = {}) => {
  calls.push({ cmd, args });
  switch (cmd) {
    case "get_build_info": return { version: "test", git_hash: "synthetic-qa", build_date: "fixture" };
    case "get_settings": return { language: "en", whisper_model: "base.en", hotkey_mode: "toggle",
      show_overlay: true, auto_paste: false, auto_select_model: true, hotkey: "Control+Shift+Space",
      hotkey_profiles: [], initial_prompt: "", profile_glossaries: {}, beam_size: 0,
      temperature_fallback: true, vad_enabled: false, has_completed_onboarding: true };
    case "get_model_info": return [{ id: "base.en", display_name: "Base English", description: "Fixture",
      size_mb: 0, downloaded: true, active: true }];
    case "get_platform": return "macos";
    case "check_accessibility_permission": return true;
    case "get_supported_formats": return ["wav", "mp3", "m4a"];
    case "hotkey_status": return { ok: true, error: null, shortcut: "Control+Shift+Space", shortcuts: [] };
    case "transcribe_file":
      active++; maximum = Math.max(maximum, active);
      return new Promise((resolve, reject) => pending.set(args.filePath, { resolve, reject, runId: args.runId }));
    // Deliberately keep the native result pending: Stop is a request, not a
    // terminal status, and finish() may still produce authoritative success.
    case "cancel_file_transcription": return [...pending.values()].some(task => task.runId === args.runId);
    case "save_transcription_text": return true;
    case "copy_transcription_text": return null;
    case "begin_meeting_file": {
      active++; maximum = Math.max(maximum, active);
      const id = `meeting-${++sequence}`;
      meetings.set(id, { path: args.filePath, status: "running", released: false });
      return id;
    }
    case "get_meeting_job": {
      const task = meetings.get(args.jobId);
      if (task.failPoll) { task.failPoll = false; throw new Error("Synthetic poll failure"); }
      if (task.status !== "running" && !task.released) { active--; task.released = true; }
      return { id: args.jobId, status: task.status, phase: task.status, error: null,
        transcript: task.status === "completed" ? transcript(task.path) : null };
    }
    case "cancel_meeting_job": meetings.get(args.jobId).status = "cancelled"; return true;
    case "create_meeting_review": return {
      review: { schema_version: 1, original: args.transcript, original_revision: "original", generation: 0,
        batches: [], revision: "revision-0" }, transcript: args.transcript,
    };
    case "plugin:dialog|open": return ["/fixtures/picked.wav"];
    default: return null;
  }
}, { shouldMockEvents: true });
await import("/src/main.ts");
