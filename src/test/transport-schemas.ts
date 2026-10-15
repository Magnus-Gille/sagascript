// Schemas of the payloads Rust sends over IPC (#347), one per frontend type.
// src/test/transport.test.ts proves each equal to its TypeScript type and
// parses the shared fixtures in contracts/transport/ with it.
import { array, boolean, literal, nullable, number, object, openObject, record, string, union } from "./schema";

// -- Settings (`get_settings`) ------------------------------------------------

export const language = literal("en", "sv", "no", "fi", "auto");

export const hotkeyProfile = object(
  { id: string, name: string, shortcut: string, language },
  // Rust omits an unset shortcut; the frontend may send null.
  { optional: { push_to_talk_shortcut: nullable(string), toggle_shortcut: nullable(string) } },
);

export const settings = object(
  {
    language,
    whisper_model: string,
    file_transcription_model: string,
    pianissimo_dictation: boolean,
    hotkey_mode: literal("push", "toggle"),
    show_overlay: boolean,
    auto_paste: boolean,
    auto_select_model: boolean,
    hotkey: string,
    hotkey_profiles: array(hotkeyProfile),
    profile_models: record(string),
    initial_prompt: string,
    profile_glossaries: record(string),
    beam_size: number,
    temperature_fallback: boolean,
    vad_enabled: boolean,
    engine_prewarm: literal("off", "on_app_start", "on_key_down"),
    engine_idle_unload_minutes: number,
    has_completed_onboarding: boolean,
  },
  // Migration markers (ADR 0007): written and read by Rust only.
  { ignored: ["profile_glossary_migrated", "model_lineup_version", "engine_defaults_version", "schema_version"] },
);

// -- Command errors (`Result<T, CommandError>`, ADR 0006) --------------------------

export const backendError = object({ code: string, message: string, retryable: boolean });

// -- Meeting documents ----------------------------------------------------------

const speakerCountHint = object({}, { optional: { exact: number, min: number, max: number, force: boolean } });
const speakerHintOutcome = object({ satisfied: boolean, delivered: number });

const diarizationReport = openObject(
  {
    schema_version: number,
    source_sha256: string,
    duration_seconds: number,
    build_revision: string,
    build_version: string,
    asr_segments: array(
      object({ start: number, end: number }, { optional: { avg_logprob: nullable(number), no_speech_prob: nullable(number) } }),
    ),
    transcript_modified: boolean,
    parameters: record(union(number, boolean)),
    activity: array(object({ start: number, end: number, speakers: array(string) })),
    regions: array(
      object(
        {
          index: number,
          start: number,
          end: number,
          track: number,
          speaker: string,
          embedding_status: literal("usable", "missing", "degenerate"),
          assigned_centroid_distance: nullable(number),
          nearest_other_centroid_distance: nullable(number),
          used_track_fallback: boolean,
        },
        { optional: { active_speech_seconds: nullable(number), overlapping_speech_seconds: nullable(number) } },
      ),
    ),
    attributions: array(
      object(
        {
          index: number,
          start: number,
          end: number,
          speaker: string,
          reason: literal("temporal_overlap", "tied_overlap", "nearest_gap", "no_speaker_evidence", "invalid_timestamp"),
          support: array(object({ speaker: string, overlap_seconds: number })),
        },
        { optional: { margin_seconds: nullable(number), gap_seconds: nullable(number) } },
      ),
    ),
  },
  {
    optional: {
      segmentation_model_sha256: nullable(string),
      embedding_model_sha256: nullable(string),
      decoder: nullable(
        object({
          strategy: string,
          beam_size: number,
          temperature_fallback: boolean,
          vad_enabled: boolean,
          vad_threshold: number,
          vad_min_silence_duration_ms: number,
          vad_speech_pad_ms: number,
          vad_samples_overlap: number,
          timestamp_method: string,
        }),
      ),
      diagnostics_included: boolean,
      speaker_hint: nullable(speakerCountHint),
      speaker_hint_outcome: nullable(speakerHintOutcome),
    },
  },
);

export const meetingTranscript = object(
  {
    schema_version: number,
    source_sha256: string,
    language: string,
    model: string,
    duration_seconds: number,
    segments: array(object({ id: string, start: number, end: number, text: string, speaker: string })),
    speakers: array(object({ id: string, label: string })),
  },
  {
    optional: {
      speaker_hint: nullable(speakerCountHint),
      speaker_hint_satisfied: nullable(boolean),
      speaker_hint_delivered: nullable(number),
      diarization: nullable(diarizationReport),
    },
  },
);

export const correctionOperation = union(
  object(
    { kind: literal("edit_segment"), segment_id: string },
    // Rust sends an unchanged field as null; the frontend omits it.
    { optional: { text: nullable(string), speaker_id: nullable(string) } },
  ),
  object({ kind: literal("rename_speaker"), speaker_id: string, label: string }),
  object({ kind: literal("merge_speakers"), from_id: string, into_id: string }),
);

export const meetingReview = object({
  schema_version: number,
  original: meetingTranscript,
  original_revision: string,
  generation: number,
  batches: array(object({ operations: array(correctionOperation) })),
  revision: string,
});

const requiredWork = object({
  decode_audio: boolean,
  transcription: boolean,
  language_detection: boolean,
  segmentation: boolean,
  embeddings: boolean,
  clustering: boolean,
});

export const reprocessingResult = object({
  proposal: object({
    schema_version: number,
    previous: meetingReview,
    proposed: meetingTranscript,
    generation: number,
    resolutions: array(nullable(array(correctionOperation))),
    revision: string,
  }),
  required_work: requiredWork,
  timings: object({
    validation_seconds: number,
    decode_seconds: number,
    analysis_seconds: number,
    clustering_seconds: number,
    full_pipeline_seconds: number,
    proposal_seconds: number,
    total_seconds: number,
  }),
});

// -- Meeting jobs (`get_meeting_job`) -----------------------------------------

export const meetingJobStatus = literal("running", "cancelling", "completed", "cancelled", "failed");

export const meetingJobSnapshot = object(
  { id: string, status: meetingJobStatus, phase: string, error: nullable(string), transcript: nullable(meetingTranscript) },
  // `reprocessing` exists only in builds with the `diarization` feature.
  { optional: { elapsed_ms: number, phase_elapsed_ms: number, reprocessing: nullable(reprocessingResult) } },
);

// -- Events (src/lib/events.ts) -----------------------------------------------

export const hotkeyStatus = object({
  ok: boolean,
  error: nullable(string),
  shortcut: string,
  shortcuts: array(string),
  settings_watcher: object({ running: boolean, recovered_panics: number }),
});

export const engineLoadState = object(
  { state: literal("unknown", "loading", "ready", "failed") },
  { optional: { source: literal("warm", "request"), loadMs: number, message: string } },
);

/** One schema per `EventMap` entry. */
export const eventPayloads = {
  "state-changed": literal("idle", "recording", "loading_model", "transcribing", "error", "settings_reloaded"),
  "transcription-result": string,
  error: string,
  "model-download-progress": object({ progress: number }, { ignored: ["model", "downloaded", "total"] }),
  "model-ready": literal(null),
  "plain-transcription-progress": object({
    runId: string,
    phase: literal("decoding", "resampling", "loading", "preparing", "encoding", "transcribing", "finalizing"),
    percent: nullable(number),
  }),
  "hotkey-registration-changed": hotkeyStatus,
  "active-hotkey-profile-changed": hotkeyProfile,
  "engine-load-state": engineLoadState,
  "update-preparing": string,
  "update-aborted": string,
  navigate_tab: string,
};
