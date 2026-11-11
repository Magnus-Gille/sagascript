// The Settings window's tabs (SAGA-379): the strip, the tray's navigate_tab
// event, the `?tab=` of the window's URL, and a panel added as one component
// and one entry in the tab list.
import { mockWindows } from "@tauri-apps/api/mocks";
import { render, screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { afterEach, describe, expect, test } from "vitest";
import ExtraPanel from "../test/ExtraPanel.svelte";
import { settle } from "../test/deferred";
import { mockIpc } from "../test/ipc";
import { renderSettings, settingsBackend } from "../test/settings";
import Settings from "./Settings.svelte";
import { SETTINGS_TABS } from "./settings/tabs";

/** The buttons of the tab strip, in order: the strip is the parent of the Dictate button. */
function stripLabels(): string[] {
  const strip = screen.getByRole("button", { name: "Dictate" }).parentElement!;
  return Array.from(strip.querySelectorAll("button"), (button) => button.textContent?.trim() ?? "");
}

const dictate = () => screen.queryByRole("button", { name: "Start recording" });
const transcribe = () => screen.queryByRole("region", { name: "File transcription" });
const settings = () => screen.queryByRole("textbox", { name: "Personal dictionary" });
const showing = () => ({ dictate: dictate() !== null, transcribe: transcribe() !== null, settings: settings() !== null });

describe("the tab strip", () => {
  test("has the three tabs, in order", async () => {
    await renderSettings();
    expect(stripLabels()).toEqual(["Dictate", "Transcribe", "Settings"]);
  });
});

describe("the tray's navigate_tab event", () => {
  test("with an unknown name changes nothing", async () => {
    const { ipc } = await renderSettings();
    await screen.findByRole("button", { name: "Start recording" });
    ipc.emit("navigate_tab", "nowhere");
    ipc.emit("navigate_tab", "");
    await settle();
    expect(showing()).toEqual({ dictate: true, transcribe: false, settings: false });
  });

  test("with each known id shows that tab", async () => {
    const { ipc } = await renderSettings();

    ipc.emit("navigate_tab", "transcribe");
    await waitFor(() => expect(showing()).toEqual({ dictate: false, transcribe: true, settings: false }));

    ipc.emit("navigate_tab", "settings");
    await waitFor(() => expect(showing()).toEqual({ dictate: false, transcribe: false, settings: true }));

    ipc.emit("navigate_tab", "dictate");
    await waitFor(() => expect(showing()).toEqual({ dictate: true, transcribe: false, settings: false }));
  });
});

describe("the ?tab= of the window's URL", () => {
  afterEach(() => window.history.replaceState(null, "", "/"));

  test("opens the tab it names", async () => {
    window.history.replaceState(null, "", "/?tab=settings");
    await renderSettings();
    await waitFor(() => expect(showing()).toEqual({ dictate: false, transcribe: false, settings: true }));
  });

  test("with an unknown name leaves the first tab showing", async () => {
    window.history.replaceState(null, "", "/?tab=nowhere");
    await renderSettings();
    await screen.findByRole("button", { name: "Start recording" });
    await settle();
    expect(showing()).toEqual({ dictate: true, transcribe: false, settings: false });
  });
});

describe("a panel added to the tab list", () => {
  async function renderWithExtra() {
    const ipc = mockIpc(settingsBackend());
    mockWindows("main");
    render(Settings, { tabs: [...SETTINGS_TABS, { id: "extra", label: "Extra", panel: ExtraPanel }] });
    await waitFor(() => {
      for (const event of ["update-preparing", "update-aborted", "transcription-result", "state-changed", "error"] as const) {
        expect(ipc.listenerCount(event)).toBeGreaterThan(0);
      }
      expect(ipc.callsTo("hotkey_status").length).toBeGreaterThan(0);
      expect(ipc.callsTo("get_last_transcription").length).toBeGreaterThan(0);
    });
    return ipc;
  }

  const extraText = () => screen.queryByText("An extra panel with 1 dictation profile(s)");

  test("is a fourth button at the end of the strip, shown while its tab is the active one", async () => {
    await renderWithExtra();
    expect(stripLabels()).toEqual(["Dictate", "Transcribe", "Settings", "Extra"]);
    expect(extraText()).toBeNull();

    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Extra" }));
    expect(await screen.findByText("An extra panel with 1 dictation profile(s)")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Transcribe" }));
    await waitFor(() => expect(extraText()).toBeNull());
  });

  test("opens on the tray's navigate_tab with its id", async () => {
    const ipc = await renderWithExtra();
    ipc.emit("navigate_tab", "extra");
    expect(await screen.findByText("An extra panel with 1 dictation profile(s)")).toBeInTheDocument();
  });
});
