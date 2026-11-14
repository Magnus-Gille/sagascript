// DownloadStatusBar on its own (SAGA-379): the bar that shows the one model
// download that runs, with its name and progress. The same area through the
// whole window: src/lib/Settings.downloads.test.ts and src/lib/Settings.test.ts.
import { screen, waitFor } from "@testing-library/svelte";
import { describe, expect, test } from "vitest";
import { deferred } from "../../test/deferred";
import { settingsBackend } from "../../test/settings";
import { renderPanel } from "../../test/settingsPanel";
import DownloadStatusBar from "./DownloadStatusBar.svelte";

describe("the download status bar", () => {
  test("shows nothing while no model downloads", async () => {
    await renderPanel(DownloadStatusBar);

    expect(screen.queryByText(/^Downloading /)).toBeNull();
    expect(screen.queryByText(/%$/)).toBeNull();
  });

  test("shows the model's name and the rounded progress while it downloads, and goes when it ends", async () => {
    const download = deferred<void>();
    const { ipc, win } = await renderPanel(DownloadStatusBar, settingsBackend({ download_model: () => download.promise }));

    const finished = win.downloads.downloadFileModel("kb-whisper-small", "KB Whisper Small", async () => undefined);
    await waitFor(() => expect(ipc.callsTo("download_model")).toEqual([{ whisperModel: "kb-whisper-small" }]));
    expect(await screen.findByText("Downloading KB Whisper Small...")).toBeInTheDocument();

    ipc.emit("model-download-progress", { progress: 12.4 });
    expect(await screen.findByText("12%")).toBeInTheDocument();
    ipc.emit("model-download-progress", { progress: 40.6 });
    expect(await screen.findByText("41%")).toBeInTheDocument();
    expect(screen.queryByText("12%")).toBeNull();

    download.resolve();
    await finished;
    await waitFor(() => expect(screen.queryByText(/^Downloading /)).toBeNull());
    expect(screen.queryByText("41%")).toBeNull();
  });
});
