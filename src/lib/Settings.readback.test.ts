// Settings.svelte: a change the backend saved versus a read-back that failed
// (#341). A saved change must never be reported as a failed save, an old copy
// must not be shown as confirmed, and an older answer must not replace a
// newer one. The window is rendered against an in-memory settings file;
// setters change it, get_settings reads it. Other Settings tests:
// Settings.test.ts, Settings.recovery.test.ts.
import { mockWindows } from "@tauri-apps/api/mocks";
import { fireEvent, render, screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { describe, expect, test } from "vitest";
import type { EnginePrewarm, HotkeyProfile, Settings as SettingsValue } from "./api";
import Settings from "./Settings.svelte";
import { deferred, settle, type Deferred } from "../test/deferred";
import { backendError, mockIpc, type CommandHandlers } from "../test/ipc";
import {
  kbWhisperMedium,
  pianissimo,
  renderSettings,
  settingsBackend,
  settingsValue,
  swedishProfile,
  whisperModel,
} from "../test/settings";

/**
 * The settings file in memory. `readFailure` makes get_settings reject and
 * `writeFailure` makes every setter reject, each with what its Rust command
 * rejects with: a backend error (every command returns `Result<_, CommandError>`)
 * with the given code, `settings_storage_failed` unless the setter names one.
 */
function settingsFile(initial: Partial<SettingsValue> = {}) {
  const file = {
    current: settingsValue(initial),
    readFailure: null as string | null,
    writeFailure: null as string | null,
    handlers: {} as CommandHandlers,
  };
  /** A setter: rejects while `writeFailure` is set, otherwise changes the file and answers `answer`. */
  const setter =
    <A, R = undefined>(change: (args: A) => Partial<SettingsValue>, answer?: R, errorCode?: string) =>
    (args: A): R => {
      if (file.writeFailure !== null) throw backendError(errorCode ?? "settings_storage_failed", file.writeFailure);
      file.current = { ...file.current, ...change(args) };
      return answer as R;
    };
  file.handlers = {
    get_settings: () => {
      if (file.readFailure !== null) throw backendError("unavailable", file.readFailure);
      return file.current;
    },
    set_file_transcription_model: setter(({ modelId }) => ({ file_transcription_model: modelId })),
    set_language: setter(({ language }) => ({ language }), [] as string[]),
    set_beam_size: setter(({ beamSize }) => ({ beam_size: beamSize })),
    set_profile_glossary: setter(
      ({ profileId, source }) => ({
        profile_glossaries: { ...file.current.profile_glossaries, [profileId]: source },
      }),
      undefined,
      "unclassified",
    ),
    // The command takes a plain string; the settings file stores the parsed mode.
    set_engine_prewarm: setter(({ mode }) => ({ engine_prewarm: mode as EnginePrewarm })),
    set_engine_idle_unload_minutes: setter(({ minutes }) => ({ engine_idle_unload_minutes: minutes })),
    set_temperature_fallback: setter(({ enabled }) => ({ temperature_fallback: enabled })),
    set_profile_model: setter(({ profileId, modelId }) => ({
      profile_models: { ...file.current.profile_models, [profileId]: modelId },
    })),
    set_hotkey_profiles: setter(({ profiles }) => ({ hotkey_profiles: profiles }), [] as string[], "hotkey_change_failed"),
  };
  return file;
}

const readbackFailure = "settings read timed out";

/** Auto resolves to KB-Whisper Medium here, so the saved Pianissimo choice reads differently from Auto. */
function backend(file: ReturnType<typeof settingsFile>) {
  return settingsBackend({ ...file.handlers, get_auto_file_model_info: () => kbWhisperMedium });
}

/** The notice the window shows while it cannot confirm what was saved. */
function unconfirmedNotice(): HTMLElement {
  return screen.getByRole("alert", { name: "Settings not confirmed" });
}

async function openTranscribeTab() {
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "Transcribe" }));
  return { user, picker: screen.getByRole("combobox", { name: "File transcription model" }) };
}

async function openSettingsTab() {
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "Settings" }));
  return user;
}

async function openAdvancedSettings() {
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "Settings" }));
  await user.click(await screen.findByText("Advanced"));
  return user;
}

/** Hold every later get_settings call: each one waits for the test to answer it. */
function holdReads(ipc: { handle: (command: "get_settings", handler: () => Promise<SettingsValue>) => void }) {
  const reads: Deferred<SettingsValue>[] = [];
  ipc.handle("get_settings", () => {
    const read = deferred<SettingsValue>();
    reads.push(read);
    return read.promise;
  });
  return reads;
}

