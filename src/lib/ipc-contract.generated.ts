// Generated from the Rust command signatures by src-tauri/src/contract/command_map.rs
// (#347). Do not edit: after changing a command, run
// `SAGASCRIPT_BLESS_CONTRACTS=1 cargo test -p sagascript contract` in src-tauri/.
// The app crate's tests fail while this file is out of date.
import type { AppState, BuildInfo, EngineLoadState, EngineStatus, GlossarySuggestion, HotkeyMode, HotkeyProfile, HotkeyStatus, Language, LoadedModelInfo, MeetingJobSnapshot, Settings, TrainingTranscript, WhisperModel } from "./api";
import type { MeetingReprocessingProposal, ProposalState, ReprocessingMode, ReprocessingRequest, SelectedReprocessingPlan } from "./meeting-reprocessing-types";
import type { CorrectionFile, CorrectionOperation, MeetingAudioAttachment, MeetingExportFormat, MeetingReview, MeetingReviewState, MeetingTranscript } from "./meeting-types";
import type { UpdateRecoveryPayload } from "./update-recovery";

/** No arguments. */
type NoArgs = Record<string, never>;

/**
 * Every command the backend registers (`tauri::generate_handler!` in
 * src-tauri/src/main.rs): the argument object the frontend sends, keyed by
 * the Rust parameter names in camelCase (an `Option` parameter may be
 * omitted or null), and the value the command resolves with. A command
 * that fails rejects with its error string.
 */
