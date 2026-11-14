// TranscribeTab.svelte on its own (SAGA-379): the profile and file model pickers,
// the queue of files and its result tabs, re-running a file, and what an update in
// preparation disables. The same area through the whole window: Settings.test.ts,
// Settings.catalog.test.ts, Settings.downloads.test.ts, Settings.meetingFlows.test.ts.
import { screen, waitFor, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { describe, expect, test } from "vitest";
import { deferred } from "../../test/deferred";
import { backendError, type CommandHandlers, type CommandName } from "../../test/ipc";
import { renderPanel } from "../../test/settingsPanel";
import { kbWhisperMedium, pianissimo, settingsBackend, settingsValue, swedishProfile } from "../../test/settings";
import type { HotkeyProfile } from "../api";
import TranscribeTab from "./TranscribeTab.svelte";

const englishProfile: HotkeyProfile = {
  id: "english",
  name: "English",
  shortcut: "Ctrl+Alt+E",
  language: "en",
  push_to_talk_shortcut: "Ctrl+Alt+E",
  toggle_shortcut: null,
};
const missing = (model: typeof pianissimo) => ({ ...model, downloaded: false });
// A hidden element has no accessible name, so the region is found by its label once it is hidden.
const region = () => screen.getByLabelText("File transcription");
const resultTabs = () => screen.getAllByRole("tab");
const openFiles = () => screen.getByRole("button", { name: "Open Files..." });
const openSavedMeeting = () => screen.getByRole("button", { name: "Open saved meeting..." });

/** The backend of a panel test: the window's default one, with the commands a finished file asks for answered. */
function backend(overrides: CommandHandlers = {}): CommandHandlers {
  return settingsBackend({ set_update_result_pending: () => undefined, ...overrides });
}

/** Answer the dialog plugin's `open` with `paths` (the IPC mock types only the app's own commands). */
function answerFilePicker(ipc: { handle: (command: CommandName, handler: never) => void }, paths: string[] | null): void {
  ipc.handle("plugin:dialog|open" as CommandName, (() => paths) as never);
}

describe("while its tab is hidden", () => {
  test("it keeps a running transcription, shows the result when the tab is shown again, and never transcribes twice", async () => {
    const transcription = deferred<string>();
    const { ipc, win, setActive } = await renderPanel(TranscribeTab, backend({ transcribe_file: () => transcription.promise }));

    win.queue.addFiles(["/audio/one.wav"]);
    await waitFor(() => expect(ipc.callsTo("transcribe_file")).toHaveLength(1));
    expect(region()).not.toHaveAttribute("hidden");
    await setActive(false);
    expect(region()).toHaveAttribute("hidden");

    transcription.resolve("Transcript of one");
    await waitFor(() => expect(win.queue.jobs[0].status).toBe("completed"));
    await setActive(true);

    expect(region()).not.toHaveAttribute("hidden");
    expect(await screen.findByDisplayValue("Transcript of one")).toBeInTheDocument();
    expect(ipc.callsTo("transcribe_file")).toHaveLength(1);
  });
});

describe("the profile", () => {
  test("every profile is offered as its name and language, and choosing one changes the language shown above", async () => {
    await renderPanel(TranscribeTab, backend({ get_settings: () => settingsValue({ hotkey_profiles: [swedishProfile, englishProfile] }) }));
    const user = userEvent.setup();
    const profile = screen.getByRole("combobox", { name: "Profile (optional)" });

    expect(
      within(profile)
        .getAllByRole("option")
        .map((option) => option.textContent),
    ).toEqual(["Svenska · Swedish", "English · English"]);
    expect(screen.getByRole("button", { name: /Language Swedish/ })).toBeInTheDocument();

    await user.selectOptions(profile, "english");

    expect(screen.getByRole("button", { name: /Language English/ })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: /Language Swedish/ })).toBeNull();
  });
});