describe("a saved change whose read-back fails", () => {
  test("the file model is reported as saved, the read-back as failed, and nothing is saved twice", async () => {
    const file = settingsFile();
    const { ipc } = await renderSettings(backend(file));
    const { user, picker } = await openTranscribeTab();

    file.readFailure = readbackFailure;
    await user.selectOptions(picker, "pianissimo-sv");

    const notice = await screen.findByRole("alert");
    expect(notice).toHaveTextContent(/saved/i);
    expect(notice).toHaveTextContent(readbackFailure);
    expect(notice).toHaveAccessibleName("Settings not confirmed");
    expect(screen.queryByText(/failed to save|could not save/i)).toBeNull();
    expect(screen.getAllByRole("alert")).toHaveLength(1);
    expect(file.current.file_transcription_model).toBe("pianissimo-sv");
    expect(ipc.callsTo("set_file_transcription_model")).toEqual([{ modelId: "pianissimo-sv" }]);
  });

  test("Reload settings reads again without saving again and shows what was saved", async () => {
    const file = settingsFile();
    const { ipc } = await renderSettings(backend(file));
    const { user, picker } = await openTranscribeTab();
    file.readFailure = readbackFailure;
    await user.selectOptions(picker, "pianissimo-sv");
    await screen.findByRole("alert", { name: "Settings not confirmed" });
    // What the window still holds is the old copy, which the notice says may be out of date.
    expect(screen.getByText(/Effective model: KB-Whisper Medium/)).toBeInTheDocument();

    file.readFailure = null;
    await user.click(screen.getByRole("button", { name: "Reload settings" }));

    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
    expect(picker).toHaveValue("pianissimo-sv");
    expect(picker).toBeEnabled();
    expect(screen.getByText(/Effective model: Pianissimo/)).toBeInTheDocument();
    expect(ipc.callsTo("set_file_transcription_model")).toHaveLength(1);
  });

  test("a saved Pianissimo choice still clears the options it does not support", async () => {
    const file = settingsFile();
    await renderSettings(backend(file));
    const { user, picker } = await openTranscribeTab();
    const diarize = screen.getByRole("checkbox", { name: /Speaker diarization/ });
    await user.click(diarize);
    expect(diarize).toBeChecked();

    file.readFailure = readbackFailure;
    await user.selectOptions(picker, "pianissimo-sv");
    await screen.findByRole("alert", { name: "Settings not confirmed" });

    expect(diarize).not.toBeChecked();
  });

  test("a changed setting is reported the same way, and the old value is not presented as confirmed", async () => {
    const file = settingsFile();
    const { ipc } = await renderSettings(backend(file));
    const user = await openAdvancedSettings();

    file.readFailure = readbackFailure;
    await user.click(screen.getByRole("switch", { name: "Temperature fallback" }));

    const notice = await screen.findByRole("alert");
    expect(notice).toHaveAccessibleName("Settings not confirmed");
    expect(notice).toHaveTextContent(/saved/i);
    expect(notice).toHaveTextContent(readbackFailure);
    expect(notice).toHaveTextContent(/may be out of date/i);
    expect(screen.queryByText(/failed to save/i)).toBeNull();
    expect(file.current.temperature_fallback).toBe(false);
    expect(ipc.callsTo("set_temperature_fallback")).toEqual([{ enabled: false }]);

    file.readFailure = null;
    await user.click(screen.getByRole("button", { name: "Reload settings" }));
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
    expect(screen.getByRole("switch", { name: "Temperature fallback" })).toHaveAttribute("aria-checked", "false");
  });

  test("a saved language stays selected and is reported as saved, not rolled back", async () => {
    const file = settingsFile();
    const { ipc } = await renderSettings(backend(file));
    const user = await openSettingsTab();
    const language = screen.getByRole("combobox", { name: "Default profile language" });
    expect(language).toHaveDisplayValue("Swedish");

    file.readFailure = readbackFailure;
    await user.selectOptions(language, "English");

    const notice = await screen.findByRole("alert", { name: "Settings not confirmed" });
    expect(notice).toHaveTextContent(/saved/i);
    expect(language).toHaveDisplayValue("English");
    expect(file.current.language).toBe("en");
    expect(ipc.callsTo("set_language")).toEqual([{ language: "en" }]);

    file.readFailure = null;
    await user.click(screen.getByRole("button", { name: "Reload settings" }));
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
    expect(language).toHaveDisplayValue("English");
  });

  test("a changed speech model for a profile is reported as saved, not as a failed change", async () => {
    const file = settingsFile();
    const { ipc } = await renderSettings(backend(file));
    const picker = await screen.findByRole("combobox", { name: "Model for Svenska" });
    await waitFor(() => expect(picker).toHaveDisplayValue(/Recommended for Swedish/));

    file.readFailure = readbackFailure;
    await userEvent.setup().selectOptions(picker, "kb-whisper-medium");

    const notice = await screen.findByRole("alert");
    expect(notice).toHaveAccessibleName("Settings not confirmed");
    expect(notice).toHaveTextContent(/saved/i);
    expect(screen.queryByText(/Could not change profile model/)).toBeNull();
    expect(ipc.callsTo("set_profile_model")).toEqual([{ profileId: "default", modelId: "kb-whisper-medium" }]);
    expect(file.current.profile_models).toEqual({ default: "kb-whisper-medium" });
  });

  test("a shortcut that was saved leaves capture mode and is reported as saved", async () => {
    const file = settingsFile();
    const { ipc } = await renderSettings(backend(file));
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Svenska Press to start/stop shortcut" }));

    file.readFailure = readbackFailure;
    fireEvent.keyDown(screen.getByRole("button", { name: "Svenska Press to start/stop shortcut" }), {
      key: "k",
      code: "KeyK",
      ctrlKey: true,
    });

    const notice = await screen.findByRole("alert");
    expect(notice).toHaveAccessibleName("Settings not confirmed");
    expect(notice).toHaveTextContent(/saved/i);
    expect(screen.queryByText(/Failed to set hotkey/)).toBeNull();
    expect(screen.queryByText("Press shortcut...")).toBeNull();
    expect(ipc.callsTo("set_hotkey_profiles")).toHaveLength(1);
    expect(file.current.hotkey_profiles[0].toggle_shortcut).toBe("Control+K");
  });

  test("a saved file model stays selected, and no other change can be chosen until the settings are confirmed", async () => {
    const file = settingsFile();
    const { ipc } = await renderSettings(backend(file));
    const { user, picker } = await openTranscribeTab();
    file.readFailure = readbackFailure;
    await user.selectOptions(picker, "pianissimo-sv");
    await screen.findByRole("alert", { name: "Settings not confirmed" });

    // The saved choice is still shown, not the old copy, and a second pick cannot be made.
    expect(picker).toHaveValue("pianissimo-sv");
    expect(picker).toBeDisabled();
    expect(ipc.callsTo("set_file_transcription_model")).toEqual([{ modelId: "pianissimo-sv" }]);

    file.readFailure = null;
    await user.click(screen.getByRole("button", { name: "Reload settings" }));
    await waitFor(() => expect(picker).toBeEnabled());
    expect(picker).toHaveValue("pianissimo-sv");
    await user.selectOptions(picker, "kb-whisper-medium");
    await waitFor(() => expect(ipc.callsTo("set_file_transcription_model")).toHaveLength(2));
    expect(picker).toHaveValue("kb-whisper-medium");
  });

  test("every control that saves a setting is disabled until the settings are confirmed", async () => {
    const file = settingsFile();
    const { ipc } = await renderSettings(backend(file));
    const user = userEvent.setup();
    const controls = async () => {
      const found: HTMLElement[] = [];
      await user.click(screen.getByRole("button", { name: "Dictate" }));
      found.push(
        screen.getByRole("combobox", { name: "Model for Svenska" }),
        screen.getByRole("combobox", { name: "Svenska language" }),
        screen.getByRole("textbox", { name: "Profile name" }),
        screen.getByRole("button", { name: "+ Add profile" }),
        screen.getByRole("button", { name: "Svenska Hold to record shortcut" }),
        screen.getByRole("switch", { name: "Auto-paste transcription" }),
      );
      await user.click(screen.getByRole("button", { name: "Transcribe" }));
      found.push(screen.getByRole("combobox", { name: "File transcription model" }));
      await user.click(screen.getByRole("button", { name: "Settings" }));
      if (!screen.queryByRole("combobox", { name: "Decoding mode" })) await user.click(await screen.findByText("Advanced"));
      found.push(
        screen.getByRole("combobox", { name: "Default profile language" }),
        screen.getByRole("switch", { name: "Show recording overlay" }),
        screen.getByRole("combobox", { name: "Decoding mode" }),
        screen.getByRole("switch", { name: "Temperature fallback" }),
        screen.getByRole("combobox", { name: "Prepare Pianissimo when" }),
        screen.getByRole("combobox", { name: "Unload after idle" }),
        screen.getByRole("switch", { name: "Voice activity detection" }),
      );
      return found;
    };
    expect((await controls()).filter((control) => control.hasAttribute("disabled"))).toEqual([]);

    file.readFailure = readbackFailure;
    ipc.emit("state-changed", "settings_reloaded");
    await screen.findByRole("alert", { name: "Settings not confirmed" });
    expect((await controls()).filter((control) => !control.hasAttribute("disabled"))).toEqual([]);

    file.readFailure = null;
    await user.click(screen.getByRole("button", { name: "Reload settings" }));
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
    expect((await controls()).filter((control) => control.hasAttribute("disabled"))).toEqual([]);
    expect(ipc.calls.filter((call) => call.command.startsWith("set_"))).toEqual([]);
  });

  test("the profile buttons and the Accessibility link are disabled while unconfirmed too", async () => {
    const bothShortcuts: HotkeyProfile = { ...swedishProfile, toggle_shortcut: "Ctrl+Alt+S" };
    const english: HotkeyProfile = {
      id: "english",
      name: "English",
      shortcut: "Ctrl+Alt+E",
      language: "en",
      push_to_talk_shortcut: null,
      toggle_shortcut: "Ctrl+Alt+E",
    };
    const file = settingsFile({
      hotkey_profiles: [bothShortcuts, english],
      profile_models: { default: "kb-whisper-small" },
      auto_paste: true,
    });
    const { ipc } = await renderSettings(
      settingsBackend({
        ...file.handlers,
        get_platform: () => "macos",
        check_accessibility_permission: () => false,
        get_profile_model_info: ({ profileId }) =>
          profileId === "default" ? whisperModel("kb-whisper-small", "KB-Whisper Small", { size_mb: 466 }) : pianissimo,
      }),
    );
    const buttons = async () => [
      ...(await screen.findAllByRole("button", { name: /^Clear .* shortcut for Svenska$/ })),
      screen.getByRole("button", { name: "Remove Svenska" }),
      screen.getByRole("button", { name: "Remove English" }),
      await screen.findByRole("button", { name: "Switch to Pianissimo" }),
      screen.getByRole("button", { name: "Open System Settings" }),
    ];
    expect((await buttons()).filter((button) => button.hasAttribute("disabled"))).toEqual([]);

    file.readFailure = readbackFailure;
    ipc.emit("state-changed", "settings_reloaded");
    await screen.findByRole("alert", { name: "Settings not confirmed" });
    expect((await buttons()).filter((button) => !button.hasAttribute("disabled"))).toEqual([]);

    file.readFailure = null;
    await userEvent.setup().click(screen.getByRole("button", { name: "Reload settings" }));
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
    expect((await buttons()).filter((button) => button.hasAttribute("disabled"))).toEqual([]);
  });

  test("the button that clears the blocking dictionary is disabled while unconfirmed", async () => {
    const file = settingsFile({ profile_glossaries: { default: "merge = merch" } });
    const { ipc } = await renderSettings(backend(file));
    const user = await openSettingsTab();
    ipc.handle("set_language", () => {
      throw backendError("dictionary_blocks_language_change", "Profile 'default' has a personal dictionary; clear it first");
    });
    await user.selectOptions(screen.getByRole("combobox", { name: "Default profile language" }), "English");
    const clear = await screen.findByRole("button", { name: "Clear default dictionary and switch to English" });
    expect(clear).toBeEnabled();

    file.readFailure = readbackFailure;
    ipc.emit("state-changed", "settings_reloaded");
    await screen.findByRole("alert", { name: "Settings not confirmed" });
    expect(clear).toBeDisabled();

    file.readFailure = null;
    await user.click(screen.getByRole("button", { name: "Reload settings" }));
    await waitFor(() => expect(clear).toBeEnabled());
    expect(ipc.callsTo("set_profile_glossary")).toEqual([]);
  });

  test("when the lock engages while a control has focus, the focus goes to Reload settings", async () => {
    const file = settingsFile();
    await renderSettings(settingsBackend(file.handlers));
    const user = await openAdvancedSettings();
    const toggle = screen.getByRole("switch", { name: "Temperature fallback" });

    file.readFailure = readbackFailure;
    await user.click(toggle);

    await waitFor(() => expect(toggle).toBeDisabled());
    await waitFor(() => expect(screen.getByRole("button", { name: "Reload settings" })).toHaveFocus());
  });

  test("an enabled control keeps the focus when the notice appears", async () => {
    const file = settingsFile();
    const { ipc } = await renderSettings(settingsBackend(file.handlers));
    const user = await openSettingsTab();
    const dictionary = screen.getByRole("textbox", { name: "Personal dictionary" });
    await user.type(dictionary, "merge = merch");

    file.readFailure = readbackFailure;
    ipc.emit("state-changed", "settings_reloaded");
    await screen.findByRole("alert", { name: "Settings not confirmed" });
    await settle();

    expect(dictionary).toHaveFocus();
  });

  test("a dictionary save is held while unconfirmed, says so, and works after a reload", async () => {
    const file = settingsFile();
    const { ipc } = await renderSettings(backend(file));
    const user = await openSettingsTab();
    await user.type(screen.getByRole("textbox", { name: "Personal dictionary" }), "merge = merch");
    file.readFailure = readbackFailure;
    ipc.emit("state-changed", "settings_reloaded");
    await screen.findByRole("alert", { name: "Settings not confirmed" });

    await user.click(screen.getByRole("button", { name: "Save changes" }));

    expect(await screen.findByText(/This change was not sent: .* Reload settings first\./)).toBeInTheDocument();
    expect(ipc.callsTo("set_profile_glossary")).toEqual([]);

    file.readFailure = null;
    await user.click(screen.getByRole("button", { name: "Reload settings" }));
    // The held-back message is gone with the notice, and the draft is still there to save.
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
    expect(screen.queryByText(/This change was not sent/)).toBeNull();
    expect(screen.getByRole("textbox", { name: "Personal dictionary" })).toHaveValue("merge = merch");
    await user.click(screen.getByRole("button", { name: "Save changes" }));
    await waitFor(() => expect(ipc.callsTo("set_profile_glossary")).toHaveLength(1));
    expect(file.current.profile_glossaries.default).toBe("merge = merch");
  });

  test("Reload settings keeps the keyboard focus in the window, before and after it succeeds", async () => {
    const file = settingsFile();
    const { ipc } = await renderSettings(backend(file));
    const { user, picker } = await openTranscribeTab();
    file.readFailure = readbackFailure;
    await user.selectOptions(picker, "pianissimo-sv");
    const reload = await screen.findByRole("button", { name: "Reload settings" });
    reload.focus();
    const reads = holdReads(ipc);

    await user.click(reload);
    await waitFor(() => expect(reads).toHaveLength(1));
    // Busy but still focusable, and a second press does not read twice.
    expect(reload).toHaveAttribute("aria-disabled", "true");
    expect(reload).toHaveFocus();
    await user.click(reload);
    expect(reads).toHaveLength(1);

    reads[0].reject(backendError("unavailable", "backend still unreachable"));
    await waitFor(() => expect(unconfirmedNotice()).toHaveTextContent("backend still unreachable"));
    await waitFor(() => expect(reload).toHaveAttribute("aria-disabled", "false"));
    expect(reload).toHaveFocus();

    await user.click(reload);
    await waitFor(() => expect(reads).toHaveLength(2));
    reads[1].resolve(settingsValue({ file_transcription_model: "pianissimo-sv" }));
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
    await waitFor(() => expect(screen.getByRole("heading", { name: "Sagascript" })).toHaveFocus());
    expect(document.body).not.toHaveFocus();
  });

  test("while the notice is shown, focus scrolling leaves room for it", async () => {
    const file = settingsFile();
    const { ipc } = await renderSettings(backend(file));
    file.readFailure = readbackFailure;

    ipc.emit("state-changed", "settings_reloaded");
    const scroller = (await screen.findByRole("alert", { name: "Settings not confirmed" })).parentElement!;
    expect(scroller.style.getPropertyValue("--notice-clearance")).toMatch(/^\d+(\.\d+)?px$/);

    file.readFailure = null;
    await userEvent.setup().click(screen.getByRole("button", { name: "Reload settings" }));
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
    expect(scroller.style.getPropertyValue("--notice-clearance")).toBe("");
  });

  test("a notice that clears while the user is elsewhere does not take the focus", async () => {
    const file = settingsFile();
    const { ipc } = await renderSettings(backend(file));
    const { user, picker } = await openTranscribeTab();
    file.readFailure = readbackFailure;
    await user.selectOptions(picker, "pianissimo-sv");
    await screen.findByRole("alert", { name: "Settings not confirmed" });
    screen.getByRole("button", { name: "Transcribe" }).focus();

    file.readFailure = null;
    ipc.emit("state-changed", "settings_reloaded");

    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
    expect(screen.getByRole("button", { name: "Transcribe" })).toHaveFocus();
  });

  test("a failed Reload settings keeps the notice and says why", async () => {
    const file = settingsFile();
    await renderSettings(backend(file));
    const { user, picker } = await openTranscribeTab();
    file.readFailure = readbackFailure;
    await user.selectOptions(picker, "pianissimo-sv");
    await screen.findByRole("alert", { name: "Settings not confirmed" });

    file.readFailure = "backend still unreachable";
    await user.click(screen.getByRole("button", { name: "Reload settings" }));

    await waitFor(() => expect(unconfirmedNotice()).toHaveTextContent("backend still unreachable"));
    expect(unconfirmedNotice()).toHaveTextContent(/saved/i);
    expect(screen.getByRole("button", { name: "Reload settings" })).toHaveAttribute("aria-disabled", "false");
  });

  test("a later settings-changed event that reads back successfully confirms the saved state", async () => {
    const file = settingsFile();
    const { ipc } = await renderSettings(backend(file));
    const { user, picker } = await openTranscribeTab();
    file.readFailure = readbackFailure;
    await user.selectOptions(picker, "pianissimo-sv");
    await screen.findByRole("alert", { name: "Settings not confirmed" });
    // The failed read-back did not touch the models.
    const before = ipc.callsTo("get_profile_model_info").length;

    file.readFailure = null;
    ipc.emit("state-changed", "settings_reloaded");

    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
    expect(picker).toHaveValue("pianissimo-sv");
    // The settings the window shows changed, so the models it offers are queried again, once.
    await waitFor(() => expect(ipc.callsTo("get_profile_model_info").length).toBe(before + 1));
    await settle();
    expect(ipc.callsTo("get_profile_model_info").length).toBe(before + 1);
  });

  test("a settings-changed event that cannot be read says the window may be out of date", async () => {
    const file = settingsFile();
    const { ipc } = await renderSettings(backend(file));
    file.readFailure = readbackFailure;

    ipc.emit("state-changed", "settings_reloaded");

    const notice = await screen.findByRole("alert", { name: "Settings not confirmed" });
    expect(notice).toHaveTextContent(readbackFailure);
    expect(notice).toHaveTextContent(/may be out of date/i);
    expect(notice).not.toHaveTextContent(/your change was saved/i);
  });
});

