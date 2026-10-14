// Settings.svelte: a change the backend saved versus a read-back that failed
// (#341). A saved change must never be reported as a failed save, an old copy
// must not be shown as confirmed, and an older answer must not replace a
// newer one. The window is rendered against an in-memory settings file;
// setters change it, get_settings reads it. Other Settings tests:
// Settings.test.ts, Settings.recovery.test.ts.
import { fireEvent, screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { describe, expect, test } from "vitest";
import type { Settings as SettingsValue } from "./api";
import { deferred, settle, type Deferred } from "../test/deferred";
import type { CommandHandlers } from "../test/ipc";
import { kbWhisperMedium, pianissimo, renderSettings, settingsBackend, settingsValue } from "../test/settings";

/**
 * The settings file in memory. `readFailure` makes get_settings reject and
 * `writeFailure` makes every setter reject, each with a Rust `Err(String)`.
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
    <A, R = undefined>(change: (args: A) => Partial<SettingsValue>, answer?: R) =>
    (args: A): R => {
      if (file.writeFailure !== null) throw file.writeFailure;
      file.current = { ...file.current, ...change(args) };
      return answer as R;
    };
  file.handlers = {
    get_settings: () => {
      if (file.readFailure !== null) throw file.readFailure;
      return file.current;
    },
    set_file_transcription_model: setter(({ modelId }) => ({ file_transcription_model: modelId })),
    set_language: setter(({ language }) => ({ language }), [] as string[]),
    set_beam_size: setter(({ beamSize }) => ({ beam_size: beamSize })),
    set_engine_prewarm: setter(({ mode }) => ({ engine_prewarm: mode })),
    set_engine_idle_unload_minutes: setter(({ minutes }) => ({ engine_idle_unload_minutes: minutes })),
    set_temperature_fallback: setter(({ enabled }) => ({ temperature_fallback: enabled })),
    set_profile_model: setter(({ profileId, modelId }) => ({
      profile_models: { ...file.current.profile_models, [profileId]: modelId },
    })),
    set_hotkey_profiles: setter(({ profiles }) => ({ hotkey_profiles: profiles }), [] as string[]),
  };
  return file;
}

const readbackFailure = "settings read timed out";

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
    const { ipc } = await renderSettings(settingsBackend(file.handlers));
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
    const { ipc } = await renderSettings(settingsBackend(file.handlers));
    const { user, picker } = await openTranscribeTab();
    file.readFailure = readbackFailure;
    await user.selectOptions(picker, "pianissimo-sv");
    await screen.findByRole("alert", { name: "Settings not confirmed" });
    expect(screen.getByText(/Effective model: Pianissimo/)).toBeInTheDocument();

    file.readFailure = null;
    await user.click(screen.getByRole("button", { name: "Reload settings" }));

    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
    expect(picker).toHaveValue("pianissimo-sv");
    expect(picker).toBeEnabled();
    expect(ipc.callsTo("set_file_transcription_model")).toHaveLength(1);
  });

  test("a saved Pianissimo choice still clears the options it does not support", async () => {
    const file = settingsFile();
    await renderSettings(settingsBackend(file.handlers));
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
    const { ipc } = await renderSettings(settingsBackend(file.handlers));
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
    const { ipc } = await renderSettings(settingsBackend(file.handlers));
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
    const { ipc } = await renderSettings(settingsBackend(file.handlers));
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
    const { ipc } = await renderSettings(settingsBackend(file.handlers));
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

  test("another change waits until the settings are confirmed, so a stale copy cannot overwrite the saved one", async () => {
    const file = settingsFile();
    const { ipc } = await renderSettings(settingsBackend(file.handlers));
    const user = await openAdvancedSettings();
    file.readFailure = readbackFailure;
    await user.click(screen.getByRole("switch", { name: "Temperature fallback" }));
    await screen.findByRole("alert", { name: "Settings not confirmed" });

    await user.selectOptions(screen.getByRole("combobox", { name: "Decoding mode" }), "Greedy (fast)");
    await settle();
    expect(ipc.callsTo("set_beam_size")).toEqual([]);
    expect(screen.getByRole("combobox", { name: "Decoding mode" })).toHaveDisplayValue("Beam search (accurate)");
    expect(screen.getByText(/This change was not sent: .* Reload settings first\./)).toBeInTheDocument();

    file.readFailure = null;
    await user.click(screen.getByRole("button", { name: "Reload settings" }));
    await waitFor(() => expect(screen.queryByRole("alert", { name: "Settings not confirmed" })).toBeNull());
    await user.selectOptions(screen.getByRole("combobox", { name: "Decoding mode" }), "Greedy (fast)");
    await waitFor(() => expect(ipc.callsTo("set_beam_size")).toEqual([{ beamSize: 0 }]));
  });

  test("a failed Reload settings keeps the notice and says why", async () => {
    const file = settingsFile();
    await renderSettings(settingsBackend(file.handlers));
    const { user, picker } = await openTranscribeTab();
    file.readFailure = readbackFailure;
    await user.selectOptions(picker, "pianissimo-sv");
    await screen.findByRole("alert", { name: "Settings not confirmed" });

    file.readFailure = "backend still unreachable";
    await user.click(screen.getByRole("button", { name: "Reload settings" }));

    await waitFor(() => expect(unconfirmedNotice()).toHaveTextContent("backend still unreachable"));
    expect(unconfirmedNotice()).toHaveTextContent(/saved/i);
    expect(screen.getByRole("button", { name: "Reload settings" })).toBeEnabled();
  });

  test("a later settings-changed event that reads back successfully confirms the saved state", async () => {
    const file = settingsFile();
    const { ipc } = await renderSettings(settingsBackend(file.handlers));
    const { user, picker } = await openTranscribeTab();
    file.readFailure = readbackFailure;
    await user.selectOptions(picker, "pianissimo-sv");
    await screen.findByRole("alert", { name: "Settings not confirmed" });

    file.readFailure = null;
    ipc.emit("state-changed", "settings_reloaded");

    await waitFor(() => expect(screen.queryByRole("alert")).toBeNull());
    expect(picker).toHaveValue("pianissimo-sv");
  });

  test("a settings-changed event that cannot be read says the window may be out of date", async () => {
    const file = settingsFile();
    const { ipc } = await renderSettings(settingsBackend(file.handlers));
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
    const { ipc } = await renderSettings(settingsBackend(file.handlers));
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
    await renderSettings(settingsBackend(file.handlers));
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
    await renderSettings(settingsBackend(file.handlers));
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
    const { ipc } = await renderSettings(settingsBackend(file.handlers));
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
    await renderSettings(settingsBackend(file.handlers));
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
    const { ipc } = await renderSettings(settingsBackend(file.handlers));
    const { user, picker } = await openTranscribeTab();
    const reads = holdReads(ipc);

    await user.selectOptions(picker, "pianissimo-sv");
    await waitFor(() => expect(reads).toHaveLength(1));
    ipc.emit("state-changed", "settings_reloaded");
    await waitFor(() => expect(reads).toHaveLength(2));

    // The second read answers first; the first one answers with the older state.
    reads[1].resolve(settingsValue({ file_transcription_model: "pianissimo-sv" }));
    await waitFor(() => expect(screen.getByText(/Effective model: Pianissimo/)).toBeInTheDocument());
    reads[0].resolve(settingsValue({ file_transcription_model: "auto" }));
    await settle();

    expect(picker).toHaveValue("pianissimo-sv");
    expect(picker).toBeEnabled();
    expect(screen.getByText(/Effective model: Pianissimo/)).toBeInTheDocument();
  });

  test("the older read of a setting cannot replace the read of the later change", async () => {
    const file = settingsFile();
    const { ipc } = await renderSettings(settingsBackend(file.handlers));
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
    const { ipc } = await renderSettings(settingsBackend(file.handlers));
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
    const { ipc } = await renderSettings(settingsBackend(file.handlers));
    const { user, picker } = await openTranscribeTab();
    const reads = holdReads(ipc);

    await user.selectOptions(picker, "pianissimo-sv");
    await waitFor(() => expect(reads).toHaveLength(1));
    ipc.emit("state-changed", "settings_reloaded");
    await waitFor(() => expect(reads).toHaveLength(2));

    reads[1].resolve(settingsValue({ file_transcription_model: "pianissimo-sv" }));
    await waitFor(() => expect(screen.getByText(/Effective model: Pianissimo/)).toBeInTheDocument());
    reads[0].reject(readbackFailure);
    await settle();

    expect(screen.queryByRole("alert")).toBeNull();
    expect(picker).toHaveValue("pianissimo-sv");
    expect(picker).toBeEnabled();
  });

  test("the latest read failing is reported even when an older one succeeds first", async () => {
    const file = settingsFile();
    const { ipc } = await renderSettings(settingsBackend(file.handlers));
    const { user, picker } = await openTranscribeTab();
    const reads = holdReads(ipc);

    await user.selectOptions(picker, "pianissimo-sv");
    await waitFor(() => expect(reads).toHaveLength(1));
    ipc.emit("state-changed", "settings_reloaded");
    await waitFor(() => expect(reads).toHaveLength(2));

    reads[0].resolve(settingsValue({ file_transcription_model: "pianissimo-sv" }));
    await settle();
    reads[1].reject(readbackFailure);

    expect(await screen.findByRole("alert", { name: "Settings not confirmed" })).toHaveTextContent(readbackFailure);
  });
});

describe("the models the window offers stay filled after a confirmed change", () => {
  test("a confirmed change refreshes the profile models once and shows no notice", async () => {
    const file = settingsFile();
    const { ipc } = await renderSettings(
      settingsBackend({ ...file.handlers, get_file_model_options: () => [pianissimo, kbWhisperMedium] }),
    );
    const user = await openAdvancedSettings();
    const before = ipc.callsTo("get_profile_model_info").length;

    await user.click(screen.getByRole("switch", { name: "Temperature fallback" }));

    await waitFor(() => expect(ipc.callsTo("get_profile_model_info").length).toBe(before + 1));
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.getByRole("switch", { name: "Temperature fallback" })).toHaveAttribute("aria-checked", "false");
  });
});