export interface CommandMap {
  /** `meeting_reprocessing_commands::accept_meeting_proposal(proposal: MeetingReprocessingProposal, current_review: MeetingReview) -> Result<ReviewState, String>` */
  accept_meeting_proposal: { args: { proposal: MeetingReprocessingProposal; currentReview: MeetingReview }; result: MeetingReviewState };
  /** `commands::acknowledge_update_result(expected_text: String) -> Result<bool, String>` */
  acknowledge_update_result: { args: { expectedText: string }; result: boolean };
  /** `meeting_review_commands::apply_meeting_corrections(review: MeetingReview, corrections: CorrectionFile) -> Result<ReviewState, String>` */
  apply_meeting_corrections: { args: { review: MeetingReview; corrections: CorrectionFile }; result: MeetingReviewState };
  /** `commands::apply_training_glossary(heard: String, corrected: String, profile_id: String, accepted: Vec<GlossarySuggestion>) -> Result<(), String>` */
  apply_training_glossary: { args: { heard: string; corrected: string; profileId: string; accepted: GlossarySuggestion[] }; result: void };
  /** `meeting_media::attach_meeting_audio(source_sha256: String) -> Result<Option<AudioAttachment>, String>` */
  attach_meeting_audio: { args: { sourceSha256: string }; result: MeetingAudioAttachment | null };
  /** `meeting_jobs::begin_meeting_file(file_path: String, prompt: Option<String>, profile_id: Option<String>) -> Result<String, String>` */
  begin_meeting_file: { args: { filePath: string; prompt?: string | null; profileId?: string | null }; result: string };
  /** `meeting_jobs::begin_meeting_reprocessing(file_path: String, prompt: Option<String>, profile_id: Option<String>, request: ReprocessingRequest) -> Result<String, String>` */
  begin_meeting_reprocessing: { args: { filePath: string; prompt?: string | null; profileId?: string | null; request: ReprocessingRequest }; result: string };
  /** `commands::cancel_file_transcription(run_id: String) -> Result<bool, String>` */
  cancel_file_transcription: { args: { runId: string }; result: boolean };
  /** `meeting_jobs::cancel_meeting_job(job_id: String) -> Result<bool, String>` */
  cancel_meeting_job: { args: { jobId: string }; result: boolean };
  /** `commands::cancel_recording() -> Result<(), String>` */
  cancel_recording: { args: NoArgs; result: void };
  /** `commands::check_accessibility_permission() -> Result<bool, String>` */
  check_accessibility_permission: { args: NoArgs; result: boolean };
  /** `commands::clear_update_recovery() -> Result<(), String>` */
  clear_update_recovery: { args: NoArgs; result: void };
  /** `commands::complete_update_preparation(nonce: String, error: Option<String>) -> Result<(), String>` */
  complete_update_preparation: { args: { nonce: string; error?: string | null }; result: void };
  /** `commands::copy_transcription_text(text: String) -> Result<(), String>` */
  copy_transcription_text: { args: { text: string }; result: void };
  /** `meeting_review_commands::create_meeting_review(transcript: MeetingTranscript) -> Result<ReviewState, String>` */
  create_meeting_review: { args: { transcript: MeetingTranscript }; result: MeetingReviewState };
  /** `meeting_media::detach_meeting_audio(token: String) -> Result<(), String>` */
  detach_meeting_audio: { args: { token: string }; result: void };
  /** `commands::download_model(whisper_model: WhisperModel) -> Result<(), String>` */
  download_model: { args: { whisperModel: string }; result: void };
  /** `commands::download_pianissimo_model() -> Result<(), String>` */
  download_pianissimo_model: { args: NoArgs; result: void };
  /** `commands::engine_status() -> Result<EngineStatus, String>` */
  engine_status: { args: NoArgs; result: EngineStatus };
  /** `commands::get_active_hotkey_profile() -> Result<Option<HotkeyProfile>, String>` */
  get_active_hotkey_profile: { args: NoArgs; result: HotkeyProfile | null };
  /** `commands::get_auto_file_model_info(language: Language) -> Result<ModelInfo, String>` */
  get_auto_file_model_info: { args: { language: Language }; result: WhisperModel };
  /** `commands::get_build_info() -> Result<BuildInfo, String>` */
  get_build_info: { args: NoArgs; result: BuildInfo };
  /** `commands::get_dictation_model_info(language: Language) -> Result<ModelInfo, String>` */
  get_dictation_model_info: { args: { language: Language }; result: WhisperModel };
  /** `commands::get_effective_model_info(language: Language) -> Result<ModelInfo, String>` */
  get_effective_model_info: { args: { language: Language }; result: WhisperModel };
  /** `commands::get_engine_load_state() -> Result<serde_json::Value, String>` */
  get_engine_load_state: { args: NoArgs; result: EngineLoadState };
  /** `commands::get_file_model_options(language: Language, include: Option<String>) -> Result<Vec<ModelInfo>, String>` */
  get_file_model_options: { args: { language: Language; include?: string | null }; result: WhisperModel[] };
  /** `commands::get_last_error() -> Result<Option<String>, String>` */
  get_last_error: { args: NoArgs; result: string | null };
  /** `commands::get_last_transcription() -> Result<Option<String>, String>` */
  get_last_transcription: { args: NoArgs; result: string | null };
  /** `commands::get_loaded_model() -> Result<LoadedModelInfo, String>` */
  get_loaded_model: { args: NoArgs; result: LoadedModelInfo };
  /** `meeting_jobs::get_meeting_job(job_id: String) -> Result<MeetingSnapshot, String>` */
  get_meeting_job: { args: { jobId: string }; result: MeetingJobSnapshot };
  /** `commands::get_model_info() -> Result<Vec<ModelInfo>, String>` */
  get_model_info: { args: NoArgs; result: WhisperModel[] };
  /** `commands::get_platform() -> Result<String, String>` */
  get_platform: { args: NoArgs; result: string };
  /** `commands::get_profile_model_info(profile_id: String) -> Result<ModelInfo, String>` */
  get_profile_model_info: { args: { profileId: string }; result: WhisperModel };
  /** `commands::get_settings() -> Result<Settings, String>` */
  get_settings: { args: NoArgs; result: Settings };
  /** `commands::get_state() -> Result<AppState, String>` */
  get_state: { args: NoArgs; result: AppState };
  /** `commands::get_supported_formats() -> Result<Vec<String>, String>` */
  get_supported_formats: { args: NoArgs; result: string[] };
  /** `commands::get_update_result_pending(result_id: String) -> Result<bool, String>` */
  get_update_result_pending: { args: { resultId: string }; result: boolean };
  /** `commands::hotkey_status() -> Result<HotkeyStatus, String>` */
  hotkey_status: { args: NoArgs; result: HotkeyStatus };
  /** `commands::is_model_downloaded(whisper_model: WhisperModel) -> Result<bool, String>` */
  is_model_downloaded: { args: { whisperModel: string }; result: boolean };
  /** `commands::is_model_ready() -> Result<bool, String>` */
  is_model_ready: { args: NoArgs; result: boolean };
  /** `commands::load_update_recovery() -> Result<Option<serde_json::Value>, String>` */
  load_update_recovery: { args: NoArgs; result: UpdateRecoveryPayload | null };
  /** `meeting_jobs::merge_meeting_speakers(transcript: MeetingTranscript, from_id: String, into_id: String) -> Result<MeetingTranscript, String>` */
  merge_meeting_speakers: { args: { transcript: MeetingTranscript; fromId: string; intoId: string }; result: MeetingTranscript };
  /** `commands::microphone_status() -> Result<String, String>` */
  microphone_status: { args: NoArgs; result: string };
  /** `commands::open_accessibility_settings() -> Result<(), String>` */
  open_accessibility_settings: { args: NoArgs; result: void };
  /** `meeting_reprocessing_commands::open_meeting_proposal() -> Result<Option<ProposalState>, String>` */
  open_meeting_proposal: { args: NoArgs; result: ProposalState | null };
  /** `meeting_review_commands::open_meeting_review() -> Result<Option<ReviewState>, String>` */
  open_meeting_review: { args: NoArgs; result: MeetingReviewState | null };
  /** `commands::open_microphone_settings() -> Result<(), String>` */
  open_microphone_settings: { args: NoArgs; result: void };
  /** `meeting_reprocessing_commands::plan_meeting_reprocessing(previous: MeetingReview, mode: ReprocessingMode, threshold: f32, save_cache: bool, prompt: Option<String>, profile_id: Option<String>) -> Result<Option<SelectedReprocessingPlan>, String>` */
  plan_meeting_reprocessing: { args: { previous: MeetingReview; mode: ReprocessingMode; threshold: number; saveCache: boolean; prompt?: string | null; profileId?: string | null }; result: SelectedReprocessingPlan | null };
  /** `meeting_reprocessing_commands::preview_meeting_proposal(proposal: MeetingReprocessingProposal) -> Result<ProposalState, String>` */
  preview_meeting_proposal: { args: { proposal: MeetingReprocessingProposal }; result: ProposalState };
  /** `meeting_jobs::rename_meeting_speaker(transcript: MeetingTranscript, speaker_id: String, label: String) -> Result<MeetingTranscript, String>` */
  rename_meeting_speaker: { args: { transcript: MeetingTranscript; speakerId: string; label: string }; result: MeetingTranscript };
  /** `commands::request_accessibility_permission() -> Result<(), String>` */
  request_accessibility_permission: { args: NoArgs; result: void };
  /** `commands::request_microphone_access() -> Result<String, String>` */
  request_microphone_access: { args: NoArgs; result: string };
  /** `meeting_review_commands::reset_meeting_review(review: MeetingReview, expected_revision: String) -> Result<ReviewState, String>` */
  reset_meeting_review: { args: { review: MeetingReview; expectedRevision: string }; result: MeetingReviewState };
  /** `meeting_reprocessing_commands::resolve_meeting_proposal(proposal: MeetingReprocessingProposal, expected_revision: String, index: usize, operations: Vec<CorrectionOperation>) -> Result<ProposalState, String>` */
  resolve_meeting_proposal: { args: { proposal: MeetingReprocessingProposal; expectedRevision: string; index: number; operations: CorrectionOperation[] }; result: ProposalState };
  /** `commands::retry_hotkey_registration() -> Result<(), String>` */
  retry_hotkey_registration: { args: NoArgs; result: void };
  /** `meeting_jobs::save_meeting_export(transcript: MeetingTranscript, format: ExportFormat) -> Result<bool, String>` */
  save_meeting_export: { args: { transcript: MeetingTranscript; format: MeetingExportFormat }; result: boolean };
  /** `meeting_reprocessing_commands::save_meeting_proposal(proposal: MeetingReprocessingProposal) -> Result<bool, String>` */
  save_meeting_proposal: { args: { proposal: MeetingReprocessingProposal }; result: boolean };
  /** `meeting_review_commands::save_meeting_review(review: MeetingReview, format: ExportFormat) -> Result<bool, String>` */
  save_meeting_review: { args: { review: MeetingReview; format: MeetingExportFormat }; result: boolean };
  /** `commands::save_transcription_text(text: String, file_name: String, directory: Option<String>) -> Result<bool, String>` */
  save_transcription_text: { args: { text: string; fileName: string; directory?: string | null }; result: boolean };
  /** `commands::save_update_recovery(payload: serde_json::Value) -> Result<(), String>` */
  save_update_recovery: { args: { payload: UpdateRecoveryPayload }; result: void };
  /** `commands::set_auto_paste(enabled: bool) -> Result<(), String>` */
  set_auto_paste: { args: { enabled: boolean }; result: void };
  /** `commands::set_auto_select_model(enabled: bool) -> Result<(), String>` */
  set_auto_select_model: { args: { enabled: boolean }; result: void };
  /** `commands::set_beam_size(beam_size: u32) -> Result<(), String>` */
  set_beam_size: { args: { beamSize: number }; result: void };
  /** `commands::set_engine_idle_unload_minutes(minutes: u32) -> Result<(), String>` */
  set_engine_idle_unload_minutes: { args: { minutes: number }; result: void };
  /** `commands::set_engine_prewarm(mode: String) -> Result<(), String>` */
  set_engine_prewarm: { args: { mode: string }; result: void };
  /** `commands::set_file_transcription_model(model_id: String) -> Result<(), String>` */
  set_file_transcription_model: { args: { modelId: string }; result: void };
  /** `commands::set_hotkey(shortcut: String) -> Result<(), String>` */
  set_hotkey: { args: { shortcut: string }; result: void };
  /** `commands::set_hotkey_mode(mode: HotkeyMode) -> Result<(), String>` */
  set_hotkey_mode: { args: { mode: HotkeyMode }; result: void };
  /** `commands::set_hotkey_profiles(profiles: Vec<HotkeyProfile>) -> Result<Vec<String>, String>` */
  set_hotkey_profiles: { args: { profiles: HotkeyProfile[] }; result: string[] };
  /** `commands::set_initial_prompt(prompt: String, expected_source: Option<String>) -> Result<(), String>` */
  set_initial_prompt: { args: { prompt: string; expectedSource?: string | null }; result: void };
  /** `commands::set_language(language: Language) -> Result<Vec<String>, String>` */
  set_language: { args: { language: Language }; result: string[] };
  /** `commands::set_onboarding_completed() -> Result<(), String>` */
  set_onboarding_completed: { args: NoArgs; result: void };
  /** `commands::set_pianissimo_dictation(enabled: bool) -> Result<(), String>` */
  set_pianissimo_dictation: { args: { enabled: boolean }; result: void };
  /** `commands::set_profile_glossary(profile_id: String, source: String, expected_source: Option<String>) -> Result<(), String>` */
  set_profile_glossary: { args: { profileId: string; source: string; expectedSource?: string | null }; result: void };
  /** `commands::set_profile_model(profile_id: String, model_id: String) -> Result<(), String>` */
  set_profile_model: { args: { profileId: string; modelId: string }; result: void };
  /** `commands::set_show_overlay(enabled: bool) -> Result<(), String>` */
  set_show_overlay: { args: { enabled: boolean }; result: void };
  /** `commands::set_temperature_fallback(enabled: bool) -> Result<(), String>` */
  set_temperature_fallback: { args: { enabled: boolean }; result: void };
  /** `commands::set_update_result_pending(result_id: String, pending: bool) -> Result<(), String>` */
  set_update_result_pending: { args: { resultId: string; pending: boolean }; result: void };
  /** `commands::set_vad_enabled(enabled: bool) -> Result<(), String>` */
  set_vad_enabled: { args: { enabled: boolean }; result: void };
  /** `commands::set_whisper_model(model: WhisperModel) -> Result<(), String>` */
  set_whisper_model: { args: { model: string }; result: void };
  /** `commands::start_recording() -> Result<(), String>` */
  start_recording: { args: NoArgs; result: void };
  /** `commands::start_training_recording(profile_id: String) -> Result<(), String>` */
  start_training_recording: { args: { profileId: string }; result: void };
  /** `commands::stop_and_transcribe() -> Result<String, String>` */
  stop_and_transcribe: { args: NoArgs; result: string };
  /** `commands::stop_and_transcribe_training() -> Result<TrainingTranscript, String>` */
  stop_and_transcribe_training: { args: NoArgs; result: TrainingTranscript };
  /** `commands::suggest_training_glossary(heard: String, corrected: String, profile_id: String) -> Result<Vec<GlossarySuggestion>, String>` */
  suggest_training_glossary: { args: { heard: string; corrected: string; profileId: string }; result: GlossarySuggestion[] };
  /** `commands::transcribe_file(file_path: String, prompt: Option<String>, diarize: Option<bool>, profile_id: Option<String>, run_id: Option<String>, auto_paste: Option<bool>) -> Result<String, String>` */
  transcribe_file: { args: { filePath: string; prompt?: string | null; diarize?: boolean | null; profileId?: string | null; runId?: string | null; autoPaste?: boolean | null }; result: string };
  /** `commands::transcribe_training_file(file_path: String, profile_id: String) -> Result<TrainingTranscript, String>` */
  transcribe_training_file: { args: { filePath: string; profileId: string }; result: TrainingTranscript };
  /** `meeting_review_commands::undo_meeting_review(review: MeetingReview, expected_revision: String) -> Result<ReviewState, String>` */
  undo_meeting_review: { args: { review: MeetingReview; expectedRevision: string }; result: MeetingReviewState };
}

