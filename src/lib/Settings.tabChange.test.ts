// What a tab change must not lose (SAGA-379). The Dictate tab and the Advanced
// section are destroyed when their tab is left and created again when it is
// shown; their state and whatever they started live in the window's
// controllers. These tests leave the tab in the middle of an operation,
// through the whole window, and come back. The panels' own tests
// (src/lib/settings/*.test.ts) render a panel that is never destroyed.
import { screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { describe, expect, test } from "vitest";
import type { Settings as SettingsValue } from "./api";
import { deferred } from "../test/deferred";
import { backendError } from "../test/ipc";
import { renderSettings, settingsBackend, settingsValue } from "../test/settings";

const tab = (name: "Dictate" | "Transcribe" | "Settings") => screen.getByRole("button", { name });
const resultField = () => screen.getByPlaceholderText(/use your hotkey/);
const transcribeTab = () => screen.findByRole("button", { name: "Open Files..." });

describe("leaving the Dictate tab", () => {
  test("a dictation that arrives while another tab shows is in the result field on return", async () => {
    const { ipc } = await renderSettings();
    const user = userEvent.setup();
    await user.click(tab("Transcribe"));
    await transcribeTab();
    expect(screen.queryByPlaceholderText(/use your hotkey/)).toBeNull();

    ipc.emit("transcription-result", "Dikterat via kortkommandot");
    await user.click(tab("Dictate"));

    expect(resultField()).toHaveValue("Dikterat via kortkommandot");
    expect(screen.getByRole("button", { name: "Copy result" })).toBeInTheDocument();
  });

  test("a recording stopped in the window finishes while another tab shows, and its text is there on return", async () => {
    const transcription = deferred<string>();
    const { ipc } = await renderSettings(
      settingsBackend({ start_recording: () => undefined, stop_and_transcribe: () => transcription.promise }),
    );
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Start recording" }));
    await user.click(await screen.findByRole("button", { name: "Stop recording" }));
    await waitFor(() => expect(ipc.callsTo("stop_and_transcribe")).toHaveLength(1));
    expect(screen.getByRole("button", { name: "Transcribing..." })).toBeDisabled();

    await user.click(tab("Transcribe"));
    await transcribeTab();
    transcription.resolve("Inspelat i fönstret");
    await user.click(tab("Dictate"));

    await waitFor(() => expect(resultField()).toHaveValue("Inspelat i fönstret"));
    expect(screen.getByRole("button", { name: "Start recording" })).toBeEnabled();
    expect(ipc.callsTo("stop_and_transcribe")).toHaveLength(1);
  });

  test("a recording that is still being transcribed shows as such on return", async () => {
    const transcription = deferred<string>();
    await renderSettings(settingsBackend({ start_recording: () => undefined, stop_and_transcribe: () => transcription.promise }));
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Start recording" }));
    await user.click(await screen.findByRole("button", { name: "Stop recording" }));

    await user.click(tab("Transcribe"));
    await transcribeTab();
    await user.click(tab("Dictate"));

    expect(screen.getByRole("button", { name: "Transcribing..." })).toBeDisabled();
    transcription.resolve("Klart");
    await waitFor(() => expect(resultField()).toHaveValue("Klart"));
  });

  test("a profile's new name that is still being saved when the tab is left is shown on return", async () => {
    let current: SettingsValue = settingsValue();
    const saving = deferred<void>();
    const { ipc } = await renderSettings(
      settingsBackend({
        get_settings: () => current,
        set_hotkey_profiles: async ({ profiles }) => {
          await saving.promise;
          current = { ...current, hotkey_profiles: profiles };
          return [];
        },
      }),
    );
    const user = userEvent.setup();
    const name = screen.getByRole("textbox", { name: "Profile name" });
    await user.clear(name);
    await user.type(name, "Norsk");

    // Pressing the tab takes the focus from the field, which saves the name, and leaves the tab.
    await user.click(tab("Transcribe"));
    await transcribeTab();
    await waitFor(() => expect(ipc.callsTo("set_hotkey_profiles")).toHaveLength(1));
    saving.resolve();
    await user.click(tab("Dictate"));

    await waitFor(() => expect(screen.getByRole("textbox", { name: "Profile name" })).toHaveValue("Norsk"));
    expect(ipc.callsTo("set_hotkey_profiles")).toHaveLength(1);
  });
});

describe("leaving the Settings tab", () => {
  test("an Advanced change the backend rejects after the tab was left marks the Settings tab, and the error and the saved value show on return", async () => {
    const saving = deferred<void>();
    const { ipc } = await renderSettings(
      settingsBackend({
        set_beam_size: async () => {
          await saving.promise;
          throw backendError("io", "Could not write the settings file.");
        },
      }),
    );
    const user = userEvent.setup();
    await user.click(tab("Settings"));
    await user.click(await screen.findByText("Advanced"));
    const mode = await screen.findByRole("combobox", { name: "Decoding mode" });
    expect(mode).toHaveValue("5");
    await user.selectOptions(mode, "0");
    await waitFor(() => expect(ipc.callsTo("set_beam_size")).toEqual([{ beamSize: 0 }]));

    await user.click(tab("Dictate"));
    await screen.findByRole("button", { name: "Start recording" });
    saving.resolve();

    // The change was made on Settings: Dictate does not show its error, the Settings button is marked.
    const marked = await screen.findByRole("button", { name: "Settings, has an error" });
    expect(screen.queryByText("Could not write the settings file.")).toBeNull();
    await user.click(marked);
    expect(await screen.findByRole("alert")).toHaveTextContent("Could not write the settings file.");
    expect(tab("Settings")).toBeInTheDocument();
    await user.click(await screen.findByText("Advanced"));
    expect(await screen.findByRole("combobox", { name: "Decoding mode" })).toHaveValue("5");
  });
});
