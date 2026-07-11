import assert from "node:assert/strict";
import test from "node:test";

import {
  adoptDownloadListeners,
  awaitDownloadCompletion,
  completedDownloadState,
  registerDownloadListeners,
} from "../src/lib/onboarding-download-state.js";

const expected = {
  downloading: false,
  downloadComplete: true,
  downloadProgress: 100,
};

test("fast download completion is authoritative without an event", async () => {
  let state;
  assert.deepEqual(
    await awaitDownloadCompletion(Promise.resolve(), (completed) => {
      state = completed;
    }),
    expected,
  );
  assert.deepEqual(state, expected);
});

test("completion state is published only after the command resolves", async () => {
  let resolveDownload;
  const download = new Promise((resolve) => {
    resolveDownload = resolve;
  });
  let completed = false;
  const pending = awaitDownloadCompletion(download, () => {
    completed = true;
  });

  await Promise.resolve();
  assert.equal(completed, false);
  resolveDownload();
  assert.deepEqual(await pending, expected);
});

test("failed downloads do not publish successful state", async () => {
  await assert.rejects(
    awaitDownloadCompletion(Promise.reject(new Error("download failed")), () => {
      throw new Error("completion callback must not run");
    }),
    /download failed/,
  );
});

test("event and command completion share an idempotent state transition", async () => {
  let state = { downloading: true, downloadComplete: false, downloadProgress: 20 };
  const markComplete = () => {
    state = completedDownloadState();
  };
  // Simulate model-ready arriving before the command promise resolves.
  markComplete();
  await awaitDownloadCompletion(Promise.resolve(), markComplete);
  assert.deepEqual(state, expected);
});

test("listener registration starts before initialization and returns both owners", async () => {
  const calls = [];
  const listeners = registerDownloadListeners(
    async () => {
      calls.push("progress");
      return () => calls.push("unlisten-progress");
    },
    async () => {
      calls.push("ready");
      return () => calls.push("unlisten-ready");
    },
    () => false,
  );
  calls.push("initialize-after-registration-started");

  assert.deepEqual(calls, ["progress", "ready", "initialize-after-registration-started"]);
  const owned = await listeners;
  assert.equal(owned.length, 2);
  owned.forEach((unlisten) => unlisten());
});

test("partial registration failure cleans up the successful listener", async () => {
  let cleaned = false;
  await assert.rejects(
    registerDownloadListeners(
      async () => () => {
        cleaned = true;
      },
      async () => {
        throw new Error("ready registration failed");
      },
      () => false,
    ),
    /ready registration failed/,
  );
  assert.equal(cleaned, true);
});

test("destroyed component immediately releases late registrations", async () => {
  let cancelled = false;
  let cleaned = 0;
  let resolveProgress;
  let resolveReady;
  const progressRegistered = new Promise((resolve) => {
    resolveProgress = resolve;
  });
  const readyRegistered = new Promise((resolve) => {
    resolveReady = resolve;
  });
  const pending = registerDownloadListeners(
    () => progressRegistered,
    () => readyRegistered,
    () => cancelled,
  );
  cancelled = true;
  resolveProgress(() => cleaned++);
  resolveReady(() => cleaned++);

  assert.equal(await pending, null);
  assert.equal(cleaned, 2);
});

test("destruction before ownership adoption releases both listeners", () => {
  let cleaned = 0;
  let adopted = false;
  const listeners = [
    () => cleaned++,
    () => cleaned++,
  ];

  assert.equal(
    adoptDownloadListeners(listeners, () => true, () => {
      adopted = true;
    }),
    false,
  );
  assert.equal(cleaned, 2);
  assert.equal(adopted, false);
});

test("live component atomically adopts both listener owners", () => {
  const listeners = [() => {}, () => {}];
  let adopted;
  assert.equal(
    adoptDownloadListeners(listeners, () => false, (progress, ready) => {
      adopted = [progress, ready];
    }),
    true,
  );
  assert.deepEqual(adopted, listeners);
});
