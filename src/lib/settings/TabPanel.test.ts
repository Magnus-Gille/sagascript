// TabPanel on its own (SAGA-379): a tab's panel exists only while its tab is
// the router's, unless the tab is kept mounted, and then it is told whether it
// shows. The same area through the whole window: src/lib/Settings.test.ts and
// src/lib/Settings.downloads.test.ts.
import { render, screen, waitFor } from "@testing-library/svelte";
import { mockWindows } from "@tauri-apps/api/mocks";
import { expect, test } from "vitest";
import { mockIpc } from "../../test/ipc";
import { settingsBackend } from "../../test/settings";
import ActiveValuePanel from "../../test/ActiveValuePanel.svelte";
import TabPanelHarness from "../../test/TabPanelHarness.svelte";
import type { SettingsWindow } from "./settingsWindow.svelte";
import type { SettingsTabDefinition } from "./tabs";

const IDS = ["first", "second"];

async function renderTab(tab: Omit<SettingsTabDefinition, "id" | "label" | "panel">) {
  const ipc = mockIpc(settingsBackend());
  mockWindows("main");
  let win!: SettingsWindow;
  render(TabPanelHarness, {
    tab: { id: "second", label: "Second", panel: ActiveValuePanel, ...tab },
    ids: IDS,
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
  return win;
}

test("a tab that is not kept mounted renders its panel only while its tab is the router's", async () => {
  const win = await renderTab({});
  expect(win.router.active).toBe("first");
  expect(screen.queryByText("Fixture panel content")).toBeNull();

  win.router.request("second");
  expect(await screen.findByText("Fixture panel content")).toBeInTheDocument();
  expect(screen.getByText("active is true")).toBeInTheDocument();

  win.router.request("first");
  await waitFor(() => expect(screen.queryByText("Fixture panel content")).toBeNull());
});

test("a tab that is kept mounted renders its panel always, active only while its tab is the router's", async () => {
  const win = await renderTab({ keepMounted: true });
  expect(win.router.active).toBe("first");
  expect(screen.getByText("Fixture panel content")).toBeInTheDocument();
  expect(screen.getByText("active is false")).toBeInTheDocument();

  win.router.request("second");
  expect(await screen.findByText("active is true")).toBeInTheDocument();

  win.router.request("first");
  expect(await screen.findByText("active is false")).toBeInTheDocument();
  expect(screen.getByText("Fixture panel content")).toBeInTheDocument();
});
