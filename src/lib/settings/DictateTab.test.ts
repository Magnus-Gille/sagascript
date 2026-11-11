// DictateTab on its own (SAGA-379): the dictation profile cards and their
// shortcut recorder, the per-profile speech model and its download, auto-paste,
// the hotkey registration health and the "Try it out" recorder, rendered over
// the window's real controllers and the typed IPC mock, without the shell.
// Settings.saving.test.ts, Settings.errorRecovery.test.ts and
// Settings.downloads.test.ts cover the same area through the whole window.
import { screen, waitFor, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { describe, expect, test, vi } from "vitest";
import { backendError, type CommandHandlers } from "../../test/ipc";
import { deferred, settle } from "../../test/deferred";
import { renderPanel } from "../../test/settingsPanel";
import { kbWhisperMedium, pianissimo, settingsBackend, settingsValue, whisperModel } from "../../test/settings";
import DictateTab from "./DictateTab.svelte";

// The real wait for the Accessibility grant polls for a minute; here it gives up after two checks.
vi.mock("../permissions", async (importOriginal) => {
  const real = await importOriginal<typeof import("../permissions")>();
  return {
    ...real,
    requestAccessibility: (options = {}) => real.requestAccessibility({ intervalMs: 5, attempts: 2, ...options }),
  };
});

const backend = (overrides: CommandHandlers = {}) =>
  settingsBackend({
    set_hotkey_profiles: () => [],
    set_profile_model: () => undefined,
    set_auto_paste: () => undefined,
    ...overrides,
  });

const render = (overrides: CommandHandlers = {}) => renderPanel(DictateTab, backend(overrides));

/** Profile Svenska, whose speech model (KB-Whisper Medium) is not on disk. */
const missingModel = whisperModel(kbWhisperMedium.id, kbWhisperMedium.display_name, { size_mb: 1530, downloaded: false });

describe("the profile cards", () => {
  test("a profile shows its name, language, model choices and both shortcuts", async () => {
    await render();

    expect(screen.getByRole("textbox", { name: "Profile name" })).toHaveValue("Svenska");
    expect(screen.getByRole("combobox", { name: "Svenska language" })).toHaveValue("sv");
    const model = screen.getByRole("combobox", { name: "Model for Svenska" });
    expect(
      within(model)
        .getAllByRole("option")
        .map((option) => option.textContent),
    ).toEqual(["Recommended for Swedish", "Pianissimo", "KB-Whisper Medium"]);
    expect(model).toHaveValue("auto");
    expect(screen.getByRole("button", { name: "Svenska Hold to record shortcut" })).toHaveTextContent("Ctrl + Shift + Space");
    expect(screen.getByRole("button", { name: "Svenska Press to start/stop shortcut" })).toHaveTextContent("Not set");
  });

  test("+ Add profile adds a draft card and captures its Hold to record shortcut until Escape", async () => {
    const { ipc } = await render();
    const user = userEvent.setup();

    await user.click(screen.getByRole("button", { name: "+ Add profile" }));

    const recorder = await screen.findByRole("button", { name: "Profile 2 Hold to record shortcut" });
    expect(recorder).toHaveTextContent("Press shortcut...");
    await waitFor(() => expect(recorder).toHaveFocus());
    expect(screen.getAllByRole("textbox", { name: "Profile name" })).toHaveLength(2);

    await user.keyboard("{Escape}");

    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Profile 2 Hold to record shortcut" })).not.toHaveTextContent("Press shortcut..."),
    );
    expect(ipc.callsTo("set_hotkey_profiles")).toEqual([]);
  });

  test("a shortcut with a modifier is saved for the profile in the slot that was captured", async () => {
    const { ipc } = await render();
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Svenska Press to start/stop shortcut" }));
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Svenska Press to start/stop shortcut" })).toHaveTextContent("Press shortcut..."),
    );

    await user.keyboard("{Control>}{Shift>}K{/Shift}{/Control}");

    await waitFor(() => expect(ipc.callsTo("set_hotkey_profiles")).toHaveLength(1));
    expect(ipc.callsTo("set_hotkey_profiles")[0].profiles).toEqual([
      expect.objectContaining({
        id: "default",
        push_to_talk_shortcut: "Ctrl+Shift+Space",
        toggle_shortcut: "Control+Shift+K",
      }),
    ]);
  });

  test("a plain letter is refused for lack of a modifier and nothing is sent", async () => {
    const { ipc } = await render();
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Svenska Hold to record shortcut" }));
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Svenska Hold to record shortcut" })).toHaveTextContent("Press shortcut..."),
    );

    await user.keyboard("k");

    expect(await screen.findByText(/Shortcut must include a modifier/)).toBeInTheDocument();
    await settle();
    expect(ipc.callsTo("set_hotkey_profiles")).toEqual([]);
  });
});

