// Generated from the Rust command signatures by src-tauri/src/contract/command_map.rs
// (#347). Do not edit: after changing a command, run
// `SAGASCRIPT_BLESS_CONTRACTS=1 cargo test -p sagascript contract` in src-tauri/.
// The app crate's tests fail while this file is out of date.
import type { PreflightReport } from "./acceptance";
import type { AppState, BuildInfo, EngineLoadState, EngineStatus, GlossarySuggestion, HotkeyMode, HotkeyProfile, HotkeyStatus, Language, LoadedModelInfo, MeetingJobSnapshot, Settings, TrainingTranscript, WhisperModel } from "./api";
import type { MeetingReprocessingProposal, ProposalState, ReprocessingMode, ReprocessingRequest, SelectedReprocessingPlan } from "./meeting-reprocessing-types";
import type { CorrectionFile, CorrectionOperation, MeetingAudioAttachment, MeetingExportFormat, MeetingReview, MeetingReviewState, MeetingTranscript } from "./meeting-types";
import type { UpdateRecoveryPayload } from "./update-recovery";
import type { BackendError } from "./errors";

/** No arguments. */
type NoArgs = Record<string, never>;

/**
 * Every command the backend registers (`tauri::generate_handler!` in
 * src-tauri/src/main.rs): the argument object the frontend sends, keyed by
 * the Rust parameter names in camelCase (an `Option` parameter may be
 * omitted or null), the value the command resolves with, and what it rejects
 * with: its error string, or a `BackendError` for a command that returns
 * `Result<T, CommandError>` (ADR 0006). Match a `BackendError` by `code`
 * only (`isBackendError`, src/lib/errors.ts), never by its message.
 */
