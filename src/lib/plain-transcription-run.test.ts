import { expect, test, vi } from "vitest";
import type { PlainTranscriptionProgress } from "./events";
import { acceptRunProgress, startStages } from "./transcribe-stages";
import { createPlainTranscriptionRun, type PlainTranscriptionRunPorts } from "./plain-transcription-run";
import type { Subscription } from "./tauri-events.svelte";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => {
    resolve = yes;
    reject = no;
  });
  return { promise, resolve, reject };
}

function fixture(overrides: Partial<PlainTranscriptionRunPorts> = {}) {
  let progressHandler: ((payload: PlainTranscriptionProgress) => void) | undefined;
  const stages = { current: startStages() };
  const events: string[] = [];
  const subscription: Subscription = {
    ready: Promise.resolve(),
    dispose: vi.fn(() => events.push("disposed")),
  };
  const ports: PlainTranscriptionRunPorts = {
    waitForMeetingActions: vi.fn(async () => {
      events.push("actions-ready");
    }),
    subscribeProgress: vi.fn((handler) => {
      events.push("subscribed");
      progressHandler = handler;
      return subscription;
    }),
    transcribeFile: vi.fn(async () => {
      events.push("submitted");
      return "recognized text";
    }),
    cancelFileTranscription: vi.fn(async () => true),
    onStart: vi.fn(() => events.push("started")),
    onProgress: vi.fn((payload) => {
      stages.current = acceptRunProgress(stages.current, "run-1", payload);
    }),
    onCancelledBeforeSubmission: vi.fn(() => events.push("cancelled-before-submit")),
    onResult: vi.fn(() => events.push("result")),
    onError: vi.fn(() => events.push("error")),
    onFinished: vi.fn(() => events.push("finished")),
    onCancelRequested: vi.fn(() => events.push("cancel-requested")),
    onCancelError: vi.fn(() => events.push("cancel-error")),
    ...overrides,
  };
  return {
    controller: createPlainTranscriptionRun(ports),
    ports,
    events,
    stages,
    emitProgress: (payload: PlainTranscriptionProgress) => progressHandler?.(payload),
  };
}

const request = { path: "/audio/interview.wav", prompt: "Astrid", profileId: "swedish", runId: "run-1" };

test("one instance guards duplicate starts and registers typed progress before native submission", async () => {
  const native = deferred<string>();
  const submitted = deferred<void>();
  const h = fixture();
  h.ports.transcribeFile = vi.fn(async () => {
    h.events.push("submitted");
    submitted.resolve();
    return native.promise;
  });
  const first = h.controller.start(request);
  const second = h.controller.start(request);
  await submitted.promise;
  expect(h.ports.onStart).toHaveBeenCalledTimes(1);
  expect(h.ports.transcribeFile).toHaveBeenCalledTimes(1);
  expect(h.events.indexOf("subscribed")).toBeLessThan(h.events.indexOf("submitted"));

  h.emitProgress({ runId: "stale-run", phase: "transcribing", percent: 90 });
  expect(h.stages.current.index).toBe(0);
  expect(h.ports.onProgress).not.toHaveBeenCalled();
  h.emitProgress({ runId: request.runId, phase: "resampling", percent: 60 });
  expect(h.ports.onProgress).toHaveBeenCalledTimes(1);
  expect(h.ports.onProgress).toHaveBeenCalledWith({ runId: request.runId, phase: "resampling", percent: 60 });
  expect(h.stages.current.index).toBe(1);
  expect(h.stages.current.percent).toBe(60);

  native.resolve("recognized text");
  await Promise.all([first, second]);
  expect(h.ports.onResult).toHaveBeenCalledWith("recognized text", false);
  expect(h.events.slice(-2)).toEqual(["disposed", "finished"]);
});