describe("renaming a profile", () => {
  test("leaving the name field sends the new name", async () => {
    const { ipc } = await render();
    const user = userEvent.setup();
    const name = screen.getByRole("textbox", { name: "Profile name" });

    await user.clear(name);
    await user.type(name, "Norsk");
    await user.tab();

    await waitFor(() => expect(ipc.callsTo("set_hotkey_profiles")).toHaveLength(1));
    expect(ipc.callsTo("set_hotkey_profiles")[0].profiles).toEqual([expect.objectContaining({ id: "default", name: "Norsk" })]);
  });

  test("a rejected name puts the saved one back and the window's banner says why", async () => {
    const { ipc, win } = await render({
      set_hotkey_profiles: () => {
        throw backendError("invalid_profile", "Profile names must be unique");
      },
    });
    const user = userEvent.setup();
    const name = screen.getByRole("textbox", { name: "Profile name" });

    await user.clear(name);
    await user.type(name, "Norsk");
    await user.tab();

    await waitFor(() => expect(name).toHaveValue("Svenska"));
    expect(ipc.callsTo("set_hotkey_profiles")).toHaveLength(1);
    expect(win.session.error).toBe("Profile names must be unique");
  });
});

describe("a profile's speech model", () => {
  test("choosing a model sends set_profile_model", async () => {
    const { ipc } = await render();

    await userEvent.setup().selectOptions(screen.getByRole("combobox", { name: "Model for Svenska" }), kbWhisperMedium.id);

    await waitFor(() => expect(ipc.callsTo("set_profile_model")).toEqual([{ profileId: "default", modelId: kbWhisperMedium.id }]));
  });

  test("a rejected model shows the previous choice again, with the message under the profile", async () => {
    await render({
      set_profile_model: () => {
        throw backendError("model_unavailable", "That model cannot be used for Swedish");
      },
    });
    const model = screen.getByRole("combobox", { name: "Model for Svenska" });

    await userEvent.setup().selectOptions(model, kbWhisperMedium.id);

    expect(await screen.findByText("That model cannot be used for Swedish")).toBeInTheDocument();
    expect(model).toHaveValue("auto");
  });
});

