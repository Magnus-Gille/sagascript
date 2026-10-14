import { expect, test } from "vitest";
import {
  adoptDownloadListeners,
  awaitDownloadCompletion,
  completedDownloadState,
  registerDownloadListeners,
  type DownloadCompletionState,
  type DownloadListeners,
  type Unlisten,
} from "./onboarding-download-state";

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

test("listener registration starts before initialization and returns both owners", async () => {
  const calls: string[] = [];
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

  expect(calls).toStrictEqual(["progress", "ready", "initialize-after-registration-started"]);
  const owned = await listeners;
  expect(owned!.length).toBe(2);
  owned!.forEach((unlisten) => unlisten());
});

test("partial registration failure cleans up the successful listener", async () => {
  let cleaned = false;
  await expect(
    registerDownloadListeners(
      async () => () => {
        cleaned = true;
      },
      async () => {
        throw new Error("ready registration failed");
      },
      () => false,
    ),
  ).rejects.toThrow(/ready registration failed/);
  expect(cleaned).toBe(true);
});

test("destroyed component immediately releases late registrations", async () => {
  let cancelled = false;
  let cleaned = 0;
  let resolveProgress!: (unlisten: Unlisten) => void;
  let resolveReady!: (unlisten: Unlisten) => void;
  const progressRegistered = new Promise<Unlisten>((resolve) => {
    resolveProgress = resolve;
  });
  const readyRegistered = new Promise<Unlisten>((resolve) => {
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

  expect(await pending).toBe(null);
  expect(cleaned).toBe(2);
});

test("destruction before ownership adoption releases both listeners", () => {
  let cleaned = 0;
  let adopted = false;
  const listeners: DownloadListeners = [() => cleaned++, () => cleaned++];

  expect(
    adoptDownloadListeners(
      listeners,
      () => true,
      () => {
        adopted = true;
      },
    ),
  ).toBe(false);
  expect(cleaned).toBe(2);
  expect(adopted).toBe(false);
});

test("live component atomically adopts both listener owners", () => {
  const listeners: DownloadListeners = [() => {}, () => {}];
  let adopted: Unlisten[] | undefined;
  expect(
    adoptDownloadListeners(
      listeners,
      () => false,
      (progress, ready) => {
        adopted = [progress, ready];
      },
    ),
  ).toBe(true);
  expect(adopted).toStrictEqual(listeners);
});