test("Stop while queued stays local, then disposes the listener and finishes as cancelled", async () => {
  const gate = deferred<void>();
  const h = fixture({ waitForMeetingActions: vi.fn(() => gate.promise) });
  const run = h.controller.start(request);
  expect(h.ports.onStart).toHaveBeenCalledOnce();
  await h.controller.cancel();
  expect(h.ports.onCancelRequested).toHaveBeenCalledOnce();
  expect(h.ports.cancelFileTranscription).not.toHaveBeenCalled();
  expect(h.ports.transcribeFile).not.toHaveBeenCalled();

  gate.resolve();
  await run;
  expect(h.ports.subscribeProgress).toHaveBeenCalledOnce();
  expect(h.ports.onCancelledBeforeSubmission).toHaveBeenCalledOnce();
  expect(h.ports.transcribeFile).not.toHaveBeenCalled();
  expect(h.ports.cancelFileTranscription).not.toHaveBeenCalled();
  expect(h.events.slice(-3)).toEqual(["cancelled-before-submit", "disposed", "finished"]);
});

test("transcription errors report the path captured by that run", async () => {
  const failure = new Error("file could not be read");
  const h = fixture({
    transcribeFile: vi.fn(async () => {
      throw failure;
    }),
  });
  const capturedRequest = { ...request, path: "/audio/original.wav" };

  await h.controller.start(capturedRequest);

  expect(h.ports.onError).toHaveBeenCalledWith(failure, false, capturedRequest.path);
  expect(h.ports.onFinished).toHaveBeenCalledOnce();
});

test("a late successful result after cancellation remains successful and reports the race", async () => {
  const native = deferred<string>();
  const submitted = deferred<void>();
  const cancelDispatch = deferred<boolean>();
  const h = fixture({
    transcribeFile: vi.fn(() => {
      submitted.resolve();
      return native.promise;
    }),
    cancelFileTranscription: vi.fn(() => cancelDispatch.promise),
  });
  const run = h.controller.start(request);
  await submitted.promise;
  const cancel = h.controller.cancel();
  await h.controller.cancel();
  expect(h.ports.cancelFileTranscription).toHaveBeenCalledWith(request.runId);
  expect(h.ports.cancelFileTranscription).toHaveBeenCalledTimes(1);
  cancelDispatch.resolve(true);
  await cancel;
  native.resolve("finished first");
  await run;
  expect(h.ports.onResult).toHaveBeenCalledWith("finished first", true);
  expect(h.ports.onError).not.toHaveBeenCalled();
});

test("late cancellation rejection after a new run starts cannot change the new run", async () => {
  const firstNative = deferred<string>();
  const secondNative = deferred<string>();
  const firstSubmitted = deferred<void>();
  const secondSubmitted = deferred<void>();
  const cancelDispatch = deferred<boolean>();
  const h = fixture({
    transcribeFile: vi.fn((_path, options) => {
      if (options.runId === request.runId) {
        firstSubmitted.resolve();
        return firstNative.promise;
      }
      secondSubmitted.resolve();
      return secondNative.promise;
    }),
    cancelFileTranscription: vi.fn(() => cancelDispatch.promise),
  });

  const firstRun = h.controller.start(request);
  await firstSubmitted.promise;
  const cancellation = h.controller.cancel();
  firstNative.resolve("first result");
  await firstRun;

  const nextRequest = { ...request, path: "/audio/next.wav", runId: "run-2" };
  const secondRun = h.controller.start(nextRequest);
  await secondSubmitted.promise;
  cancelDispatch.reject(new Error("late cancellation failure"));
  await cancellation;

  expect(h.ports.cancelFileTranscription).toHaveBeenCalledTimes(1);
  expect(h.ports.onCancelError).not.toHaveBeenCalled();
  expect(h.ports.onError).not.toHaveBeenCalled();
  secondNative.resolve("second result");
  await secondRun;
  expect(h.ports.onResult).toHaveBeenLastCalledWith("second result", false);
  expect(h.ports.onFinished).toHaveBeenCalledTimes(2);
});
