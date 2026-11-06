import { describe, expect, test } from "vitest";
import {
  canStart,
  canStop,
  channelCount,
  describeRecordingError,
  endedEarlyText,
  estimateBytesPerHour,
  fileManagerName,
  firstUseStep,
  formatStartedAt,
  formatBytes,
  formatDuration,
  formatElapsed,
  formatLevelDb,
  formatSizePerHour,
  gapText,
  initialRecorderState,
  levelToPercent,
  noteworthyRecoveries,
  recoveryHeading,
  reduceRecorder,
  sessionNotices,
  toRecorderError,
  trashName,
  warningText,
  type RecorderAction,
  type RecorderState,
} from "./record-state";
import { backendError } from "../test/ipc";
import { finishedRecording, recoveryNote, sessionInfo } from "../test/record";
import { RECORDING_ERROR_CODES } from "./record-types";

const ready: RecorderState = reduceRecorder(initialRecorderState, {
  type: "first-use-loaded",
  folderConfirmed: true,
  consentAcknowledged: true,
});

function run(state: RecorderState, ...actions: RecorderAction[]): RecorderState {
  return actions.reduce(reduceRecorder, state);
}

describe("first use", () => {
  test("nothing starts before the settings were read", () => {
    expect(firstUseStep(initialRecorderState.firstUse)).toBe("loading");
    expect(canStart(initialRecorderState)).toBe(false);
    expect(reduceRecorder(initialRecorderState, { type: "start-requested" })).toBe(initialRecorderState);
  });

  test("the folder is confirmed first, then the consent reminder, then recording may start", () => {
    let state = reduceRecorder(initialRecorderState, { type: "first-use-loaded", folderConfirmed: false, consentAcknowledged: false });
    expect(firstUseStep(state.firstUse)).toBe("folder");
    expect(canStart(state)).toBe(false);

    state = reduceRecorder(state, { type: "consent-acknowledged" });
    expect(firstUseStep(state.firstUse)).toBe("folder");
    expect(canStart(state)).toBe(false);

    state = reduceRecorder(state, { type: "folder-confirmed" });
    expect(firstUseStep(state.firstUse)).toBe("done");
    expect(canStart(state)).toBe(true);
  });

  test("a returning user starts at once", () => {
    expect(firstUseStep(ready.firstUse)).toBe("done");
    expect(canStart(ready)).toBe(true);
  });

  test("a start request before the steps are done changes nothing", () => {
    const state = reduceRecorder(initialRecorderState, { type: "first-use-loaded", folderConfirmed: true, consentAcknowledged: false });
    expect(reduceRecorder(state, { type: "start-requested" })).toBe(state);
  });
});

describe("recording lifecycle", () => {
  test("idle -> starting -> recording -> stopping -> finished", () => {
    let state = reduceRecorder(ready, { type: "start-requested" });
    expect(state.phase).toBe("starting");
    expect(canStart(state)).toBe(false);

    state = reduceRecorder(state, { type: "start-succeeded", info: sessionInfo({ elapsed_ms: 10 }) });
    expect(state.phase).toBe("recording");
    expect(canStop(state)).toBe(true);

    state = reduceRecorder(state, { type: "session", info: sessionInfo({ elapsed_ms: 5000, level: 0.5 }) });
    expect(state.session?.elapsed_ms).toBe(5000);

    state = reduceRecorder(state, { type: "stop-requested" });
    expect(state.phase).toBe("stopping");
    expect(canStop(state)).toBe(false);

    state = reduceRecorder(state, { type: "session", info: sessionInfo({ state: "finalizing" }) });
    expect(state.phase).toBe("stopping");

    state = reduceRecorder(state, { type: "stop-succeeded", finished: finishedRecording });
    expect(state.phase).toBe("finished");
    expect(state.finished).toBe(finishedRecording);
    expect(canStart(state)).toBe(true);
  });

  test("a stale idle report does not cancel a start in flight", () => {
    const starting = reduceRecorder(ready, { type: "start-requested" });
    const after = reduceRecorder(starting, { type: "session", info: sessionInfo({ state: "idle" }) });
    expect(after.phase).toBe("starting");
  });

  test("a late recording report does not undo a stop", () => {
    const stopping = run(ready, { type: "start-requested" }, { type: "session", info: sessionInfo() }, { type: "stop-requested" });
    expect(reduceRecorder(stopping, { type: "session", info: sessionInfo({ elapsed_ms: 9000 }) }).phase).toBe("stopping");
  });

  test("the answer of a start that events overtook is ignored", () => {
    const state = run(
      ready,
      { type: "start-requested" },
      { type: "session", info: sessionInfo() },
      { type: "stop-requested" },
      { type: "start-succeeded", info: sessionInfo() },
    );
    expect(state.phase).toBe("stopping");
  });

  test("a session started elsewhere (tray) is followed", () => {
    expect(reduceRecorder(ready, { type: "session", info: sessionInfo() }).phase).toBe("recording");
  });

  test("a session stopped elsewhere returns to idle", () => {
    const recording = reduceRecorder(ready, { type: "session", info: sessionInfo() });
    expect(reduceRecorder(recording, { type: "session", info: sessionInfo({ state: "done" }) }).phase).toBe("idle");
    expect(reduceRecorder(recording, { type: "session", info: sessionInfo({ state: "idle" }) }).phase).toBe("idle");
  });

  test("a result does not open the finished view unless a stop was pending", () => {
    expect(reduceRecorder(ready, { type: "stop-succeeded", finished: finishedRecording })).toBe(ready);
  });

  test("starting a new recording clears the previous result", () => {
    const finished = run(
      ready,
      { type: "start-requested" },
      { type: "session", info: sessionInfo() },
      { type: "stop-succeeded", finished: finishedRecording },
    );
    const next = reduceRecorder(finished, { type: "start-requested" });
    expect(next.phase).toBe("starting");
    expect(next.finished).toBeNull();
  });
});

