import { invoke } from "@tauri-apps/api/core";

export type Language = "en" | "sv" | "no" | "auto";
export type HotkeyMode = "push" | "toggle";

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
  show_overlay: boolean;
  auto_paste: boolean;
  auto_select_model: boolean;
  hotkey: string;
  initial_prompt: string;
  beam_size: number;
  temperature_fallback: boolean;
  vad_enabled: boolean;
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
}

export async function getState(): Promise<AppState> {
  return invoke("get_state");
}

export async function getSettings(): Promise<Settings> {
  return invoke("get_settings");
}

export async function updateSettings(settings: Settings): Promise<void> {
  return invoke("update_settings", { settings });
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

export async function setHotkey(shortcut: string): Promise<void> {
  return invoke("set_hotkey", { shortcut });
}

/** Whether the hotkey is actually registered right now (not just the saved
 * setting) — reads the backend's process-wide registration-health flag. */
export async function hotkeyStatus(): Promise<HotkeyStatus> {
  return invoke("hotkey_status");
}

export async function setAutoPaste(enabled: boolean): Promise<void> {
  return invoke("set_auto_paste", { enabled });
}

export async function setInitialPrompt(prompt: string): Promise<void> {
  return invoke("set_initial_prompt", { prompt });
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
  options?: { prompt?: string; diarize?: boolean }
): Promise<string> {
  return invoke("transcribe_file", {
    filePath,
    prompt: options?.prompt ?? null,
    diarize: options?.diarize ?? false,
  });
}

export async function getSupportedFormats(): Promise<string[]> {
  return invoke("get_supported_formats");
}

export async function startRecording(): Promise<void> {
  return invoke("start_recording");
}

export async function stopAndTranscribe(): Promise<string> {
  return invoke("stop_and_transcribe");
}

// -- Permission / platform queries (for onboarding) --

export async function checkAccessibilityPermission(): Promise<boolean> {
  return invoke("check_accessibility_permission");
}

export async function requestAccessibilityPermission(): Promise<void> {
  return invoke("request_accessibility_permission");
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
