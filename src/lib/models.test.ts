import { describe, expect, test } from "vitest";
import { ModelDownloads, PIANISSIMO_ID, downloadModelById } from "./models.svelte";
import { backendError, mockIpc } from "../test/ipc";

describe("downloadModelById", () => {
  test("Pianissimo goes through its own command", async () => {
    const ipc = mockIpc({ download_pianissimo_model: () => undefined });

    await downloadModelById(PIANISSIMO_ID);

    expect(ipc.calls).toEqual([{ command: "download_pianissimo_model", args: {} }]);
  });

  test("every other model goes through download_model by id", async () => {
    const ipc = mockIpc({ download_model: () => undefined });

    await downloadModelById("kb-whisper-medium");
    await downloadModelById("base.en");

    expect(ipc.calls).toEqual([
      { command: "download_model", args: { whisperModel: "kb-whisper-medium" } },
      { command: "download_model", args: { whisperModel: "base.en" } },
    ]);
  });

  test("a model whose id merely starts like Pianissimo is not Pianissimo", async () => {
    const ipc = mockIpc({ download_model: () => undefined });

    await downloadModelById(`${PIANISSIMO_ID}-coreml`);

    expect(ipc.callsTo("download_model")).toEqual([{ whisperModel: `${PIANISSIMO_ID}-coreml` }]);
  });

  test("the backend's refusal reaches the caller", async () => {
    mockIpc({
      download_pianissimo_model: () => {
        throw backendError("pianissimo_unavailable", "Pianissimo is not supported on this OS");
      },
    });

    await expect(downloadModelById(PIANISSIMO_ID)).rejects.toEqual(
      backendError("pianissimo_unavailable", "Pianissimo is not supported on this OS"),
    );
  });
});

describe("ModelDownloads", () => {
  test("is idle until a download starts, and one download at a time shows as busy", () => {
    const downloads = new ModelDownloads();
    expect(downloads).toMatchObject({ id: null, name: "", progress: 0, busy: false });

    downloads.start("kb-whisper-medium", "KB-Whisper Medium");

    expect(downloads).toMatchObject({ id: "kb-whisper-medium", name: "KB-Whisper Medium", progress: 0, busy: true });
  });

  test("a new download starts at 0%, not where the last one stopped", () => {
    const downloads = new ModelDownloads();
    downloads.start("a", "A");
    downloads.progress = 80;

    downloads.start("b", "B");

    expect(downloads.progress).toBe(0);
  });

  test("finishing, after success or failure, frees it and resets the progress", () => {
    const downloads = new ModelDownloads();
    downloads.start("a", "A");
    downloads.progress = 99;

    downloads.finish();

    expect(downloads).toMatchObject({ id: null, progress: 0, busy: false });
  });
});