describe("errors", () => {
  test("a failed start shows its error and can be dismissed or retried", () => {
    const error = toRecorderError(backendError("recording_permission_denied", "No access.", false));
    let state = run(ready, { type: "start-requested" }, { type: "start-failed", error });
    expect(state.phase).toBe("error");
    expect(state.error).toEqual({ code: "recording_permission_denied", message: "No access.", retryable: false });
    expect(canStart(state)).toBe(true);

    state = reduceRecorder(state, { type: "dismiss" });
    expect(state).toMatchObject({ phase: "idle", error: null });
  });

  test("a failed stop shows the error and the session report moves the window on", () => {
    const state = run(
      ready,
      { type: "session", info: sessionInfo() },
      { type: "stop-requested" },
      { type: "stop-failed", error: { code: "recording_io", message: null, retryable: true } },
    );
    expect(state.phase).toBe("error");
    expect(reduceRecorder(state, { type: "session", info: sessionInfo() })).toMatchObject({ phase: "recording", error: null });
  });

  test("a failed session carries its error code", () => {
    const state = reduceRecorder(ready, { type: "session", info: sessionInfo({ state: "failed", error_code: "recording_disk_full" }) });
    expect(state.phase).toBe("error");
    expect(state.error?.code).toBe("recording_disk_full");
  });

  test("a failed session without a code gets the generic session code", () => {
    const state = reduceRecorder(ready, { type: "session", info: sessionInfo({ state: "failed" }) });
    expect(state.error?.code).toBe("recording_session_failed");
  });

  test("anything thrown becomes an error", () => {
    expect(toRecorderError("disk exploded")).toEqual({ code: "unknown", message: "disk exploded", retryable: false });
    expect(toRecorderError(undefined)).toEqual({ code: "unknown", message: null, retryable: false });
  });

  test.each(RECORDING_ERROR_CODES)("%s has its own text", (code) => {
    const text = describeRecordingError({ code, message: "backend words" });
    expect(text.text).not.toContain("backend words");
    expect(text.text.length).toBeGreaterThan(10);
    expect(text.hint).toMatch(/\S/);
  });

  test("an unknown code falls back to the backend message and the retry hint", () => {
    expect(describeRecordingError({ code: "recording_io", message: "Could not write.", retryable: true })).toEqual({
      text: "Could not write.",
      hint: "Try again.",
    });
    expect(describeRecordingError({ code: "x" })).toEqual({ text: "The recording failed.", hint: null });
  });

  test("an early end has a reason in words", () => {
    expect(endedEarlyText(null)).toBeNull();
    expect(endedEarlyText("device_changed")).toMatch(/microphone changed/);
    expect(endedEarlyText("system_sleep")).toMatch(/sleep/);
    expect(endedEarlyText("something_new")).toMatch(/ended early/);
  });
});