export interface CommandMap {
  /** `meeting_reprocessing_commands::accept_meeting_proposal(proposal: MeetingReprocessingProposal, current_review: MeetingReview) -> Result<ReviewState, CommandError>` */
  accept_meeting_proposal: { args: { proposal: MeetingReprocessingProposal; currentReview: MeetingReview }; result: MeetingReviewState; error: BackendError };
  /** `commands::acceptance_preflight(expect_revision: String) -> Result<sagascript_core::acceptance::PreflightReport, CommandError>` */
  acceptance_preflight: { args: { expectRevision: string }; result: PreflightReport; error: BackendError };
  /** `commands::acknowledge_update_result(expected_text: String) -> Result<bool, CommandError>` */
  acknowledge_update_result: { args: { expectedText: string }; result: boolean; error: BackendError };
  /** `meeting_review_commands::apply_meeting_corrections(review: MeetingReview, corrections: CorrectionFile) -> Result<ReviewState, CommandError>` */
  apply_meeting_corrections: { args: { review: MeetingReview; corrections: CorrectionFile }; result: MeetingReviewState; error: BackendError };
  /** `commands::apply_training_glossary(heard: String, corrected: String, profile_id: String, accepted: Vec<GlossarySuggestion>) -> Result<(), CommandError>` */
  apply_training_glossary: { args: { heard: string; corrected: string; profileId: string; accepted: GlossarySuggestion[] }; result: void; error: BackendError };
  /** `meeting_media::attach_meeting_audio(source_sha256: String) -> Result<Option<AudioAttachment>, CommandError>` */
  attach_meeting_audio: { args: { sourceSha256: string }; result: MeetingAudioAttachment | null; error: BackendError };
  /** `meeting_jobs::begin_meeting_file(file_path: String, prompt: Option<String>, profile_id: Option<String>) -> Result<String, CommandError>` */
  begin_meeting_file: { args: { filePath: string; prompt?: string | null; profileId?: string | null }; result: string; error: BackendError };
  /** `meeting_jobs::begin_meeting_reprocessing(file_path: String, prompt: Option<String>, profile_id: Option<String>, request: ReprocessingRequest) -> Result<String, CommandError>` */
  begin_meeting_reprocessing: { args: { filePath: string; prompt?: string | null; profileId?: string | null; request: ReprocessingRequest }; result: string; error: BackendError };
  /** `commands::cancel_file_transcription(run_id: String) -> Result<bool, CommandError>` */
  cancel_file_transcription: { args: { runId: string }; result: boolean; error: BackendError };
  /** `meeting_jobs::cancel_meeting_job(job_id: String) -> Result<bool, CommandError>` */
  cancel_meeting_job: { args: { jobId: string }; result: boolean; error: BackendError };
  /** `commands::cancel_recording() -> Result<(), CommandError>` */
  cancel_recording: { args: NoArgs; result: void; error: BackendError };
  /** `commands::check_accessibility_permission() -> Result<bool, CommandError>` */
  check_accessibility_permission: { args: NoArgs; result: boolean; error: BackendError };
  /** `commands::clear_update_recovery() -> Result<(), CommandError>` */
  clear_update_recovery: { args: NoArgs; result: void; error: BackendError };
  /** `commands::complete_update_preparation(nonce: String, error: Option<String>) -> Result<(), CommandError>` */
  complete_update_preparation: { args: { nonce: string; error?: string | null }; result: void; error: BackendError };
  /** `commands::copy_transcription_text(text: String) -> Result<(), CommandError>` */
  copy_transcription_text: { args: { text: string }; result: void; error: BackendError };
  /** `meeting_review_commands::create_meeting_review(transcript: MeetingTranscript) -> Result<ReviewState, CommandError>` */
  create_meeting_review: { args: { transcript: MeetingTranscript }; result: MeetingReviewState; error: BackendError };
  /** `meeting_media::detach_meeting_audio(token: String) -> Result<(), CommandError>` */
  detach_meeting_audio: { args: { token: string }; result: void; error: BackendError };
  /** `commands::download_model(whisper_model: WhisperModel) -> Result<(), CommandError>` */
  download_model: { args: { whisperModel: string }; result: void; error: BackendError };
  /** `commands::download_pianissimo_model() -> Result<(), CommandError>` */
  download_pianissimo_model: { args: NoArgs; result: void; error: BackendError };
  /** `commands::engine_status() -> Result<EngineStatus, CommandError>` */
  engine_status: { args: NoArgs; result: EngineStatus; error: BackendError };
  /** `commands::get_active_hotkey_profile() -> Result<Option<HotkeyProfile>, CommandError>` */
  get_active_hotkey_profile: { args: NoArgs; result: HotkeyProfile | null; error: BackendError };
  /** `commands::get_auto_file_model_info(language: Language) -> Result<ModelInfo, CommandError>` */
  get_auto_file_model_info: { args: { language: Language }; result: WhisperModel; error: BackendError };
  /** `commands::get_build_info() -> Result<BuildInfo, CommandError>` */
  get_build_info: { args: NoArgs; result: BuildInfo; error: BackendError };
  /** `commands::get_dictation_model_info(language: Language) -> Result<ModelInfo, CommandError>` */
  get_dictation_model_info: { args: { language: Language }; result: WhisperModel; error: BackendError };
  /** `commands::get_effective_model_info(language: Language) -> Result<ModelInfo, CommandError>` */
  get_effective_model_info: { args: { language: Language }; result: WhisperModel; error: BackendError };
  /** `commands::get_engine_load_state() -> Result<serde_json::Value, CommandError>` */
  get_engine_load_state: { args: NoArgs; result: EngineLoadState; error: BackendError };
  /** `commands::get_file_model_options(language: Language, include: Option<String>) -> Result<Vec<ModelInfo>, CommandError>` */
  get_file_model_options: { args: { language: Language; include?: string | null }; result: WhisperModel[]; error: BackendError };
  /** `commands::get_last_error() -> Result<Option<String>, CommandError>` */
  get_last_error: { args: NoArgs; result: string | null; error: BackendError };
  /** `commands::get_last_transcription() -> Result<Option<String>, CommandError>` */
  get_last_transcription: { args: NoArgs; result: string | null; error: BackendError };
  /** `commands::get_loaded_model() -> Result<LoadedModelInfo, CommandError>` */
  get_loaded_model: { args: NoArgs; result: LoadedModelInfo; error: BackendError };
  /** `meeting_jobs::get_meeting_job(job_id: String) -> Result<MeetingSnapshot, CommandError>` */
  get_meeting_job: { args: { jobId: string }; result: MeetingJobSnapshot; error: BackendError };
  /** `commands::get_model_info() -> Result<Vec<ModelInfo>, CommandError>` */
  get_model_info: { args: NoArgs; result: WhisperModel[]; error: BackendError };
  /** `commands::get_platform() -> Result<String, CommandError>` */
  get_platform: { args: NoArgs; result: string; error: BackendError };
  /** `commands::get_profile_model_info(profile_id: String) -> Result<ModelInfo, CommandError>` */
  get_profile_model_info: { args: { profileId: string }; result: WhisperModel; error: BackendError };
  /** `commands::get_settings() -> Result<Settings, CommandError>` */
  get_settings: { args: NoArgs; result: Settings; error: BackendError };
  /** `commands::get_state() -> Result<AppState, CommandError>` */
  get_state: { args: NoArgs; result: AppState; error: BackendError };
  /** `commands::get_supported_formats() -> Result<Vec<String>, CommandError>` */
  get_supported_formats: { args: NoArgs; result: string[]; error: BackendError };
  /** `commands::get_update_result_pending(result_id: String) -> Result<bool, CommandError>` */
  get_update_result_pending: { args: { resultId: string }; result: boolean; error: BackendError };
  /** `commands::hotkey_status() -> Result<HotkeyStatus, CommandError>` */
  hotkey_status: { args: NoArgs; result: HotkeyStatus; error: BackendError };
  /** `commands::is_model_downloaded(whisper_model: WhisperModel) -> Result<bool, CommandError>` */
  is_model_downloaded: { args: { whisperModel: string }; result: boolean; error: BackendError };
  /** `commands::is_model_ready() -> Result<bool, CommandError>` */
  is_model_ready: { args: NoArgs; result: boolean; error: BackendError };
  /** `commands::load_update_recovery() -> Result<Option<serde_json::Value>, CommandError>` */
  load_update_recovery: { args: NoArgs; result: UpdateRecoveryPayload | null; error: BackendError };
  /** `meeting_jobs::merge_meeting_speakers(transcript: MeetingTranscript, from_id: String, into_id: String) -> Result<MeetingTranscript, CommandError>` */
  merge_meeting_speakers: { args: { transcript: MeetingTranscript; fromId: string; intoId: string }; result: MeetingTranscript; error: BackendError };
  /** `commands::microphone_status() -> Result<String, CommandError>` */
  microphone_status: { args: NoArgs; result: string; error: BackendError };
  /** `commands::open_accessibility_settings() -> Result<(), CommandError>` */
  open_accessibility_settings: { args: NoArgs; result: void; error: BackendError };
  /** `meeting_reprocessing_commands::open_meeting_proposal() -> Result<Option<ProposalState>, CommandError>` */
  open_meeting_proposal: { args: NoArgs; result: ProposalState | null; error: BackendError };
  /** `meeting_review_commands::open_meeting_review() -> Result<Option<ReviewState>, CommandError>` */
  open_meeting_review: { args: NoArgs; result: MeetingReviewState | null; error: BackendError };
  /** `commands::open_microphone_settings() -> Result<(), CommandError>` */
  open_microphone_settings: { args: NoArgs; result: void; error: BackendError };
  /** `meeting_reprocessing_commands::plan_meeting_reprocessing(previous: MeetingReview, mode: ReprocessingMode, threshold: f32, save_cache: bool, prompt: Option<String>, profile_id: Option<String>) -> Result<Option<SelectedReprocessingPlan>, CommandError>` */
  plan_meeting_reprocessing: { args: { previous: MeetingReview; mode: ReprocessingMode; threshold: number; saveCache: boolean; prompt?: string | null; profileId?: string | null }; result: SelectedReprocessingPlan | null; error: BackendError };
  /** `meeting_reprocessing_commands::preview_meeting_proposal(proposal: MeetingReprocessingProposal) -> Result<ProposalState, CommandError>` */
  preview_meeting_proposal: { args: { proposal: MeetingReprocessingProposal }; result: ProposalState; error: BackendError };
  /** `meeting_jobs::rename_meeting_speaker(transcript: MeetingTranscript, speaker_id: String, label: String) -> Result<MeetingTranscript, CommandError>` */
  rename_meeting_speaker: { args: { transcript: MeetingTranscript; speakerId: string; label: string }; result: MeetingTranscript; error: BackendError };
  /** `commands::request_accessibility_permission() -> Result<(), CommandError>` */
  request_accessibility_permission: { args: NoArgs; result: void; error: BackendError };
  /** `commands::request_microphone_access() -> Result<String, CommandError>` */
  request_microphone_access: { args: NoArgs; result: string; error: BackendError };
  /** `meeting_review_commands::reset_meeting_review(review: MeetingReview, expected_revision: String) -> Result<ReviewState, CommandError>` */
  reset_meeting_review: { args: { review: MeetingReview; expectedRevision: string }; result: MeetingReviewState; error: BackendError };
  /** `meeting_reprocessing_commands::resolve_meeting_proposal(proposal: MeetingReprocessingProposal, expected_revision: String, index: usize, operations: Vec<CorrectionOperation>) -> Result<ProposalState, CommandError>` */
  resolve_meeting_proposal: { args: { proposal: MeetingReprocessingProposal; expectedRevision: string; index: number; operations: CorrectionOperation[] }; result: ProposalState; error: BackendError };
  /** `commands::retry_hotkey_registration() -> Result<(), CommandError>` */
  retry_hotkey_registration: { args: NoArgs; result: void; error: BackendError };
  /** `meeting_jobs::save_meeting_export(transcript: MeetingTranscript, format: ExportFormat) -> Result<bool, CommandError>` */
  save_meeting_export: { args: { transcript: MeetingTranscript; format: MeetingExportFormat }; result: boolean; error: BackendError };
  /** `meeting_reprocessing_commands::save_meeting_proposal(proposal: MeetingReprocessingProposal) -> Result<bool, CommandError>` */
  save_meeting_proposal: { args: { proposal: MeetingReprocessingProposal }; result: boolean; error: BackendError };
  /** `meeting_review_commands::save_meeting_review(review: MeetingReview, format: ExportFormat) -> Result<bool, CommandError>` */
  save_meeting_review: { args: { review: MeetingReview; format: MeetingExportFormat }; result: boolean; error: BackendError };
  /** `commands::save_transcription_text(text: String, file_name: String, directory: Option<String>) -> Result<bool, CommandError>` */
  save_transcription_text: { args: { text: string; fileName: string; directory?: string | null }; result: boolean; error: BackendError };
  /** `commands::save_update_recovery(payload: serde_json::Value) -> Result<(), CommandError>` */
  save_update_recovery: { args: { payload: UpdateRecoveryPayload }; result: void; error: BackendError };
  /** `commands::set_auto_paste(enabled: bool) -> Result<(), CommandError>` */
  set_auto_paste: { args: { enabled: boolean }; result: void; error: BackendError };
  /** `commands::set_auto_select_model(enabled: bool) -> Result<(), CommandError>` */
  set_auto_select_model: { args: { enabled: boolean }; result: void; error: BackendError };
  /** `commands::set_beam_size(beam_size: u32) -> Result<(), CommandError>` */
  set_beam_size: { args: { beamSize: number }; result: void; error: BackendError };
  /** `commands::set_engine_idle_unload_minutes(minutes: u32) -> Result<(), CommandError>` */
  set_engine_idle_unload_minutes: { args: { minutes: number }; result: void; error: BackendError };
  /** `commands::set_engine_prewarm(mode: String) -> Result<(), CommandError>` */
  set_engine_prewarm: { args: { mode: string }; result: void; error: BackendError };
  /** `commands::set_file_transcription_model(model_id: String) -> Result<(), CommandError>` */
  set_file_transcription_model: { args: { modelId: string }; result: void; error: BackendError };
  /** `commands::set_hotkey(shortcut: String) -> Result<(), CommandError>` */
  set_hotkey: { args: { shortcut: string }; result: void; error: BackendError };
  /** `commands::set_hotkey_mode(mode: HotkeyMode) -> Result<(), CommandError>` */
  set_hotkey_mode: { args: { mode: HotkeyMode }; result: void; error: BackendError };
  /** `commands::set_hotkey_profiles(profiles: Vec<HotkeyProfile>) -> Result<Vec<String>, CommandError>` */
  set_hotkey_profiles: { args: { profiles: HotkeyProfile[] }; result: string[]; error: BackendError };
  /** `commands::set_initial_prompt(prompt: String, expected_source: Option<String>) -> Result<(), CommandError>` */
  set_initial_prompt: { args: { prompt: string; expectedSource?: string | null }; result: void; error: BackendError };
  /** `commands::set_language(language: Language) -> Result<Vec<String>, CommandError>` */
  set_language: { args: { language: Language }; result: string[]; error: BackendError };
  /** `commands::set_onboarding_completed() -> Result<(), CommandError>` */
  set_onboarding_completed: { args: NoArgs; result: void; error: BackendError };
  /** `commands::set_pianissimo_dictation(enabled: bool) -> Result<(), CommandError>` */
  set_pianissimo_dictation: { args: { enabled: boolean }; result: void; error: BackendError };
  /** `commands::set_profile_glossary(profile_id: String, source: String, expected_source: Option<String>) -> Result<(), CommandError>` */
  set_profile_glossary: { args: { profileId: string; source: string; expectedSource?: string | null }; result: void; error: BackendError };
  /** `commands::set_profile_model(profile_id: String, model_id: String) -> Result<(), CommandError>` */
  set_profile_model: { args: { profileId: string; modelId: string }; result: void; error: BackendError };
  /** `commands::set_show_overlay(enabled: bool) -> Result<(), CommandError>` */
  set_show_overlay: { args: { enabled: boolean }; result: void; error: BackendError };
  /** `commands::set_temperature_fallback(enabled: bool) -> Result<(), CommandError>` */
  set_temperature_fallback: { args: { enabled: boolean }; result: void; error: BackendError };
  /** `commands::set_update_result_pending(result_id: String, pending: bool) -> Result<(), CommandError>` */
  set_update_result_pending: { args: { resultId: string; pending: boolean }; result: void; error: BackendError };
  /** `commands::set_vad_enabled(enabled: bool) -> Result<(), CommandError>` */
  set_vad_enabled: { args: { enabled: boolean }; result: void; error: BackendError };
  /** `commands::set_whisper_model(model: WhisperModel) -> Result<(), CommandError>` */
  set_whisper_model: { args: { model: string }; result: void; error: BackendError };
  /** `commands::start_recording() -> Result<(), CommandError>` */
  start_recording: { args: NoArgs; result: void; error: BackendError };
  /** `commands::start_training_recording(profile_id: String) -> Result<(), CommandError>` */
  start_training_recording: { args: { profileId: string }; result: void; error: BackendError };
  /** `commands::stop_and_transcribe() -> Result<String, CommandError>` */
  stop_and_transcribe: { args: NoArgs; result: string; error: BackendError };
  /** `commands::stop_and_transcribe_training() -> Result<TrainingTranscript, CommandError>` */
  stop_and_transcribe_training: { args: NoArgs; result: TrainingTranscript; error: BackendError };
  /** `commands::suggest_training_glossary(heard: String, corrected: String, profile_id: String) -> Result<Vec<GlossarySuggestion>, CommandError>` */
  suggest_training_glossary: { args: { heard: string; corrected: string; profileId: string }; result: GlossarySuggestion[]; error: BackendError };
  /** `commands::transcribe_file(file_path: String, prompt: Option<String>, diarize: Option<bool>, profile_id: Option<String>, run_id: Option<String>, auto_paste: Option<bool>) -> Result<String, CommandError>` */
  transcribe_file: { args: { filePath: string; prompt?: string | null; diarize?: boolean | null; profileId?: string | null; runId?: string | null; autoPaste?: boolean | null }; result: string; error: BackendError };
  /** `commands::transcribe_training_file(file_path: String, profile_id: String) -> Result<TrainingTranscript, CommandError>` */
  transcribe_training_file: { args: { filePath: string; profileId: string }; result: TrainingTranscript; error: BackendError };
  /** `meeting_review_commands::undo_meeting_review(review: MeetingReview, expected_revision: String) -> Result<ReviewState, CommandError>` */
  undo_meeting_review: { args: { review: MeetingReview; expectedRevision: string }; result: MeetingReviewState; error: BackendError };
}

/** The registered command names, sorted. */
export const COMMAND_NAMES = [
  "accept_meeting_proposal",
  "acceptance_preflight",
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
 * The commands that reject with a `BackendError` (`Result<T, CommandError>`);
 * every other command rejects with a string.
 */
export const BACKEND_ERROR_COMMANDS = [
  "accept_meeting_proposal",
  "acceptance_preflight",
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
  acceptance_preflight: { required: ["expectRevision"], optional: [] },
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