/** The registered command names, sorted. */
export const COMMAND_NAMES = [
  "accept_meeting_proposal",
  "acknowledge_update_result",
  "apply_meeting_corrections",
  "apply_training_glossary",
  "attach_meeting_audio",
  "begin_meeting_file",
  "begin_meeting_reprocessing",
  "cancel_file_transcription",
  "cancel_meeting_job",
  "cancel_recording",
  "check_accessibility_permission",
  "clear_update_recovery",
  "complete_update_preparation",
  "copy_transcription_text",
  "create_meeting_review",
  "detach_meeting_audio",
  "download_model",
  "download_pianissimo_model",
  "engine_status",
  "get_active_hotkey_profile",
  "get_auto_file_model_info",
  "get_build_info",
  "get_dictation_model_info",
  "get_effective_model_info",
  "get_engine_load_state",
  "get_file_model_options",
  "get_last_error",
  "get_last_transcription",
  "get_loaded_model",
  "get_meeting_job",
  "get_model_info",
  "get_platform",
  "get_profile_model_info",
  "get_settings",
  "get_state",
  "get_supported_formats",
  "get_update_result_pending",
  "hotkey_status",
  "is_model_downloaded",
  "is_model_ready",
  "load_update_recovery",
  "merge_meeting_speakers",
  "microphone_status",
  "open_accessibility_settings",
  "open_meeting_proposal",
  "open_meeting_review",
  "open_microphone_settings",
  "plan_meeting_reprocessing",
  "preview_meeting_proposal",
  "rename_meeting_speaker",
  "request_accessibility_permission",
  "request_microphone_access",
  "reset_meeting_review",
  "resolve_meeting_proposal",
  "retry_hotkey_registration",
  "save_meeting_export",
  "save_meeting_proposal",
  "save_meeting_review",
  "save_transcription_text",
  "save_update_recovery",
  "set_auto_paste",
  "set_auto_select_model",
  "set_beam_size",
  "set_engine_idle_unload_minutes",
  "set_engine_prewarm",
  "set_file_transcription_model",
  "set_hotkey",
  "set_hotkey_mode",
  "set_hotkey_profiles",
  "set_initial_prompt",
  "set_language",
  "set_onboarding_completed",
  "set_pianissimo_dictation",
  "set_profile_glossary",
  "set_profile_model",
  "set_show_overlay",
  "set_temperature_fallback",
  "set_update_result_pending",
  "set_vad_enabled",
  "set_whisper_model",
  "start_recording",
  "start_training_recording",
  "stop_and_transcribe",
  "stop_and_transcribe_training",
  "suggest_training_glossary",
  "transcribe_file",
  "transcribe_training_file",
  "undo_meeting_review",
] as const satisfies readonly (keyof CommandMap)[];

