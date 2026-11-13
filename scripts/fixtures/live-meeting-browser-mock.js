// Synthetic live-meeting fixture. It mounts only LiveMeetingPanel and never
// invokes Tauri, native audio, settings, or a backend service.
import { mount } from "svelte";
import LiveMeetingPanel from "../../src/lib/LiveMeetingPanel.svelte";

const calls = [];
let saveTextFailures = 1;
let snapshot = createSnapshot("running", 1);

function source(sourceName, overrides = {}) {
  return {
    source: sourceName,
    connected: true,
    signal_level: sourceName === "microphone" ? 0.82 : 0.64,
    last_audio_ms: 412_000,
    last_text_ms: 411_200,
    text_delay_ms: 800,
    queued_ms: sourceName === "microphone" ? 220 : 540,
    dropped_frames: sourceName === "microphone" ? 0 : 3,
    error: null,
    ...overrides,
  };
}

function segment(id, sourceName, startMs, text, status = "final", revision = 1) {
  return { id, source: sourceName, start_ms: startMs, end_ms: startMs + 400, text, status, revision };
}

function historySegments() {
  const segments = Array.from({ length: 420 }, (_, index) =>
    segment(`history-${index}`, index % 2 === 0 ? "microphone" : "system", index * 1_000, `Historik ${index}`),
  );
  segments.push(segment("preview-421", "microphone", 420_500, "Preview från mig", "preview", 2));
  segments.push(segment("final-421", "system", 420_500, "Finalt från mötet", "final", 1));
  // ASR can finalize an energy-only window without words. These raw rows
  // must not add visible articles or change the 422-row pagination count.
  segments.push(segment("blank-mic", "microphone", 420_500, "   "));
  segments.push(segment("blank-system", "system", 420_500, ""));
  return segments;
}

function createSnapshot(phase, revision, error = null) {
  return {
    id: "qa-live-session",
    revision,
    phase,
    elapsed_ms: 421_000,
    sources: [source("microphone"), source("system")],
    segments: historySegments(),
    gaps: [{ source: "system", start_ms: 398_000, end_ms: 399_000, reason: "fixture silence" }],
    error,
    audio_available: true,
  };
}

function clone(value) {
  return value === null ? null : JSON.parse(JSON.stringify(value));
}

const api = {
  async load() {
    return clone(snapshot);
  },
  async start() {
    calls.push({ action: "start" });
    snapshot = createSnapshot("running", (snapshot?.revision ?? 0) + 1);
  },
  async stop(id) {
    calls.push({ action: "stop", id });
    snapshot = { ...snapshot, phase: "draining", revision: snapshot.revision + 1 };
  },
  async discard(id) {
    calls.push({ action: "discard", id });
    snapshot = null;
  },
  async saveText(id) {
    calls.push({ action: "save-text", id });
    if (saveTextFailures > 0) {
      saveTextFailures -= 1;
      throw new Error("Synthetic text export failed; temporary text remains available.");
    }
  },
  async saveAudio(id) {
    calls.push({ action: "save-audio", id });
  },
};

window.qa = {
  calls,
  setPhase(phase, error = null) {
    if (!snapshot) return;
    snapshot = { ...snapshot, phase, error, revision: snapshot.revision + 1 };
  },
  setRefinement(status, progress = 0, error = null) {
    if (!snapshot) return;
    snapshot = { ...snapshot, phase: status === "running" ? "refining" : "completed", refinement: {status, progress_percent: progress, error}, error, revision: snapshot.revision + 1 };
    if (status === "completed") snapshot.segments = [segment("refined-0", "microphone", 1000, "Efterbearbetad text")];
  },
  setSaveTextFailures(count) {
    saveTextFailures = count;
  },
  snapshot() {
    return clone(snapshot);
  },
};

mount(LiveMeetingPanel, {
  target: document.getElementById("app"),
  props: { api, supported: true },
});