describe("the file transcription model", () => {
  test("it offers Auto and each of the backend's models with its size and whether it is on disk", async () => {
    await renderPanel(TranscribeTab, backend({ get_file_model_options: () => [pianissimo, missing(kbWhisperMedium)] }));
    const select = screen.getByRole("combobox", { name: "File transcription model" });

    await waitFor(() =>
      expect(
        within(select)
          .getAllByRole("option")
          .map((option) => option.textContent),
      ).toEqual(["Auto — Pianissimo (recommended)", "Pianissimo · 640 MB · ready", "KB-Whisper Medium · 1530 MB · download needed"]),
    );
  });

  test("choosing a model saves it", async () => {
    const { ipc } = await renderPanel(TranscribeTab, backend({ set_file_transcription_model: () => undefined }));
    const select = screen.getByRole("combobox", { name: "File transcription model" });
    await waitFor(() => expect(within(select).getAllByRole("option")).toHaveLength(3));

    await userEvent.setup().selectOptions(select, "kb-whisper-medium");

    await waitFor(() => expect(ipc.callsTo("set_file_transcription_model")).toEqual([{ modelId: "kb-whisper-medium" }]));
  });

  test("a model the backend rejects is put back, and the message shows as an alert under the select", async () => {
    const { ipc } = await renderPanel(
      TranscribeTab,
      backend({
        set_file_transcription_model: () => {
          throw backendError("settings_write_failed", "The settings file is read-only.");
        },
      }),
    );
    const select = screen.getByRole("combobox", { name: "File transcription model" });
    await waitFor(() => expect(within(select).getAllByRole("option")).toHaveLength(3));

    await userEvent.setup().selectOptions(select, "kb-whisper-medium");

    expect(await screen.findByRole("alert")).toHaveTextContent("The settings file is read-only.");
    expect(ipc.callsTo("set_file_transcription_model")).toHaveLength(1);
    expect(select).toHaveValue("auto");
  });

  test("a selected model that is not downloaded is downloaded on request, and the lineup is read again", async () => {
    const download = deferred<void>();
    const { ipc } = await renderPanel(
      TranscribeTab,
      backend({
        get_settings: () => settingsValue({ file_transcription_model: kbWhisperMedium.id }),
        get_file_model_options: () => [pianissimo, missing(kbWhisperMedium)],
        download_model: () => download.promise,
      }),
    );
    const readsBefore = ipc.callsTo("get_file_model_options").length;

    await userEvent.setup().click(await screen.findByRole("button", { name: "Download selected model" }));

    await waitFor(() => expect(ipc.callsTo("download_model")).toEqual([{ whisperModel: "kb-whisper-medium" }]));
    expect(await screen.findByRole("button", { name: /^Downloading KB-Whisper Medium/ })).toBeDisabled();
    download.resolve();
    await waitFor(() => expect(ipc.callsTo("get_file_model_options").length).toBeGreaterThan(readsBefore));
    await waitFor(() => expect(screen.getByRole("button", { name: "Download selected model" })).toBeEnabled());
  });

  test("a model that is on disk offers no download", async () => {
    await renderPanel(TranscribeTab, backend({ get_settings: () => settingsValue({ file_transcription_model: kbWhisperMedium.id }) }));
    const select = screen.getByRole("combobox", { name: "File transcription model" });
    await waitFor(() => expect(within(select).getAllByRole("option")).toHaveLength(3));

    expect(select).toHaveValue("kb-whisper-medium");
    expect(screen.queryByRole("button", { name: "Download selected model" })).toBeNull();
  });

  test("with Pianissimo selected, speaker diarization and the extra context are disabled", async () => {
    await renderPanel(TranscribeTab, backend({ get_settings: () => settingsValue({ file_transcription_model: pianissimo.id }) }));

    expect(screen.getByRole("checkbox", { name: "Speaker diarization" })).toBeDisabled();
    expect(screen.getByRole("textbox", { name: "Extra context for this file" })).toBeDisabled();
  });

  test("with another model selected, speaker diarization and the extra context can be used", async () => {
    await renderPanel(TranscribeTab, backend({ get_settings: () => settingsValue({ file_transcription_model: kbWhisperMedium.id }) }));

    expect(screen.getByRole("checkbox", { name: "Speaker diarization" })).toBeEnabled();
    expect(screen.getByRole("textbox", { name: "Extra context for this file" })).toBeEnabled();
  });

  test("choosing Pianissimo unticks speaker diarization and empties the extra context", async () => {
    let current = settingsValue({ file_transcription_model: kbWhisperMedium.id });
    await renderPanel(
      TranscribeTab,
      backend({
        get_settings: () => current,
        set_file_transcription_model: ({ modelId }) => {
          current = { ...current, file_transcription_model: modelId };
        },
      }),
    );
    const user = userEvent.setup();
    const select = screen.getByRole("combobox", { name: "File transcription model" });
    await waitFor(() => expect(within(select).getAllByRole("option")).toHaveLength(3));
    const diarization = screen.getByRole("checkbox", { name: "Speaker diarization" });
    const context = screen.getByRole("textbox", { name: "Extra context for this file" });
    await user.click(diarization);
    await user.type(context, "Astrid, Grimnir");
    expect(diarization).toBeChecked();
    expect(context).toHaveValue("Astrid, Grimnir");

    await user.selectOptions(select, pianissimo.id);

    await waitFor(() => expect(diarization).toBeDisabled());
    expect(diarization).not.toBeChecked();
    expect(context).toHaveValue("");
  });
});

