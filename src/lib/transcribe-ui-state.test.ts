import { expect, test } from "vitest";
import {
  canCancelPlainTranscription,
  canRetryTranscribeFile,
  clampDecodePct,
  displayTranscribeProgress,
  isMissingTranscribeFileError,
  parseTranscribePhase,
  transcribeBaseName,
  transcribePhaseFloor,
  transcribeSaveDefaults,
} from "./transcribe-ui-state";
import { backendError } from "../test/ipc";
import commandErrors from "../../contracts/transport/command_errors.json";

test("plain cancel is only available for non-diarized runs in flight", () => {
  expect(canCancelPlainTranscription({ transcribing: true, meetingJobStatus: null, cancellingPlain: false })).toBe(true);
  expect(canCancelPlainTranscription({ transcribing: true, meetingJobStatus: "running", cancellingPlain: false })).toBe(false);
  expect(canCancelPlainTranscription({ transcribing: false, meetingJobStatus: null, cancellingPlain: false })).toBe(false);
  expect(canCancelPlainTranscription({ transcribing: true, meetingJobStatus: null, cancellingPlain: true })).toBe(false);
});

test("retry needs a previous file and an idle UI", () => {
  expect(canRetryTranscribeFile(false, false, false, "a.wav")).toBe(true);
  expect(canRetryTranscribeFile(true, false, false, "a.wav")).toBe(false);
  expect(canRetryTranscribeFile(false, true, false, "a.wav")).toBe(false);
  expect(canRetryTranscribeFile(false, false, true, "a.wav")).toBe(false);
  expect(canRetryTranscribeFile(false, false, false, null)).toBe(false);
  expect(canRetryTranscribeFile(false, false, false, "   ")).toBe(false);
});

test("missing-file errors forget the remembered file gracefully", () => {
  // The backend says so by code (DictationError::FileNotFound, ADR 0006).
  expect(
    isMissingTranscribeFileError(
      backendError("file_not_found", "File decode error: Failed to open file: No such file or directory (os error 2)"),
    ),
  ).toBe(true);
  expect(isMissingTranscribeFileError(backendError("file_not_found", "Das war nicht zu finden"))).toBe(true);
  // Other failures keep the file: the code decides, never the text.
  expect(isMissingTranscribeFileError(backendError("file_decode_error", "Failed to open file: Permission denied"))).toBe(false);
  expect(isMissingTranscribeFileError(backendError("transcription_failed", "Whisper inference failed: -6"))).toBe(false);
  expect(
    isMissingTranscribeFileError(backendError("file_decode_error", "File decode error: Failed to open file: No such file or directory")),
  ).toBe(false);
  expect(isMissingTranscribeFileError("File decode error: Failed to open file: No such file or directory")).toBe(false);
  expect(isMissingTranscribeFileError(new Error("Failed to open file: ENOENT"))).toBe(false);
  expect(isMissingTranscribeFileError(undefined)).toBe(false);
});

test("of the errors Rust sends, only file_not_found forgets the remembered file", () => {
  for (const [variant, error] of Object.entries(commandErrors)) {
    expect(isMissingTranscribeFileError(error), variant).toBe(variant === "file_not_found");
  }
});

test("displayed progress floors at 1% while running and resets when idle", () => {
  expect(displayTranscribeProgress(0, true)).toBe(1);
  expect(displayTranscribeProgress(-5, true)).toBe(1);
  expect(displayTranscribeProgress(NaN, true)).toBe(1);
  expect(displayTranscribeProgress(1, true)).toBe(1);
  expect(displayTranscribeProgress(47.8, true)).toBe(47);
  expect(displayTranscribeProgress(100, true)).toBe(100);
  expect(displayTranscribeProgress(140, true)).toBe(100);
  expect(displayTranscribeProgress(47, false)).toBe(0);
  expect(displayTranscribeProgress(0, false)).toBe(0);
});

test("prep phases tick the floor upward until real progress takes over", () => {
  expect(transcribePhaseFloor("decoding")).toBe(1);
  expect(transcribePhaseFloor("loading")).toBe(3);
  expect(transcribePhaseFloor("preparing")).toBe(5);
  expect(transcribePhaseFloor("encoding")).toBe(6);
  expect(transcribePhaseFloor(null)).toBe(1);
  expect(parseTranscribePhase("decoding")).toBe("decoding");
  expect(parseTranscribePhase("loading")).toBe("loading");
  expect(parseTranscribePhase("preparing")).toBe("preparing");
  expect(parseTranscribePhase("encoding")).toBe("encoding");
  expect(parseTranscribePhase("transcribing")).toBe(null);
  expect(parseTranscribePhase(null)).toBe(null);
  expect(parseTranscribePhase(42)).toBe(null);
  expect(displayTranscribeProgress(0, true, "loading")).toBe(3);
  expect(displayTranscribeProgress(2, true, "preparing")).toBe(5);
  expect(displayTranscribeProgress(40, true, "preparing")).toBe(40);
  expect(displayTranscribeProgress(0, false, "loading")).toBe(0);
});

test("decode payloads clamp to 0-100 or vanish when unusable", () => {
  expect(clampDecodePct(0)).toBe(0);
  expect(clampDecodePct(42.7)).toBe(42);
  expect(clampDecodePct(100)).toBe(100);
  expect(clampDecodePct(140)).toBe(100);
  expect(clampDecodePct(-3)).toBe(0);
  expect(clampDecodePct(NaN)).toBe(null);
  expect(clampDecodePct("42")).toBe(null);
  expect(clampDecodePct(null)).toBe(null);
  expect(clampDecodePct(undefined)).toBe(null);
});

test("save defaults point at the audio folder with a .txt basename", () => {
  expect(transcribeSaveDefaults("/Users/x/audio/talk.m4a")).toStrictEqual({
    fileName: "talk.txt",
    directory: "/Users/x/audio",
  });
  expect(transcribeSaveDefaults("C:\\audio\\talk")).toStrictEqual({
    fileName: "talk.txt",
    directory: "C:\\audio",
  });
  expect(transcribeSaveDefaults("talk.wav")).toStrictEqual({
    fileName: "talk.txt",
    directory: null,
  });
  expect(transcribeSaveDefaults(null)).toStrictEqual({
    fileName: "transcription.txt",
    directory: null,
  });
  expect(transcribeSaveDefaults("   ")).toStrictEqual({
    fileName: "transcription.txt",
    directory: null,
  });
});

test("re-run button shows the bare file name, never the full path", () => {
  expect(transcribeBaseName("/Users/x/audio/talk.m4a")).toBe("talk.m4a");
  expect(transcribeBaseName("C:\\audio\\talk.m4a")).toBe("talk.m4a");
  expect(transcribeBaseName("talk.wav")).toBe("talk.wav");
  expect(transcribeBaseName(null)).toBe("");
  expect(transcribeBaseName("")).toBe("");
});