/**
 * Each command's argument keys, for checks at run time: `required` must be
 * sent, `optional` (an `Option` parameter) may be.
 */
export const COMMAND_ARGUMENTS: {
  readonly [C in keyof CommandMap]: {
    readonly required: readonly (keyof CommandMap[C]["args"])[];
    readonly optional: readonly (keyof CommandMap[C]["args"])[];
  };
} = {
  accept_meeting_proposal: { required: ["proposal", "currentReview"], optional: [] },
  acknowledge_update_result: { required: ["expectedText"], optional: [] },
  apply_meeting_corrections: { required: ["review", "corrections"], optional: [] },
  apply_training_glossary: { required: ["heard", "corrected", "profileId", "accepted"], optional: [] },
  attach_meeting_audio: { required: ["sourceSha256"], optional: [] },
  begin_meeting_file: { required: ["filePath"], optional: ["prompt", "profileId"] },
  begin_meeting_reprocessing: { required: ["filePath", "request"], optional: ["prompt", "profileId"] },
  cancel_file_transcription: { required: ["runId"], optional: [] },
  cancel_meeting_job: { required: ["jobId"], optional: [] },
  cancel_recording: { required: [], optional: [] },
  check_accessibility_permission: { required: [], optional: [] },
  clear_update_recovery: { required: [], optional: [] },
  complete_update_preparation: { required: ["nonce"], optional: ["error"] },
  copy_transcription_text: { required: ["text"], optional: [] },
  create_meeting_review: { required: ["transcript"], optional: [] },
  detach_meeting_audio: { required: ["token"], optional: [] },
  download_model: { required: ["whisperModel"], optional: [] },
  download_pianissimo_model: { required: [], optional: [] },
  engine_status: { required: [], optional: [] },
  get_active_hotkey_profile: { required: [], optional: [] },
  get_auto_file_model_info: { required: ["language"], optional: [] },
  get_build_info: { required: [], optional: [] },
  get_dictation_model_info: { required: ["language"], optional: [] },
  get_effective_model_info: { required: ["language"], optional: [] },
  get_engine_load_state: { required: [], optional: [] },
  get_file_model_options: { required: ["language"], optional: ["include"] },
  get_last_error: { required: [], optional: [] },
  get_last_transcription: { required: [], optional: [] },
  get_loaded_model: { required: [], optional: [] },
  get_meeting_job: { required: ["jobId"], optional: [] },
  get_model_info: { required: [], optional: [] },
  get_platform: { required: [], optional: [] },
  get_profile_model_info: { required: ["profileId"], optional: [] },
  get_settings: { required: [], optional: [] },
  get_state: { required: [], optional: [] },
  get_supported_formats: { required: [], optional: [] },
  get_update_result_pending: { required: ["resultId"], optional: [] },
  hotkey_status: { required: [], optional: [] },
  is_model_downloaded: { required: ["whisperModel"], optional: [] },
  is_model_ready: { required: [], optional: [] },
  load_update_recovery: { required: [], optional: [] },
  merge_meeting_speakers: { required: ["transcript", "fromId", "intoId"], optional: [] },
  microphone_status: { required: [], optional: [] },
  open_accessibility_settings: { required: [], optional: [] },
  open_meeting_proposal: { required: [], optional: [] },
  open_meeting_review: { required: [], optional: [] },
  open_microphone_settings: { required: [], optional: [] },
  plan_meeting_reprocessing: { required: ["previous", "mode", "threshold", "saveCache"], optional: ["prompt", "profileId"] },
  preview_meeting_proposal: { required: ["proposal"], optional: [] },
  rename_meeting_speaker: { required: ["transcript", "speakerId", "label"], optional: [] },
  request_accessibility_permission: { required: [], optional: [] },
  request_microphone_access: { required: [], optional: [] },
  reset_meeting_review: { required: ["review", "expectedRevision"], optional: [] },
  resolve_meeting_proposal: { required: ["proposal", "expectedRevision", "index", "operations"], optional: [] },
  retry_hotkey_registration: { required: [], optional: [] },
  save_meeting_export: { required: ["transcript", "format"], optional: [] },
  save_meeting_proposal: { required: ["proposal"], optional: [] },
  save_meeting_review: { required: ["review", "format"], optional: [] },
  save_transcription_text: { required: ["text", "fileName"], optional: ["directory"] },
  save_update_recovery: { required: ["payload"], optional: [] },
  set_auto_paste: { required: ["enabled"], optional: [] },
  set_auto_select_model: { required: ["enabled"], optional: [] },
  set_beam_size: { required: ["beamSize"], optional: [] },
  set_engine_idle_unload_minutes: { required: ["minutes"], optional: [] },
  set_engine_prewarm: { required: ["mode"], optional: [] },
  set_file_transcription_model: { required: ["modelId"], optional: [] },
  set_hotkey: { required: ["shortcut"], optional: [] },
  set_hotkey_mode: { required: ["mode"], optional: [] },
  set_hotkey_profiles: { required: ["profiles"], optional: [] },
  set_initial_prompt: { required: ["prompt"], optional: ["expectedSource"] },
  set_language: { required: ["language"], optional: [] },
  set_onboarding_completed: { required: [], optional: [] },
  set_pianissimo_dictation: { required: ["enabled"], optional: [] },
  set_profile_glossary: { required: ["profileId", "source"], optional: ["expectedSource"] },
  set_profile_model: { required: ["profileId", "modelId"], optional: [] },
  set_show_overlay: { required: ["enabled"], optional: [] },
  set_temperature_fallback: { required: ["enabled"], optional: [] },
  set_update_result_pending: { required: ["resultId", "pending"], optional: [] },
  set_vad_enabled: { required: ["enabled"], optional: [] },
  set_whisper_model: { required: ["model"], optional: [] },
  start_recording: { required: [], optional: [] },
  start_training_recording: { required: ["profileId"], optional: [] },
  stop_and_transcribe: { required: [], optional: [] },
  stop_and_transcribe_training: { required: [], optional: [] },
  suggest_training_glossary: { required: ["heard", "corrected", "profileId"], optional: [] },
  transcribe_file: { required: ["filePath"], optional: ["prompt", "diarize", "profileId", "runId", "autoPaste"] },
  transcribe_training_file: { required: ["filePath", "profileId"], optional: [] },
  undo_meeting_review: { required: ["review", "expectedRevision"], optional: [] },
};