describe("opening files", () => {
  test("Open Files... queues the chosen files, one result tab each, the first of them selected", async () => {
    const holds = [deferred<string>(), deferred<string>()];
    const { ipc } = await renderPanel(
      TranscribeTab,
      backend({ transcribe_file: ({ filePath }) => holds[filePath.endsWith("one.wav") ? 0 : 1].promise }),
    );
    answerFilePicker(ipc, ["/audio/one.wav", "/audio/two.wav"]);

    await userEvent.setup().click(openFiles());

    expect(await screen.findByRole("tablist", { name: "Transcription results" })).toBeInTheDocument();
    expect(resultTabs().map((tab) => tab.title)).toEqual(["/audio/one.wav", "/audio/two.wav"]);
    expect(resultTabs().map((tab) => tab.getAttribute("aria-selected"))).toEqual(["true", "false"]);
    await waitFor(() => expect(ipc.callsTo("transcribe_file")).toHaveLength(1));
    expect(ipc.callsTo("transcribe_file")[0].filePath).toBe("/audio/one.wav");
  });

  test("a cancelled file dialog queues nothing", async () => {
    const { ipc } = await renderPanel(TranscribeTab, backend());
    answerFilePicker(ipc, null);

    await userEvent.setup().click(openFiles());

    await waitFor(() => expect(openFiles()).toBeEnabled());
    expect(screen.queryByRole("tablist", { name: "Transcription results" })).toBeNull();
    expect(ipc.callsTo("transcribe_file")).toHaveLength(0);
  });

  test("the arrow keys, Home and End move the selection between the result tabs", async () => {
    const hold = deferred<string>();
    const { win } = await renderPanel(TranscribeTab, backend({ transcribe_file: () => hold.promise }));
    win.queue.addFiles(["/audio/one.wav", "/audio/two.wav", "/audio/three.wav"]);
    const user = userEvent.setup();
    const selected = () => resultTabs().find((tab) => tab.getAttribute("aria-selected") === "true")?.title;
    await waitFor(() => expect(selected()).toBe("/audio/one.wav"));

    resultTabs()[0].focus();
    await user.keyboard("{ArrowRight}");
    expect(selected()).toBe("/audio/two.wav");
    expect(resultTabs()[1]).toHaveFocus();

    await user.keyboard("{End}");
    expect(selected()).toBe("/audio/three.wav");

    await user.keyboard("{ArrowRight}");
    expect(selected()).toBe("/audio/one.wav");

    await user.keyboard("{ArrowLeft}");
    expect(selected()).toBe("/audio/three.wav");

    await user.keyboard("{Home}");
    expect(selected()).toBe("/audio/one.wav");
    expect(resultTabs()[0]).toHaveFocus();
  });

  test("Open saved meeting... adds a job named Saved review", async () => {
    const { ipc } = await renderPanel(TranscribeTab, backend({ open_meeting_review: () => null }));

    await userEvent.setup().click(openSavedMeeting());

    await waitFor(() => expect(ipc.callsTo("open_meeting_review")).toEqual([{}]));
    expect(await screen.findByRole("tab", { name: /Saved review/ })).toBeInTheDocument();
    expect(ipc.callsTo("transcribe_file")).toHaveLength(0);
  });
});