describe("a rejected change", () => {
  test("the file model control goes back to the saved choice and the error says so", async () => {
    const file = settingsFile();
    const { ipc } = await renderSettings(backend(file));
    const { user, picker } = await openTranscribeTab();

    file.writeFailure = "disk full";
    await user.selectOptions(picker, "pianissimo-sv");

    expect(await screen.findByRole("alert")).toHaveTextContent("disk full");
    expect(screen.queryByRole("alert", { name: "Settings not confirmed" })).toBeNull();
    await waitFor(() => expect(picker).toHaveValue("auto"));
    expect(file.current.file_transcription_model).toBe("auto");
    expect(ipc.callsTo("get_settings")).toHaveLength(1);
  });

  test("a setting's control goes back to the saved value", async () => {
    const file = settingsFile();
    await renderSettings(backend(file));
    const user = await openAdvancedSettings();
    const decoding = screen.getByRole("combobox", { name: "Decoding mode" });
    expect(decoding).toHaveDisplayValue("Beam search (accurate)");

    file.writeFailure = "disk full";
    await user.selectOptions(decoding, "Greedy (fast)");

    expect(await screen.findByRole("alert")).toHaveTextContent("disk full");
    await waitFor(() => expect(decoding).toHaveDisplayValue("Beam search (accurate)"));
    expect(screen.queryByRole("alert", { name: "Settings not confirmed" })).toBeNull();
  });

  test.each([
    { control: "Prepare Pianissimo when", choice: "Only when needed", saved: "When I press the dictation key" },
    { control: "Unload after idle", choice: "Never", saved: "10 minutes" },
  ])("the $control control goes back to the saved choice", async ({ control, choice, saved }) => {
    const file = settingsFile();
    await renderSettings(backend(file));
    const user = await openAdvancedSettings();
    const select = screen.getByRole("combobox", { name: control });
    expect(select).toHaveDisplayValue(saved);

    file.writeFailure = "disk full";
    await user.selectOptions(select, choice);

    expect(await screen.findByRole("alert")).toHaveTextContent("disk full");
    await waitFor(() => expect(select).toHaveDisplayValue(saved));
  });

  test("a profile's language and name go back to the saved ones", async () => {
    const file = settingsFile();
    const { ipc } = await renderSettings(backend(file));
    const user = userEvent.setup();
    const language = screen.getByRole("combobox", { name: "Svenska language" });
    const name = screen.getByRole("textbox", { name: "Profile name" });
    expect(language).toHaveDisplayValue("Swedish");

    file.writeFailure = "disk full";
    await user.selectOptions(language, "English");
    expect(await screen.findByRole("alert")).toHaveTextContent("disk full");
    await waitFor(() => expect(language).toHaveDisplayValue("Swedish"));

    await user.clear(name);
    await user.type(name, "Renamed");
    await user.tab();
    await waitFor(() => expect(ipc.callsTo("set_hotkey_profiles")).toHaveLength(2));
    await waitFor(() => expect(name).toHaveValue("Svenska"));
    expect(file.current.hotkey_profiles[0].name).toBe("Svenska");
  });

  test("a profile's model control goes back to the saved model", async () => {
    const file = settingsFile();
    await renderSettings(backend(file));
    const picker = await screen.findByRole("combobox", { name: "Model for Svenska" });
    await waitFor(() => expect(picker).toHaveDisplayValue(/Recommended for Swedish/));

    file.writeFailure = "disk full";
    await userEvent.setup().selectOptions(picker, "kb-whisper-medium");

    expect(await screen.findByText("disk full")).toBeInTheDocument();
    expect(picker).toHaveDisplayValue(/Recommended for Swedish/);
  });
});

