import { invoke } from "@tauri-apps/api/core";

export type Language = "en" | "sv" | "auto";
export type TranscriptionBackend = "local" | "remote";
export type HotkeyMode = "push" | "toggle";

export interface WhisperModel {
  id: string;
  display_name: string;
  description: string;
  size_mb: number;
  downloaded: boolean;
  english_only: boolean;
  swedish_optimized: boolean;
}

export interface Settings {
  language: Language;
  backend: TranscriptionBackend;
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

export async function setBackend(backend: TranscriptionBackend): Promise<void> {
  return invoke("set_backend", { backend });
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

export async function saveApiKey(key: string): Promise<boolean> {
  return invoke("save_api_key", { key });
}

export async function hasApiKey(): Promise<boolean> {
  return invoke("has_api_key");
}

export async function deleteApiKey(): Promise<boolean> {
  return invoke("delete_api_key");
}

export async function getModelInfo(): Promise<WhisperModel[]> {
  return invoke("get_model_info");
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