describe("running a file again", () => {
  test("Re-run and the recent files appear after a file ran, and Re-run queues the same file as a new job", async () => {
    const { ipc, win } = await renderPanel(TranscribeTab, backend({ transcribe_file: ({ filePath }) => `Transcript of ${filePath}` }));
    expect(screen.queryByRole("button", { name: "Re-run" })).toBeNull();

    win.queue.addFiles(["/audio/one.wav"]);
    await screen.findByDisplayValue("Transcript of /audio/one.wav");

    const recent = screen.getByRole("combobox", { name: /Recent files to re-run/ });
    expect(
      within(recent)
        .getAllByRole("option")
        .map((option) => option.textContent),
    ).toEqual(["one.wav"]);
    await userEvent.setup().click(screen.getByRole("button", { name: "Re-run" }));

    await waitFor(() => expect(ipc.callsTo("transcribe_file")).toHaveLength(2));
    expect(ipc.callsTo("transcribe_file").map((call) => call.filePath)).toEqual(["/audio/one.wav", "/audio/one.wav"]);
    expect(resultTabs()).toHaveLength(2);
    expect(resultTabs()[1]).toHaveAttribute("aria-selected", "true");
  });

  test("when the profile the file ran with is gone, Re-run says so and queues nothing", async () => {
    let profiles = [swedishProfile, englishProfile];
    const { ipc, win } = await renderPanel(
      TranscribeTab,
      backend({
        get_settings: () => settingsValue({ hotkey_profiles: profiles }),
        transcribe_file: ({ filePath }) => `Transcript of ${filePath}`,
      }),
    );
    const user = userEvent.setup();
    await user.selectOptions(screen.getByRole("combobox", { name: "Profile (optional)" }), "english");
    win.queue.addFiles(["/audio/one.wav"]);
    await screen.findByDisplayValue("Transcript of /audio/one.wav");
    expect(ipc.callsTo("transcribe_file")[0].profileId).toBe("english");

    // The profile is removed in the settings file, which the window then reloads.
    profiles = [swedishProfile];
    await win.session.sync();
    await user.click(screen.getByRole("button", { name: "Re-run" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("The profile for this file is no longer available.");
    expect(ipc.callsTo("transcribe_file")).toHaveLength(1);
    expect(resultTabs()).toHaveLength(1);
  });
});

describe("while an update is being prepared", () => {
  test("Open Files... and Open saved meeting... are disabled", async () => {
    const { ipc, win } = await renderPanel(
      TranscribeTab,
      backend({ complete_update_preparation: () => undefined, save_update_recovery: () => undefined }),
    );
    expect(openFiles()).toBeEnabled();
    expect(openSavedMeeting()).toBeEnabled();

    ipc.emit("update-preparing", "nonce-1");

    await waitFor(() => expect(win.recovery.preparing).toBe(true));
    expect(openFiles()).toBeDisabled();
    expect(openSavedMeeting()).toBeDisabled();
  });
});