describe("two answers that arrive out of order", () => {
  test("the older read of a file model change cannot replace a newer settings-changed read", async () => {
    const file = settingsFile();
    const { ipc } = await renderSettings(backend(file));
    const { user, picker } = await openTranscribeTab();
    const reads = holdReads(ipc);

    await user.selectOptions(picker, "pianissimo-sv");
    await waitFor(() => expect(reads).toHaveLength(1));
    ipc.emit("state-changed", "settings_reloaded");
    await waitFor(() => expect(reads).toHaveLength(2));

    // The second read answers first; the first one answers with the older state.
    expect(screen.getByText(/Effective model: KB-Whisper Medium/)).toBeInTheDocument();
    reads[1].resolve(settingsValue({ file_transcription_model: "pianissimo-sv" }));
    await waitFor(() => expect(screen.getByText(/Effective model: Pianissimo/)).toBeInTheDocument());
    reads[0].resolve(settingsValue({ file_transcription_model: "auto" }));
    await settle();

    expect(picker).toHaveValue("pianissimo-sv");
    expect(picker).toBeEnabled();
    expect(screen.getByText(/Effective model: Pianissimo/)).toBeInTheDocument();
    expect(screen.queryByText(/Effective model: KB-Whisper Medium/)).toBeNull();
  });

  test("the older read of a setting cannot replace the read of the later change", async () => {
    const file = settingsFile();
    const { ipc } = await renderSettings(backend(file));
    const user = await openAdvancedSettings();
    const decoding = screen.getByRole("combobox", { name: "Decoding mode" });
    const reads = holdReads(ipc);

    await user.selectOptions(decoding, "Greedy (fast)");
    await waitFor(() => expect(reads).toHaveLength(1));
    await user.selectOptions(decoding, "Beam search (accurate)");
    await waitFor(() => expect(reads).toHaveLength(2));
    expect(ipc.callsTo("set_beam_size")).toEqual([{ beamSize: 0 }, { beamSize: 5 }]);

    reads[1].resolve(settingsValue({ beam_size: 5 }));
    await settle();
    reads[0].resolve(settingsValue({ beam_size: 0 }));
    await settle();

    expect(decoding).toHaveDisplayValue("Beam search (accurate)");
  });

  test("answers in order leave the newest state", async () => {
    const file = settingsFile();
    const { ipc } = await renderSettings(backend(file));
    const user = await openAdvancedSettings();
    const decoding = screen.getByRole("combobox", { name: "Decoding mode" });
    const reads = holdReads(ipc);

    await user.selectOptions(decoding, "Greedy (fast)");
    await waitFor(() => expect(reads).toHaveLength(1));
    await user.selectOptions(decoding, "Beam search (accurate)");
    await waitFor(() => expect(reads).toHaveLength(2));

    reads[0].resolve(settingsValue({ beam_size: 0 }));
    await settle();
    reads[1].resolve(settingsValue({ beam_size: 5 }));
    await settle();

    expect(decoding).toHaveDisplayValue("Beam search (accurate)");
  });

  test("an older read that fails after a newer one was shown reports nothing", async () => {
    const file = settingsFile();
    const { ipc } = await renderSettings(backend(file));
    const { user, picker } = await openTranscribeTab();
    const reads = holdReads(ipc);

    await user.selectOptions(picker, "pianissimo-sv");
    await waitFor(() => expect(reads).toHaveLength(1));
    ipc.emit("state-changed", "settings_reloaded");
    await waitFor(() => expect(reads).toHaveLength(2));

    reads[1].resolve(settingsValue({ file_transcription_model: "pianissimo-sv" }));
    await waitFor(() => expect(screen.getByText(/Effective model: Pianissimo/)).toBeInTheDocument());
    reads[0].reject(backendError("unavailable", readbackFailure));
    await settle();

    expect(screen.queryByRole("alert")).toBeNull();
    expect(picker).toHaveValue("pianissimo-sv");
    expect(picker).toBeEnabled();
  });

  test("the latest read failing is reported even when an older one succeeds first", async () => {
    const file = settingsFile();
    const { ipc } = await renderSettings(backend(file));
    const { user, picker } = await openTranscribeTab();
    const reads = holdReads(ipc);

    await user.selectOptions(picker, "pianissimo-sv");
    await waitFor(() => expect(reads).toHaveLength(1));
    ipc.emit("state-changed", "settings_reloaded");
    await waitFor(() => expect(reads).toHaveLength(2));

    reads[0].resolve(settingsValue({ file_transcription_model: "pianissimo-sv" }));
    await settle();
    reads[1].reject(backendError("unavailable", readbackFailure));

    expect(await screen.findByRole("alert", { name: "Settings not confirmed" })).toHaveTextContent(readbackFailure);
  });

  test("the latest read failing is reported even when an older one succeeds later", async () => {
    const file = settingsFile();
    const { ipc } = await renderSettings(backend(file));
    const user = await openAdvancedSettings();
    const decoding = screen.getByRole("combobox", { name: "Decoding mode" });
    const reads = holdReads(ipc);

    await user.selectOptions(decoding, "Greedy (fast)");
    await waitFor(() => expect(reads).toHaveLength(1));
    await user.selectOptions(decoding, "Beam search (accurate)");
    await waitFor(() => expect(reads).toHaveLength(2));
    expect(ipc.callsTo("set_beam_size")).toEqual([{ beamSize: 0 }, { beamSize: 5 }]);

    reads[1].reject(backendError("unavailable", readbackFailure));
    await screen.findByRole("alert", { name: "Settings not confirmed" });
    // The older read answers now with the state before the second change.
    reads[0].resolve(settingsValue({ beam_size: 0 }));
    await settle();

    expect(unconfirmedNotice()).toHaveTextContent(readbackFailure);
    expect(decoding).toHaveDisplayValue("Beam search (accurate)");
    expect(decoding).toBeDisabled();
  });
});

