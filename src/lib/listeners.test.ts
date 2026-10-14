// Every window (Settings, the dictation indicator, Onboarding) registers
// Tauri event listeners. Unmounting one must remove all of them (#362): a
// listener that outlives its component keeps writing into dead state and, on a
// hot reload or a second mount, runs twice.
import { mockWindows } from "@tauri-apps/api/mocks";
import { render, waitFor } from "@testing-library/svelte";
import { describe, expect, test, vi } from "vitest";
import Onboarding from "./Onboarding.svelte";
import Overlay from "./Overlay.svelte";
import { mockIpc } from "../test/ipc";
import { renderSettings, settingsValue } from "../test/settings";

/** What the webview reports for drag and drop: one registration per phase. */
const DRAG_DROP = ["tauri://drag-drop", "tauri://drag-enter", "tauri://drag-leave", "tauri://drag-over"];

describe("unmounting removes every event listener", () => {
  test("Settings", async () => {
    const { ipc, view } = await renderSettings();
    await waitFor(() =>
      expect(ipc.listeners()).toEqual(
        [
          "error",
          "hotkey-registration-changed",
          "model-download-progress",
          "model-ready",
          "navigate_tab",
          "state-changed", // the Dictate button, its result and the settings reload
          "transcription-result",
          "update-aborted",
          "update-preparing",
          ...DRAG_DROP,
        ].sort(),
      ),
    );

    view.unmount();

    await waitFor(() => expect(ipc.listeners()).toEqual([]));
  });

  test("the dictation indicator", async () => {
    const ipc = mockIpc({
      get_active_hotkey_profile: () => settingsValue().hotkey_profiles[0],
      get_state: () => "idle",
      get_engine_load_state: () => ({ state: "unknown" }),
    });
    const view = render(Overlay);
    await waitFor(() => expect(ipc.listeners()).toEqual(["active-hotkey-profile-changed", "engine-load-state", "state-changed"]));
    await waitFor(() => expect(ipc.callsTo("get_engine_load_state").length).toBe(1));

    view.unmount();

    await waitFor(() => expect(ipc.listeners()).toEqual([]));
  });

  test("Onboarding", async () => {
    const ipc = mockIpc({
      get_platform: () => "windows",
      microphone_status: () => "unsupported",
      check_accessibility_permission: () => true,
      get_settings: () => settingsValue({ has_completed_onboarding: false }),
    });
    const view = render(Onboarding, { oncomplete: vi.fn() });
    await waitFor(() => expect(ipc.listeners()).toEqual(["model-download-progress", "model-ready"]));
    await waitFor(() => expect(ipc.callsTo("get_settings").length).toBe(1));

    view.unmount();

    await waitFor(() => expect(ipc.listeners()).toEqual([]));
  });

  test("a component unmounted while its listeners are still registering leaves none behind", async () => {
    mockWindows("main");
    const ipc = mockIpc({ get_platform: () => "windows", microphone_status: () => "unsupported" });
    // Unmount in the same tick as mount, before any registration settled.
    render(Onboarding, { oncomplete: vi.fn() }).unmount();

    await new Promise((resolve) => setTimeout(resolve, 50));
    expect(ipc.listeners()).toEqual([]);
  });
});
