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

export async function setAutoPaste(enabled: boolean): Promise<void> {
  return invoke("set_auto_paste", { enabled });
}

export async function setShowOverlay(enabled: boolean): Promise<void> {
  return invoke("set_show_overlay", { enabled });
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

export async function transcribeFile(filePath: string): Promise<string> {
  return invoke("transcribe_file", { filePath });
}

export async function getSupportedFormats(): Promise<string[]> {
  return invoke("get_supported_formats");
}

// -- Permission / platform queries (for onboarding) --

export async function checkAccessibilityPermission(): Promise<boolean> {
  return invoke("check_accessibility_permission");
}

export async function requestAccessibilityPermission(): Promise<void> {
  return invoke("request_accessibility_permission");
}

export async function checkMicrophonePermission(): Promise<boolean> {
  return invoke("check_microphone_permission");
}

export async function requestMicrophonePermission(): Promise<boolean> {
  return invoke("request_microphone_permission");
}

export async function getPlatform(): Promise<string> {
  return invoke("get_platform");
}
