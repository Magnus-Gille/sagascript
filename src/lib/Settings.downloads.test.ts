// Model downloads in Settings.svelte (#362): the Dictate profile's "Download
// speech engine", the Transcribe tab's file model download, and what the
// window shows while one runs. Pianissimo downloads through its own command.
import { backendError } from "../test/ipc";
import { screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { describe, expect, test } from "vitest";
import { deferred } from "../test/deferred";
import { kbWhisperMedium, pianissimo, renderSettings, settingsBackend, settingsValue, whisperModel } from "../test/settings";

const missing = (model: typeof pianissimo) => ({ ...model, downloaded: false });

async function openTranscribeTab() {
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "Transcribe" }));
  return user;
}

describe("a profile's speech engine download", () => {
  test("Pianissimo downloads through its own command, shows progress, then re-reads the model state", async () => {
    const download = deferred<void>();
    let downloaded = false;
    const { ipc } = await renderSettings(
      settingsBackend({
        get_profile_model_info: () => (downloaded ? pianissimo : missing(pianissimo)),
        download_pianissimo_model: () => download.promise,
      }),
    );
    const user = userEvent.setup();
    const button = await screen.findByRole("button", { name: "Download speech engine" });
    const modelQueries = ipc.callsTo("get_profile_model_info").length;

    await user.click(button);
    await waitFor(() => expect(ipc.callsTo("download_pianissimo_model")).toEqual([{}]));
    ipc.emit("model-download-progress", { progress: 40.4 });
    expect(await screen.findByRole("button", { name: "Downloading 40%" })).toBeDisabled();
    expect(screen.getByText("Downloading Pianissimo...")).toBeInTheDocument();

    downloaded = true;
    download.resolve();
    await waitFor(() => expect(ipc.callsTo("get_profile_model_info").length).toBeGreaterThan(modelQueries));
    await waitFor(() => expect(screen.queryByText("Downloading Pianissimo...")).toBeNull());
    expect(screen.queryByRole("button", { name: /^Download/ })).toBeNull();
    expect(ipc.callsTo("download_model")).toEqual([]);
  });

  test("another model downloads by its id, and a failure is reported on that profile", async () => {
    const { ipc } = await renderSettings(
      settingsBackend({
        get_profile_model_info: () => missing(kbWhisperMedium),
        download_model: () => {
          throw backendError("model_download_failed", "No space left on device");
        },
      }),
    );

    await userEvent.setup().click(await screen.findByRole("button", { name: "Download speech engine" }));

    expect(await screen.findByText("No space left on device")).toBeInTheDocument();
    expect(ipc.callsTo("download_model")).toEqual([{ whisperModel: "kb-whisper-medium" }]);
    expect(ipc.callsTo("download_pianissimo_model")).toEqual([]);
    expect(screen.getByRole("button", { name: "Download speech engine" })).toBeEnabled();
  });
});

describe("the file transcription model download", () => {
  test("a selected Pianissimo that is not downloaded downloads through its own command", async () => {
    const download = deferred<void>();
    const { ipc } = await renderSettings(
      settingsBackend({
        get_settings: () => settingsValue({ file_transcription_model: pianissimo.id }),
        get_file_model_options: () => [missing(pianissimo), kbWhisperMedium],
        download_pianissimo_model: () => download.promise,
      }),
    );
    const user = await openTranscribeTab();
    const optionQueries = ipc.callsTo("get_file_model_options").length;

    await user.click(await screen.findByRole("button", { name: "Download selected model" }));
    await waitFor(() => expect(ipc.callsTo("download_pianissimo_model")).toEqual([{}]));
    ipc.emit("model-download-progress", { progress: 25 });
    expect(await screen.findByRole("button", { name: "Downloading Pianissimo… 25%" })).toBeDisabled();

    download.resolve();
    await waitFor(() => expect(screen.queryByText("Downloading Pianissimo...")).toBeNull());
    expect(ipc.callsTo("get_file_model_options").length).toBeGreaterThan(optionQueries);
    expect(ipc.callsTo("download_model")).toEqual([]);
  });

  test("Auto downloads the recommended model, by its id", async () => {
    const { ipc } = await renderSettings(
      settingsBackend({
        get_auto_file_model_info: () => kbWhisperMedium,
        get_effective_model_info: () => missing(pianissimo),
        download_pianissimo_model: () => undefined,
      }),
    );
    const user = await openTranscribeTab();

    await user.click(await screen.findByRole("button", { name: "Download Pianissimo" }));

    await waitFor(() => expect(ipc.callsTo("download_pianissimo_model")).toEqual([{}]));
    expect(ipc.callsTo("download_model")).toEqual([]);
  });

  test("a download that fails shows its error and frees the controls", async () => {
    await renderSettings(
      settingsBackend({
        get_settings: () => settingsValue({ file_transcription_model: kbWhisperMedium.id }),
        get_file_model_options: () => [pianissimo, missing(kbWhisperMedium)],
        download_model: () => {
          throw backendError("model_download_failed", "Download interrupted");
        },
      }),
    );
    const user = await openTranscribeTab();

    await user.click(await screen.findByRole("button", { name: "Download selected model" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("Download interrupted");
    expect(screen.getByRole("button", { name: "Download selected model" })).toBeEnabled();
  });
});

describe("a download on the Dictate tab updates what the Transcribe tab offers", () => {
  // Auto resolves to Pianissimo for Swedish files. Once it is on disk the
  // Transcribe tab must stop offering to download it.
  async function renderWithPianissimoMissing(profileHandlers: (lineup: () => typeof pianissimo) => Parameters<typeof settingsBackend>[0]) {
    let onDisk = false;
    const lineup = () => (onDisk ? pianissimo : missing(pianissimo));
    await renderSettings(
      settingsBackend({
        get_file_model_options: () => [lineup(), kbWhisperMedium],
        get_effective_model_info: () => lineup(),
        download_pianissimo_model: () => {
          onDisk = true;
        },
        ...profileHandlers(lineup),
      }),
    );
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Transcribe" }));
    expect(await screen.findByRole("button", { name: "Download Pianissimo" })).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Dictate" }));
    return user;
  }

  test("downloading a profile's speech engine", async () => {
    const user = await renderWithPianissimoMissing((lineup) => ({ get_profile_model_info: () => lineup() }));

    await user.click(await screen.findByRole("button", { name: "Download speech engine" }));
    await waitFor(() => expect(screen.queryByText("Downloading Pianissimo...")).toBeNull());
    await user.click(screen.getByRole("button", { name: "Transcribe" }));

    await waitFor(() => expect(screen.queryByRole("button", { name: "Download Pianissimo" })).toBeNull());
  });

  test("switching a retired model to the recommended one downloads it first", async () => {
    const kbWhisperSmall = whisperModel("kb-whisper-small", "KB-Whisper Small", { size_mb: 466 });
    const user = await renderWithPianissimoMissing(() => ({
      get_settings: () => settingsValue({ profile_models: { default: "kb-whisper-small" } }),
      get_profile_model_info: () => kbWhisperSmall,
      set_profile_model: () => undefined,
    }));

    await user.click(await screen.findByRole("button", { name: "Switch to Pianissimo (download 640 MB)" }));
    await waitFor(() => expect(screen.queryByText("Downloading Pianissimo...")).toBeNull());
    await user.click(screen.getByRole("button", { name: "Transcribe" }));

    await waitFor(() => expect(screen.queryByRole("button", { name: "Download Pianissimo" })).toBeNull());
  });
});
