import { invoke } from "@tauri-apps/api/core";
import type { MeetingExportFormat, MeetingTranscript } from "./meeting-types";

export type Language = "en" | "sv" | "no" | "fi" | "auto";
export type HotkeyMode = "push" | "toggle" | "presenter";

export type PresenterFinishAction = "insert_only" | "return" | "command_return";

export interface PresenterConfig {
  finish_shortcut: string;
  cancel_shortcut: string | null;
  app_actions: Record<string, PresenterFinishAction>;
}

export interface HotkeyProfile {
  id: string;
  name: string;
  shortcut: string;
  language: Language;
}

export interface WhisperModel {
  id: string;
  display_name: string;
  description: string;
  size_mb: number;
  downloaded: boolean;
  active: boolean;
}

export interface Settings {
  language: Language;
  whisper_model: string;
  hotkey_mode: HotkeyMode;
  presenter: PresenterConfig;
  show_overlay: boolean;
  auto_paste: boolean;
  auto_select_model: boolean;
  hotkey: string;
  hotkey_profiles: HotkeyProfile[];
  initial_prompt: string;
  profile_glossaries: Record<string, string>;
  beam_size: number;
  temperature_fallback: boolean;
  vad_enabled: boolean;
  has_completed_onboarding: boolean;
}

export interface BuildInfo {
  version: string;
  git_hash: string;
  build_date: string;
}

export interface LoadedModelInfo {
  effective_model: string;
  effective_model_id: string;
  loaded_model: string | null;
  is_loaded: boolean;
  is_downloaded: boolean;
}

export type AppState = "idle" | "recording" | "transcribing" | "error";

export interface HotkeyStatus {
  ok: boolean;
  error: string | null;
  shortcut: string;
  shortcuts: string[];
}

export interface TrainingTranscript {
  raw_text: string;
  effective_text: string;
}

export interface GlossarySuggestion {
  observed: string;
  canonical: string;
  kind: "alias" | "hint_only";
  context: string;
}

export type MeetingJobStatus = "running" | "cancelling" | "completed" | "cancelled" | "failed";

export interface MeetingJobSnapshot {
  id: string;
  status: MeetingJobStatus;
  phase: string;
  error: string | null;
  transcript: MeetingTranscript | null;
}

export async function getState(): Promise<AppState> {
  return invoke("get_state");
}

export async function getLastError(): Promise<string | null> {
  return invoke("get_last_error");
}

export async function getLastTranscription(): Promise<string | null> {
  return invoke("get_last_transcription");
}

export async function getSettings(): Promise<Settings> {
  return invoke("get_settings");
}

export async function setLanguage(language: Language): Promise<void> {
  return invoke("set_language", { language });
}

export async function setWhisperModel(model: string): Promise<void> {
  return invoke("set_whisper_model", { model });
}

export async function setAutoSelectModel(enabled: boolean): Promise<void> {
  return invoke("set_auto_select_model", { enabled });
}

export async function setHotkeyMode(mode: HotkeyMode): Promise<void> {
  return invoke("set_hotkey_mode", { mode });
}

export async function setPresenterConfig(config: PresenterConfig): Promise<void> {
  return invoke("set_presenter_config", { config });
}

export async function setHotkey(shortcut: string): Promise<void> {
  return invoke("set_hotkey", { shortcut });
}

export async function setHotkeyProfiles(profiles: HotkeyProfile[]): Promise<void> {
  return invoke("set_hotkey_profiles", { profiles });
}

export async function getActiveHotkeyProfile(): Promise<HotkeyProfile | null> {
  return invoke("get_active_hotkey_profile");
}

/** Whether the hotkey is actually registered right now (not just the saved
 * setting) — reads the backend's process-wide registration-health flag. */
export async function hotkeyStatus(): Promise<HotkeyStatus> {
  return invoke("hotkey_status");
}

/** Retry the shortcuts persisted by either the GUI or CLI. */
export async function retryHotkeyRegistration(): Promise<void> {
  return invoke("retry_hotkey_registration");
}

export async function setAutoPaste(enabled: boolean): Promise<void> {
  return invoke("set_auto_paste", { enabled });
}

export async function setInitialPrompt(prompt: string, expectedSource?: string): Promise<void> {
  return invoke("set_initial_prompt", { prompt, expectedSource });
}

export async function setProfileGlossary(profileId: string, source: string, expectedSource?: string): Promise<void> {
  return invoke("set_profile_glossary", { profileId, source, expectedSource });
}

export async function setShowOverlay(enabled: boolean): Promise<void> {
  return invoke("set_show_overlay", { enabled });
}

export async function setBeamSize(beamSize: number): Promise<void> {
  return invoke("set_beam_size", { beamSize });
}