const dictionaryBlock = "Profile 'default' has a personal dictionary; clear it first";

describe("a failed read that is not part of a save", () => {
  /** Temperature fallback saved (its read-back is reads[0]), then a language the dictionary blocks (its preview read is reads[1]). */
  async function savedChangeThenBlockedLanguage() {
    const file = settingsFile({ profile_glossaries: { default: "merge = merch" } });
    const { ipc } = await renderSettings(backend(file));
    const user = await openAdvancedSettings();
    ipc.handle("set_language", () => {
      throw backendError("dictionary_blocks_language_change", dictionaryBlock);
    });
    const reads = holdReads(ipc);

    await user.click(screen.getByRole("switch", { name: "Temperature fallback" }));
    await waitFor(() => expect(reads).toHaveLength(1));
    await user.selectOptions(screen.getByRole("combobox", { name: "Default profile language" }), "English");
    await waitFor(() => expect(reads).toHaveLength(2));
    return { ipc, reads };
  }

  test("a failed preview read is reported even when the older read-back of a saved change then succeeds", async () => {
    const { reads } = await savedChangeThenBlockedLanguage();

    reads[1].reject(backendError("unavailable", readbackFailure));
    await screen.findByRole("alert", { name: "Settings not confirmed" });
    // The older read answers with the saved toggle, but a newer read has failed since.
    reads[0].resolve(settingsValue({ temperature_fallback: false }));
    await settle();

    const notice = unconfirmedNotice();
    expect(notice).toHaveTextContent(readbackFailure);
    expect(notice).toHaveTextContent(/your change was saved/i);
    expect(screen.getByRole("switch", { name: "Temperature fallback" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Reload settings" })).toBeInTheDocument();
  });

  test("a failed preview read and a failed read-back are both reported, and the save is not called confirmed", async () => {
    const { ipc, reads } = await savedChangeThenBlockedLanguage();

    reads[1].reject(backendError("unavailable", readbackFailure));
    await screen.findByRole("alert", { name: "Settings not confirmed" });
    reads[0].reject(backendError("unavailable", "the older read failed as well"));
    await settle();

    const notice = unconfirmedNotice();
    expect(notice).toHaveTextContent(readbackFailure);
    expect(notice).toHaveTextContent(/your change was saved/i);
    expect(screen.getByRole("switch", { name: "Temperature fallback" })).toBeDisabled();
    expect(ipc.callsTo("set_temperature_fallback")).toEqual([{ enabled: false }]);
  });

  test("a failed preview read after a rejected language change locks the window with one message", async () => {
    const file = settingsFile({ profile_glossaries: { default: "merge = merch" } });
    const { ipc } = await renderSettings(backend(file));
    const user = await openSettingsTab();
    ipc.handle("set_language", () => {
      throw backendError("dictionary_blocks_language_change", dictionaryBlock);
    });
    const language = screen.getByRole("combobox", { name: "Default profile language" });

    file.readFailure = readbackFailure;
    await user.selectOptions(language, "English");

    const notice = await screen.findByRole("alert", { name: "Settings not confirmed" });
    // Nothing was saved and nothing changed on disk: neutral wording, naming the failed read.
    expect(notice).toHaveTextContent("The settings could not be re-read");
    expect(notice).toHaveTextContent(readbackFailure);
    expect(notice).not.toHaveTextContent(/settings changed|your change was saved/i);
    // The rejection stays as the reason, without a second instruction to select the language again.
    expect(screen.getByText(/has a personal dictionary/)).toBeInTheDocument();
    expect(screen.queryByText(/Select the language again to retry/)).toBeNull();
    expect(language).toBeDisabled();

    // Reload settings lifts the lock and the rejection message, which no preview follows.
    file.readFailure = null;
    await user.click(screen.getByRole("button", { name: "Reload settings" }));
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
    expect(screen.queryByText(/has a personal dictionary/)).toBeNull();
    expect(language).toBeEnabled();
    expect(language).toHaveDisplayValue("Swedish");
  });

  test("a cleared dictionary whose read-back was dropped for a newer failed read is reported as saved", async () => {
    const file = settingsFile({ profile_glossaries: { default: "merge = merch" } });
    const { ipc } = await renderSettings(backend(file));
    const user = await openSettingsTab();
    ipc.handle("set_language", () => {
      throw backendError("dictionary_blocks_language_change", dictionaryBlock);
    });
    await user.selectOptions(screen.getByRole("combobox", { name: "Default profile language" }), "English");
    const clear = await screen.findByRole("button", { name: "Clear default dictionary and switch to English" });
    ipc.handle("set_language", () => []);
    const reads = holdReads(ipc);

    await user.click(clear);
    await waitFor(() => expect(reads).toHaveLength(1));
    ipc.emit("state-changed", "settings_reloaded");
    await waitFor(() => expect(reads).toHaveLength(2));
    reads[1].reject(backendError("unavailable", readbackFailure));
    await screen.findByRole("alert", { name: "Settings not confirmed" });
    reads[0].resolve(settingsValue({ language: "en" }));
    await settle();

    const notice = unconfirmedNotice();
    expect(notice).toHaveTextContent(readbackFailure);
    expect(notice).toHaveTextContent(/your change was saved/i);
    expect(ipc.callsTo("set_profile_glossary")).toHaveLength(1);
  });

  test("the first read answering after a newer read failed still shows the window, with the notice", async () => {
    const reads: Deferred<SettingsValue>[] = [];
    const ipc = mockIpc(
      settingsBackend({
        get_settings: () => {
          const read = deferred<SettingsValue>();
          reads.push(read);
          return read.promise;
        },
      }),
    );
    mockWindows("main");
    render(Settings);
    await waitFor(() => {
      expect(reads).toHaveLength(1);
      expect(ipc.listenerCount("state-changed")).toBeGreaterThan(0);
    });
    expect(screen.getByText("Loading settings...")).toBeInTheDocument();

    ipc.emit("state-changed", "settings_reloaded");
    await waitFor(() => expect(reads).toHaveLength(2));
    reads[1].reject(backendError("unavailable", readbackFailure));
    await settle();
    reads[0].resolve(settingsValue());

    const notice = await screen.findByRole("alert", { name: "Settings not confirmed" });
    expect(notice).toHaveTextContent(readbackFailure);
    expect(notice).not.toHaveTextContent(/your change was saved/i);
    expect(screen.queryByText("Loading settings...")).toBeNull();

    // Reload settings works from here.
    await userEvent.setup().click(screen.getByRole("button", { name: "Reload settings" }));
    await waitFor(() => expect(reads).toHaveLength(3));
    reads[2].resolve(settingsValue());
    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
    expect(screen.getByRole("button", { name: "Dictate" })).toBeInTheDocument();
  });
});

describe("the models the window offers stay filled after a confirmed change", () => {
  test("a confirmed change refreshes the profile models once and shows no notice", async () => {
    const file = settingsFile();
    const { ipc } = await renderSettings(
      settingsBackend({ ...file.handlers, get_file_model_options: () => [pianissimo, kbWhisperMedium] }),
    );
    const user = await openSettingsTab();
    const before = ipc.callsTo("get_profile_model_info").length;

    // A language change is one that models depend on. A setting that none does,
    // such as Temperature fallback, no longer queries them (#362, Settings.saving.test.ts).
    await user.selectOptions(screen.getByRole("combobox", { name: "Default profile language" }), "English");

    await waitFor(() => expect(ipc.callsTo("get_profile_model_info").length).toBe(before + 1));
    // Leave time for a second query to follow: it must not.
    await settle();
    expect(ipc.callsTo("get_profile_model_info").length).toBe(before + 1);
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.getByRole("combobox", { name: "Default profile language" })).toHaveValue("en");
  });
});