describe("a profile whose model is not downloaded", () => {
  const withMissingModel = { get_profile_model_info: () => missingModel };

  test("offers the download, shows its progress and ends with the model ready", async () => {
    const download = deferred<void>();
    let downloaded = false;
    const { ipc } = await render({
      get_profile_model_info: () => (downloaded ? kbWhisperMedium : missingModel),
      download_model: () => download.promise,
    });
    expect(await screen.findByText("KB-Whisper Medium required · 1530 MB")).toBeInTheDocument();

    await userEvent.setup().click(screen.getByRole("button", { name: "Download speech engine" }));

    await waitFor(() => expect(ipc.callsTo("download_model")).toEqual([{ whisperModel: kbWhisperMedium.id }]));
    await waitFor(() => expect(ipc.listenerCount("model-download-progress")).toBeGreaterThan(0));
    ipc.emit("model-download-progress", { progress: 42.4 });
    expect(await screen.findByRole("button", { name: "Downloading 42%" })).toBeDisabled();

    downloaded = true;
    download.resolve();
    expect(await screen.findByText("KB-Whisper Medium · Ready")).toBeInTheDocument();
    await waitFor(() => expect(screen.queryByRole("button", { name: /Downloading|Download speech engine/ })).not.toBeInTheDocument());
  });

  test("a failed download shows its message under the profile and offers the download again", async () => {
    await render({
      ...withMissingModel,
      download_model: () => {
        throw backendError("download_failed", "No space left on the disk");
      },
    });

    await userEvent.setup().click(await screen.findByRole("button", { name: "Download speech engine" }));

    expect(await screen.findByText("No space left on the disk")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Download speech engine" })).toBeEnabled();
  });
});

describe("auto-paste", () => {
  test("on Windows the switch saves auto-paste on", async () => {
    const { ipc } = await render();

    await userEvent.setup().click(screen.getByRole("switch", { name: "Auto-paste transcription" }));

    await waitFor(() => expect(ipc.callsTo("set_auto_paste")).toEqual([{ enabled: true }]));
  });

  test("on macOS a refused Accessibility permission keeps it off and says it is required", async () => {
    const { ipc, win } = await render({
      get_platform: () => "macos",
      check_accessibility_permission: () => false,
      request_accessibility_permission: () => undefined,
    });
    const user = userEvent.setup();
    expect(screen.queryByText(/Requires Accessibility permission/)).not.toBeInTheDocument();

    await user.click(screen.getByRole("switch", { name: "Auto-paste transcription" }));

    expect(await screen.findByText(/Requires Accessibility permission/)).toBeInTheDocument();
    await waitFor(() => expect(ipc.callsTo("set_auto_paste")).toEqual([{ enabled: false }]));
    expect(screen.getByRole("switch", { name: "Auto-paste transcription" })).toHaveAttribute("aria-checked", "false");
    await waitFor(() => expect(win.session.error).toBe("Accessibility permission was not granted. Auto-paste remains off."));
  });
});

describe("the hotkey registration health", () => {
  const unregistered = {
    ok: false,
    error: "Ctrl+Shift+Space is used by another app",
    shortcut: "Ctrl+Shift+Space",
    shortcuts: [],
    settings_watcher: { running: true, recovered_panics: 0 },
  };

  test("a registration that failed shows Not registered with the reason until a later event says it is bound", async () => {
    const { ipc } = await render({ hotkey_status: () => unregistered });

    expect(await screen.findByText(/Not registered: Ctrl\+Shift\+Space is used by another app/)).toBeInTheDocument();

    await waitFor(() => expect(ipc.listenerCount("hotkey-registration-changed")).toBeGreaterThan(0));
    ipc.emit("hotkey-registration-changed", { ...unregistered, ok: true, error: null, shortcuts: ["Ctrl+Shift+Space"] });

    await waitFor(() => expect(screen.queryByText(/Not registered/)).not.toBeInTheDocument());
  });
});

describe("Try it out", () => {
  test("records, stops, shows the transcription and copies it", async () => {
    const { ipc } = await render({
      start_recording: () => undefined,
      stop_and_transcribe: () => "Hej världen",
      copy_transcription_text: () => undefined,
    });
    const user = userEvent.setup();

    await user.click(screen.getByRole("button", { name: "Start recording" }));
    await waitFor(() => expect(ipc.callsTo("start_recording")).toHaveLength(1));
    await user.click(await screen.findByRole("button", { name: "Stop recording" }));

    await waitFor(() => expect(ipc.callsTo("stop_and_transcribe")).toHaveLength(1));
    const result = screen.getByPlaceholderText(/use your hotkey/);
    await waitFor(() => expect(result).toHaveValue("Hej världen"));
    expect(screen.getByRole("button", { name: "Start recording" })).toBeEnabled();

    await user.click(screen.getByRole("button", { name: "Copy result" }));

    expect(await screen.findByText("Copied to clipboard.")).toBeInTheDocument();
    expect(ipc.callsTo("copy_transcription_text")).toEqual([{ text: "Hej världen" }]);
  });

  test("a cancelled save says nothing was written", async () => {
    const { ipc } = await render({
      get_last_transcription: () => "Hej världen",
      save_transcription_text: () => false,
    });

    await userEvent.setup().click(await screen.findByRole("button", { name: "Save result…" }));

    expect(await screen.findByText("Save cancelled — nothing was written.")).toBeInTheDocument();
    expect(ipc.callsTo("save_transcription_text")).toEqual([{ text: "Hej världen", fileName: "dictation.txt", directory: null }]);
  });

  test("a dictation by hotkey disables the button as Transcribing until the state is idle again", async () => {
    const { ipc } = await render();
    await waitFor(() => expect(ipc.listenerCount("state-changed")).toBeGreaterThan(0));

    ipc.emit("state-changed", "transcribing");

    const busy = await screen.findByRole("button", { name: "Transcribing..." });
    expect(busy).toBeDisabled();

    ipc.emit("state-changed", "idle");

    expect(await screen.findByRole("button", { name: "Start recording" })).toBeEnabled();
    expect(screen.queryByRole("button", { name: "Transcribing..." })).not.toBeInTheDocument();
  });
});

describe("while the window cannot confirm what is saved", () => {
  test("the controls that save a setting are disabled", async () => {
    const { win } = await render({
      get_settings: () => settingsValue({ hotkey_profiles: [settingsValue().hotkey_profiles[0]] }),
      get_profile_model_info: () => pianissimo,
    });
    const controls = () => [
      screen.getByRole("button", { name: "+ Add profile" }),
      screen.getByRole("textbox", { name: "Profile name" }),
      screen.getByRole("combobox", { name: "Svenska language" }),
      screen.getByRole("combobox", { name: "Model for Svenska" }),
      screen.getByRole("switch", { name: "Auto-paste transcription" }),
    ];
    for (const control of controls()) expect(control).toBeEnabled();

    win.session.unconfirmed = { afterSave: true, detail: "backend unreachable" };

    await waitFor(() => {
      for (const control of controls()) expect(control).toBeDisabled();
    });
  });
});