export async function setTemperatureFallback(enabled: boolean): Promise<void> {
  return invoke("set_temperature_fallback", { enabled });
}

export async function setVadEnabled(enabled: boolean): Promise<void> {
  return invoke("set_vad_enabled", { enabled });
}

export async function getModelInfo(): Promise<WhisperModel[]> {
  return invoke("get_model_info");
}

export async function getEffectiveModelInfo(language: Language): Promise<WhisperModel> {
  return invoke("get_effective_model_info", { language });
}

export async function getLoadedModel(): Promise<LoadedModelInfo> {
  return invoke("get_loaded_model");
}

export async function isModelReady(): Promise<boolean> {
  return invoke("is_model_ready");
}

export async function isModelDownloaded(whisperModel: string): Promise<boolean> {
  return invoke("is_model_downloaded", { whisperModel });
}

export async function downloadModel(whisperModel: string): Promise<void> {
  return invoke("download_model", { whisperModel });
}

export async function getBuildInfo(): Promise<BuildInfo> {
  return invoke("get_build_info");
}

export async function transcribeFile(
  filePath: string,
  options?: { prompt?: string; diarize?: boolean; profileId?: string }
): Promise<string> {
  return invoke("transcribe_file", {
    filePath,
    prompt: options?.prompt ?? null,
    diarize: options?.diarize ?? false,
    profileId: options?.profileId ?? null,
  });
}

export async function beginMeetingFile(
  filePath: string,
  prompt: string | null,
  profileId: string | null,
): Promise<string> {
  return invoke("begin_meeting_file", { filePath, prompt, profileId });
}

export async function getMeetingJob(jobId: string): Promise<MeetingJobSnapshot> {
  return invoke("get_meeting_job", { jobId });
}

export async function cancelMeetingJob(jobId: string): Promise<boolean> {
  return invoke("cancel_meeting_job", { jobId });
}

export async function renameMeetingSpeaker(
  transcript: MeetingTranscript,
  speakerId: string,
  label: string,
): Promise<MeetingTranscript> {
  return invoke("rename_meeting_speaker", { transcript, speakerId, label });
}

export async function mergeMeetingSpeakers(
  transcript: MeetingTranscript,
  fromId: string,
  intoId: string,
): Promise<MeetingTranscript> {
  return invoke("merge_meeting_speakers", { transcript, fromId, intoId });
}

export async function saveMeetingExport(
  transcript: MeetingTranscript,
  format: MeetingExportFormat,
): Promise<boolean> {
  return invoke("save_meeting_export", { transcript, format });
}

export async function getSupportedFormats(): Promise<string[]> {
  return invoke("get_supported_formats");
}

export async function startRecording(): Promise<void> {
  return invoke("start_recording");
}

export async function startTrainingRecording(profileId: string): Promise<void> {
  return invoke("start_training_recording", { profileId });
}

export async function cancelRecording(): Promise<void> {
  return invoke("cancel_recording");
}

export async function stopAndTranscribe(): Promise<string> {
  return invoke("stop_and_transcribe");
}

export async function stopAndTranscribeTraining(): Promise<TrainingTranscript> {
  return invoke("stop_and_transcribe_training");
}

export async function transcribeTrainingFile(
  filePath: string,
  profileId: string
): Promise<TrainingTranscript> {
  return invoke("transcribe_training_file", { filePath, profileId });
}

export async function suggestTrainingGlossary(
  heard: string,
  corrected: string,
  profileId: string
): Promise<GlossarySuggestion[]> {
  return invoke("suggest_training_glossary", { heard, corrected, profileId });
}

export async function applyTrainingGlossary(
  heard: string,
  corrected: string,
  profileId: string,
  accepted: GlossarySuggestion[]
): Promise<void> {
  return invoke("apply_training_glossary", { heard, corrected, profileId, accepted });
}

// -- Permission / platform queries (for onboarding) --

export async function checkAccessibilityPermission(): Promise<boolean> {
  return invoke("check_accessibility_permission");
}

export async function requestAccessibilityPermission(): Promise<void> {
  return invoke("request_accessibility_permission");
}

export async function openAccessibilitySettings(): Promise<void> {
  return invoke("open_accessibility_settings");
}

export async function microphoneStatus(): Promise<string> {
  return invoke("microphone_status");
}

export async function requestMicrophoneAccess(): Promise<string> {
  return invoke("request_microphone_access");
}

export async function openMicrophoneSettings(): Promise<void> {
  return invoke("open_microphone_settings");
}

export async function getPlatform(): Promise<string> {
  return invoke("get_platform");
}

export async function setOnboardingCompleted(): Promise<void> {
  return invoke("set_onboarding_completed");
}
