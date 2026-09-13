import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import ts from "typescript";

const source = await readFile(
  new URL("../src/lib/transcribe-ui-state.ts", import.meta.url),
  "utf8",
);
const module = ts.transpileModule(source, {
  compilerOptions: {
    module: ts.ModuleKind.ESNext,
    target: ts.ScriptTarget.ES2022,
  },
}).outputText;
const {
  canCancelPlainTranscription,
  displayTranscribeProgress,
  transcribePhaseFloor,
  parseTranscribePhase,
  canRetryTranscribeFile,
  isMissingTranscribeFileError,
  transcribeSaveDefaults,
  transcribeBaseName,
} = await import(
  `data:text/javascript;base64,${Buffer.from(module).toString("base64")}`
);

test("plain cancel is only available for non-diarized runs in flight", () => {
  assert.equal(
    canCancelPlainTranscription({ transcribing: true, meetingJobStatus: null, cancellingPlain: false }),
    true,
  );
  assert.equal(
    canCancelPlainTranscription({ transcribing: true, meetingJobStatus: "running", cancellingPlain: false }),
    false,
  );
  assert.equal(
    canCancelPlainTranscription({ transcribing: false, meetingJobStatus: null, cancellingPlain: false }),
    false,
  );
  assert.equal(
    canCancelPlainTranscription({ transcribing: true, meetingJobStatus: null, cancellingPlain: true }),
    false,
  );
});

test("retry needs a previous file and an idle UI", () => {
  assert.equal(canRetryTranscribeFile(false, false, false, "a.wav"), true);
  assert.equal(canRetryTranscribeFile(true, false, false, "a.wav"), false);
  assert.equal(canRetryTranscribeFile(false, true, false, "a.wav"), false);
  assert.equal(canRetryTranscribeFile(false, false, true, "a.wav"), false);
  assert.equal(canRetryTranscribeFile(false, false, false, null), false);
  assert.equal(canRetryTranscribeFile(false, false, false, "   "), false);
});

test("missing-file errors forget the remembered file gracefully", () => {
  assert.equal(isMissingTranscribeFileError("No such file or directory"), true);
  assert.equal(isMissingTranscribeFileError(new Error("ENOENT: open failed")), true);
  assert.equal(isMissingTranscribeFileError("Whisper inference failed: -6"), false);
});

test("displayed progress floors at 1% while running and resets when idle", () => {
  assert.equal(displayTranscribeProgress(0, true), 1);
  assert.equal(displayTranscribeProgress(-5, true), 1);
  assert.equal(displayTranscribeProgress(NaN, true), 1);
  assert.equal(displayTranscribeProgress(1, true), 1);
  assert.equal(displayTranscribeProgress(47.8, true), 47);
  assert.equal(displayTranscribeProgress(100, true), 100);
  assert.equal(displayTranscribeProgress(140, true), 100);
  assert.equal(displayTranscribeProgress(47, false), 0);
  assert.equal(displayTranscribeProgress(0, false), 0);
});

test("prep phases tick the floor upward until real progress takes over", () => {
  assert.equal(transcribePhaseFloor("decoding"), 1);
  assert.equal(transcribePhaseFloor("loading"), 3);
  assert.equal(transcribePhaseFloor("preparing"), 5);
  assert.equal(transcribePhaseFloor(null), 1);
  assert.equal(parseTranscribePhase("decoding"), "decoding");
  assert.equal(parseTranscribePhase("loading"), "loading");
  assert.equal(parseTranscribePhase("preparing"), "preparing");
  assert.equal(parseTranscribePhase("transcribing"), null);
  assert.equal(parseTranscribePhase(null), null);
  assert.equal(parseTranscribePhase(42), null);
  assert.equal(displayTranscribeProgress(0, true, "loading"), 3);
  assert.equal(displayTranscribeProgress(2, true, "preparing"), 5);
  assert.equal(displayTranscribeProgress(40, true, "preparing"), 40);
  assert.equal(displayTranscribeProgress(0, false, "loading"), 0);
});

test("save defaults point at the audio folder with a .txt basename", () => {
  assert.deepEqual(transcribeSaveDefaults("/Users/x/audio/talk.m4a"), {
    fileName: "talk.txt",
    directory: "/Users/x/audio",
  });
  assert.deepEqual(transcribeSaveDefaults("C:\\audio\\talk"), {
    fileName: "talk.txt",
    directory: "C:\\audio",
  });
  assert.deepEqual(transcribeSaveDefaults("talk.wav"), {
    fileName: "talk.txt",
    directory: null,
  });
  assert.deepEqual(transcribeSaveDefaults(null), {
    fileName: "transcription.txt",
    directory: null,
  });
  assert.deepEqual(transcribeSaveDefaults("   "), {
    fileName: "transcription.txt",
    directory: null,
  });
});

test("re-run button shows the bare file name, never the full path", () => {
  assert.equal(transcribeBaseName("/Users/x/audio/talk.m4a"), "talk.m4a");
  assert.equal(transcribeBaseName("C:\\audio\\talk.m4a"), "talk.m4a");
  assert.equal(transcribeBaseName("talk.wav"), "talk.wav");
  assert.equal(transcribeBaseName(null), "");
  assert.equal(transcribeBaseName(""), "");
});
