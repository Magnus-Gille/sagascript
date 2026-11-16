// Render one panel of the Settings window on its own (SAGA-379): the panel
// over the window's real controllers and the typed IPC mock, without the shell
// (tab strip, banners, notices) around it. settings.ts renders the whole window.
import { mockWindows } from "@tauri-apps/api/mocks";
import { render, waitFor } from "@testing-library/svelte";
import type { Component } from "svelte";
import { expect } from "vitest";
import type { SettingsWindow } from "../lib/settings/settingsWindow.svelte";
import type { SettingsPanelProps } from "../lib/settings/tabs";
import { mockIpc, type CommandHandlers, type IpcMock } from "./ipc";
import { settingsBackend } from "./settings";
import SettingsPanelHarness from "./SettingsPanelHarness.svelte";

export interface RenderedPanel {
  ipc: IpcMock;
  /**
   * The window the panel renders from. For what reaches a panel from outside
   * it (`win.router.request(...)`, `win.queue.addFiles(...)`) and for what the
   * shell shows and the panel does not (`win.errors.of("settings").message`,
   * the tab's error).
   */
  win: SettingsWindow;
  view: ReturnType<typeof render<typeof SettingsPanelHarness>>;
  /** Show or hide the panel's tab, as the window does when the tab changes. */
  setActive(active: boolean): Promise<void>;
}

/**
 * Render `panel` against `handlers` and wait until the window has mounted:
 * settings loaded, every event listener registered and the last dictation
 * result fetched (the same point renderSettings waits for).
 */
export async function renderPanel(
  panel: Component<SettingsPanelProps>,
  handlers: CommandHandlers = settingsBackend(),
  { active = true }: { active?: boolean } = {},
): Promise<RenderedPanel> {
  const ipc = mockIpc(handlers);
  // getCurrentWebview() (drag and drop) reads the window label from here.
  mockWindows("main");
  let win!: SettingsWindow;
  const view = render(SettingsPanelHarness, {
    panel,
    active,
    onWindow: (created: SettingsWindow) => {
      win = created;
    },
  });
  await waitFor(() => {
    for (const event of ["update-preparing", "update-aborted", "transcription-result", "state-changed", "error"] as const) {
      expect(ipc.listenerCount(event)).toBeGreaterThan(0);
    }
    expect(ipc.callsTo("hotkey_status").length).toBeGreaterThan(0);
    expect(ipc.callsTo("get_last_transcription").length).toBeGreaterThan(0);
  });
  return { ipc, win, view, setActive: (next) => view.rerender({ active: next }) };
}
