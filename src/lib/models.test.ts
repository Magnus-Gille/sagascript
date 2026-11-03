import { describe, expect, test } from "vitest";
import {
  ModelDownloads,
  PIANISSIMO_DA_ID,
  PIANISSIMO_ID,
  downloadModelById,
  firstPianissimoId,
  isPianissimoId,
  pianissimoIdForLanguage,
} from "./models.svelte";
import { backendError, mockIpc } from "../test/ipc";

describe("downloadModelById", () => {
  test("Pianissimo goes through its own command", async () => {
    const ipc = mockIpc({ download_pianissimo_model: () => undefined });

    await downloadModelById(PIANISSIMO_ID);

    expect(ipc.calls).toEqual([{ command: "download_pianissimo_model", args: {} }]);
  });

  test("Danish Pianissimo goes through the same command, naming its model", async () => {
    const ipc = mockIpc({ download_pianissimo_model: () => undefined });

    await downloadModelById(PIANISSIMO_DA_ID);

    expect(ipc.calls).toEqual([{ command: "download_pianissimo_model", args: { model: "pianissimo-da" } }]);
  });

  test("an engine repair downloads the model the error is about", () => {
    // A Danish profile on Auto resolves to pianissimo-da: that is what to repair,
    // not the Swedish model, and not a file preference the dictation never used.
    expect(firstPianissimoId(["small", PIANISSIMO_DA_ID, PIANISSIMO_ID])).toBe(PIANISSIMO_DA_ID);
    expect(firstPianissimoId([undefined, null, "auto"])).toBe(PIANISSIMO_ID);
  });

  test("each language has at most one Pianissimo model, its own", () => {
    expect(pianissimoIdForLanguage("sv")).toBe(PIANISSIMO_ID);
    expect(pianissimoIdForLanguage("da")).toBe(PIANISSIMO_DA_ID);
    for (const language of ["en", "no", "fi", "auto", null, undefined]) expect(pianissimoIdForLanguage(language)).toBeNull();
    // A failed Danish job is repaired with the Danish model even when the file preference is Swedish.
    expect(firstPianissimoId([pianissimoIdForLanguage("da"), PIANISSIMO_ID])).toBe(PIANISSIMO_DA_ID);
  });

  test("only the Pianissimo ids are Pianissimo", () => {
    expect([PIANISSIMO_ID, PIANISSIMO_DA_ID].every(isPianissimoId)).toBe(true);
    expect(["pianissimo", "pianissimo-sv-coreml", "small", "", null, undefined].some(isPianissimoId)).toBe(false);
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
