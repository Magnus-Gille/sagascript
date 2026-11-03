// A backend for rendering Settings.svelte in component tests (#361): the
// answers to every command Settings invokes while it mounts, as plain
// fixtures. Tests override single handlers through mockIpc's `handlers`
// argument or `ipc.handle()`.
import { mockWindows } from "@tauri-apps/api/mocks";
import { render, waitFor } from "@testing-library/svelte";
import { expect } from "vitest";
import { PIANISSIMO_ID } from "../lib/models.svelte";
import Settings from "../lib/Settings.svelte";
import type { BuildInfo, EngineStatus, HotkeyProfile, HotkeyStatus, Settings as SettingsValue, WhisperModel } from "../lib/api";
import { mockIpc, type CommandHandlers, type IpcMock } from "./ipc";

export const buildInfo: BuildInfo = { version: "1.4.4", git_hash: "a69d5d1", build_date: "2026-10-13" };

export const engineStatus: EngineStatus = {
  installed: true,
  supported: true,
  host_path: "/Applications/Sagascript.app/Contents/Resources/EngineHost/sagascript-engine-host",
  host_version: "0.9.0",
  host_git_sha: "0123456789abcdef",
  warm: false,
};

export function whisperModel(id: string, displayName: string, overrides: Partial<WhisperModel> = {}): WhisperModel {
  return { id, display_name: displayName, description: "", size_mb: 0, downloaded: true, active: false, ...overrides };
}

export const pianissimo = whisperModel(PIANISSIMO_ID, "Pianissimo", { size_mb: 640 });
export const kbWhisperMedium = whisperModel("kb-whisper-medium", "KB-Whisper Medium", { size_mb: 1530 });

export const swedishProfile: HotkeyProfile = {
  id: "default",
  name: "Svenska",
  shortcut: "Ctrl+Shift+Space",
  language: "sv",
  push_to_talk_shortcut: "Ctrl+Shift+Space",
  toggle_shortcut: null,
};

export function settingsValue(overrides: Partial<SettingsValue> = {}): SettingsValue {
  return {
    language: "sv",
    whisper_model: "auto",
    file_transcription_model: "auto",
    pianissimo_dictation: false,
    hotkey_mode: "push",
    show_overlay: true,
    auto_paste: false,
    auto_select_model: true,
    hotkey: "Ctrl+Shift+Space",
    hotkey_profiles: [swedishProfile],
    profile_models: {},
    initial_prompt: "",
    profile_glossaries: {},
    beam_size: 5,
    temperature_fallback: true,
    vad_enabled: true,
    engine_prewarm: "on_key_down",
    engine_idle_unload_minutes: 10,
    has_completed_onboarding: true,
    ...overrides,
  };
}

const hotkeyRegistered: HotkeyStatus = {
  ok: true,
  error: null,
  shortcut: "Ctrl+Shift+Space",
  shortcuts: ["Ctrl+Shift+Space"],
  settings_watcher: { running: true, recovered_panics: 0 },
};

/** Every command Settings invokes while it mounts, answered for a Swedish profile on Windows. */
export function settingsBackend(overrides: CommandHandlers = {}): CommandHandlers {
  return {
    get_build_info: () => buildInfo,
    engine_status: () => engineStatus,
    get_settings: () => settingsValue(),
    get_platform: () => "windows",
    get_supported_formats: () => ["wav", "mp3", "m4a"],
    hotkey_status: () => hotkeyRegistered,
    get_profile_model_info: () => pianissimo,
    get_file_model_options: () => [pianissimo, kbWhisperMedium],
    get_effective_model_info: () => pianissimo,
    get_auto_file_model_info: () => pianissimo,
    load_update_recovery: () => null,
    get_last_error: () => null,
    get_last_transcription: () => null,
    get_update_result_pending: () => false,
    ...overrides,
  };
}

export interface RenderedSettings {
  ipc: IpcMock;
  view: ReturnType<typeof render<typeof Settings>>;
}

/**
 * Render Settings against `handlers` and wait until it has mounted: settings
 * loaded and every event listener registered. Unless `restored` is false (a
 * test that holds the startup recovery read), also until the last dictation
 * result was fetched, which Settings does after restoring recovery drafts.
 */
export async function renderSettings(
  handlers: CommandHandlers = settingsBackend(),
  { restored = true }: { restored?: boolean } = {},
): Promise<RenderedSettings> {
  const ipc = mockIpc(handlers);
  // getCurrentWebview() (drag and drop) reads the window label from here.
  mockWindows("main");
  const view = render(Settings);
  await waitFor(() => {
    for (const event of ["update-preparing", "update-aborted", "transcription-result", "state-changed", "error"] as const) {
      expect(ipc.listenerCount(event)).toBeGreaterThan(0);
    }
    expect(ipc.callsTo("hotkey_status").length).toBeGreaterThan(0);
    if (restored) expect(ipc.callsTo("get_last_transcription").length).toBeGreaterThan(0);
  });
  return { ipc, view };
}
