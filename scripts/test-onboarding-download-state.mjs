import assert from "node:assert/strict";
import test from "node:test";

import {
  awaitDownloadCompletion,
  completedDownloadState,
} from "../src/lib/onboarding-download-state.js";

const expected = {
  downloading: false,
  downloadComplete: true,
  downloadProgress: 100,
};

test("fast download completion is authoritative without an event", async () => {
  assert.deepEqual(await awaitDownloadCompletion(Promise.resolve()), expected);
});

test("completion state is published only after the command resolves", async () => {
  let resolveDownload;
  const download = new Promise((resolve) => {
    resolveDownload = resolve;
  });
  let completed = false;
  const pending = awaitDownloadCompletion(download).then((state) => {
    completed = true;
    return state;
  });

  await Promise.resolve();
  assert.equal(completed, false);
  resolveDownload();
  assert.deepEqual(await pending, expected);
});

test("failed downloads do not publish successful state", async () => {
  await assert.rejects(
    awaitDownloadCompletion(Promise.reject(new Error("download failed"))),
    /download failed/,
  );
});

test("event and command completion are idempotent", () => {
  assert.deepEqual(completedDownloadState(), completedDownloadState());
  assert.notEqual(completedDownloadState(), completedDownloadState());
});
