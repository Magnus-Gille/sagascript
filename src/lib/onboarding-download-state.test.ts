import { expect, test } from "vitest";
import { awaitDownloadCompletion, completedDownloadState, type DownloadCompletionState } from "./onboarding-download-state";

const expected = {
  downloading: false,
  downloadComplete: true,
  downloadProgress: 100,
};

test("fast download completion is authoritative without an event", async () => {
  let state: DownloadCompletionState | undefined;
  expect(
    await awaitDownloadCompletion(Promise.resolve(), (completed) => {
      state = completed;
    }),
  ).toStrictEqual(expected);
  expect(state).toStrictEqual(expected);
});

test("completion state is published only after the command resolves", async () => {
  let resolveDownload!: () => void;
  const download = new Promise<void>((resolve) => {
    resolveDownload = resolve;
  });
  let completed = false;
  const pending = awaitDownloadCompletion(download, () => {
    completed = true;
  });

  await Promise.resolve();
  expect(completed).toBe(false);
  resolveDownload();
  expect(await pending).toStrictEqual(expected);
});

test("failed downloads do not publish successful state", async () => {
  await expect(
    awaitDownloadCompletion(Promise.reject(new Error("download failed")), () => {
      throw new Error("completion callback must not run");
    }),
  ).rejects.toThrow(/download failed/);
});

test("event and command completion share an idempotent state transition", async () => {
  let state: DownloadCompletionState = { downloading: true, downloadComplete: false, downloadProgress: 20 };
  const markComplete = () => {
    state = completedDownloadState();
  };
  // Simulate model-ready arriving before the command promise resolves.
  markComplete();
  await awaitDownloadCompletion(Promise.resolve(), markComplete);
  expect(state).toStrictEqual(expected);
});