describe("numbers", () => {
  test("the meter maps -60 dBFS..0 dBFS to 0..100", () => {
    expect(levelToPercent(0)).toBe(0);
    expect(levelToPercent(0.0005)).toBe(0); // below -60 dB
    expect(levelToPercent(0.001)).toBe(0); // exactly -60 dB
    expect(levelToPercent(0.0316)).toBe(50); // -30 dB
    expect(levelToPercent(1)).toBe(100);
    expect(levelToPercent(1.5)).toBe(100);
  });

  test("the meter ignores invalid levels", () => {
    expect(levelToPercent(Number.NaN)).toBe(0);
    expect(levelToPercent(-0.3)).toBe(0);
    expect(levelToPercent(Number.POSITIVE_INFINITY)).toBe(0);
  });

  test("the level in dB", () => {
    expect(formatLevelDb(1)).toBe("0 dB");
    expect(formatLevelDb(0.5)).toBe("-6 dB");
    expect(formatLevelDb(0)).toBe("silent");
  });

  test("elapsed time is h:mm:ss", () => {
    expect(formatElapsed(0)).toBe("0:00:00");
    expect(formatElapsed(999)).toBe("0:00:00");
    expect(formatElapsed(5_000)).toBe("0:00:05");
    expect(formatElapsed(65_400)).toBe("0:01:05");
    expect(formatElapsed(3_661_000)).toBe("1:01:01");
    expect(formatElapsed(36_000_000)).toBe("10:00:00");
    expect(formatElapsed(-5)).toBe("0:00:00");
    expect(formatElapsed(Number.NaN)).toBe("0:00:00");
  });

  test("durations in a list drop an empty hour", () => {
    expect(formatDuration(62)).toBe("1:02");
    expect(formatDuration(3_725)).toBe("1:02:05");
  });

  test("size per hour: lossless grows with the channels, compact does not", () => {
    expect(estimateBytesPerHour("lossless")).toBe(295_000_000);
    expect(estimateBytesPerHour("lossless", 2)).toBe(590_000_000);
    expect(estimateBytesPerHour("compact")).toBe(46_000_000);
    expect(estimateBytesPerHour("compact", 2)).toBe(46_000_000);
    expect(formatSizePerHour("lossless")).toBe("about 295 MB per hour");
    expect(formatSizePerHour("compact")).toBe("about 46.0 MB per hour");
  });

  test("a channel pair [n, n] is mono", () => {
    expect(channelCount([3, 3])).toBe(1);
    expect(channelCount([1, 2])).toBe(2);
  });

  test("byte sizes use decimal units", () => {
    expect(formatBytes(0)).toBe("0 B");
    expect(formatBytes(999)).toBe("999 B");
    expect(formatBytes(1_500)).toBe("1.5 kB");
    expect(formatBytes(5_100_000)).toBe("5.1 MB");
    expect(formatBytes(1_200_000_000)).toBe("1.2 GB");
  });
});

describe("notices", () => {
  test("every folder and disk warning has text, and an unknown one a generic line", () => {
    for (const warning of ["low_disk", "fat32", "exfat", "network_volume", "cloud_synced"]) {
      expect(warningText(warning)).not.toBe(warningText("something_new"));
    }
    expect(warningText("something_new")).toBe("There is a problem with the recording folder.");
    expect(warningText("constructor")).toBe("There is a problem with the recording folder.");
  });

  test("gaps are counted with their silence", () => {
    expect(gapText(0, 0)).toBeNull();
    expect(gapText(1, 0.4)).toBe("1 gap in the audio (0.4 s of silence)");
    expect(gapText(3, 12.4)).toBe("3 gaps in the audio (12 s of silence)");
  });

  test("a running session lists what needs attention", () => {
    expect(sessionNotices(sessionInfo())).toEqual([]);
    const notices = sessionNotices(
      sessionInfo({ stalled: true, clipped: true, gap_count: 2, gap_ms: 1500, warnings: ["low_disk", "cloud_synced"] }),
    );
    expect(notices).toEqual([
      "No audio is arriving from the microphone.",
      "The input is clipping: lower the microphone gain.",
      "2 gaps in the audio (1.5 s of silence).",
      warningText("low_disk"),
      warningText("cloud_synced"),
    ]);
    expect(sessionNotices(sessionInfo({ device_lost: true, stalled: true }))[0]).toMatch(/disconnected/);
  });
});

describe("recovery and platform text", () => {
  test("the heading counts what recovery did", () => {
    expect(recoveryHeading([])).toBeNull();
    expect(recoveryHeading([recoveryNote({ outcome: "already_complete" })])).toBeNull();
    expect(recoveryHeading([recoveryNote()])).toBe("Recovered 1 recording");
    expect(recoveryHeading([recoveryNote(), recoveryNote(), recoveryNote({ outcome: "failed" })])).toBe(
      "Recovered 2 recordings, could not recover 1",
    );
    expect(recoveryHeading([recoveryNote({ outcome: "skipped_locked" })])).toBe("Skipped 1 in use");
  });

  test("only notes that did something are listed", () => {
    const done = recoveryNote({ outcome: "already_complete" });
    const kept = recoveryNote();
    expect(noteworthyRecoveries([done, kept])).toEqual([kept]);
  });

  test("the file manager and the bin are named per platform", () => {
    expect(fileManagerName("macos")).toBe("Finder");
    expect(fileManagerName("windows")).toBe("Explorer");
    expect(fileManagerName("linux")).toBe("file manager");
    expect(trashName("windows")).toBe("Recycle Bin");
    expect(trashName("macos")).toBe("Trash");
  });

  test("a start time that is no date is shown as sent", () => {
    expect(formatStartedAt("not a date")).toBe("not a date");
    expect(formatStartedAt("2026-11-06T10:00:00Z", "en-US")).toMatch(/2026/);
  });
});
