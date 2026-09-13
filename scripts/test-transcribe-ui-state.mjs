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
  MAX_RECENT_TRANSCRIBE_FILES,
  canCancelPlainTranscription,
  displayTranscribeProgress,
  pushRecentTranscribeFile,
  stageRecentTranscribeFile,
  canRetryTranscribeFile,
  isMissingTranscribeFileError,
  pruneMissingTranscribeFile,
  transcribeSaveDefaults,
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

test("recent list is session-only paths: deduped, most-recent-first, capped", () => {
  assert.equal(MAX_RECENT_TRANSCRIBE_FILES, 5);
  let list = [];
  for (const file of ["a.wav", "b.wav", "c.wav", "d.wav", "e.wav", "f.wav"]) {
    list = pushRecentTranscribeFile(list, file);
  }
  assert.deepEqual(list, ["f.wav", "e.wav", "d.wav", "c.wav", "b.wav"]);
  list = pushRecentTranscribeFile(list, "d.wav");
  assert.deepEqual(list, ["d.wav", "f.wav", "e.wav", "c.wav", "b.wav"]);
  assert.deepEqual(pushRecentTranscribeFile(list, "   "), list);
  // No mutation of the input array.
  const input = ["a.wav"];
  pushRecentTranscribeFile(input, "b.wav");
  assert.deepEqual(input, ["a.wav"]);
});

test("staging a recent entry never crashes on missing files", () => {
  assert.equal(stageRecentTranscribeFile(["a.wav"], "a.wav"), "a.wav");
  assert.equal(stageRecentTranscribeFile(["a.wav"], "moved.wav"), null);
});

test("retry needs a previous file and an idle UI", () => {
  assert.equal(canRetryTranscribeFile(false, false, false, "a.wav"), true);
  assert.equal(canRetryTranscribeFile(true, false, false, "a.wav"), false);
  assert.equal(canRetryTranscribeFile(false, true, false, "a.wav"), false);
  assert.equal(canRetryTranscribeFile(false, false, true, "a.wav"), false);
  assert.equal(canRetryTranscribeFile(false, false, false, null), false);
  assert.equal(canRetryTranscribeFile(false, false, false, "   "), false);
});

test("missing-file errors prune the recent list gracefully", () => {
  assert.equal(isMissingTranscribeFileError("No such file or directory"), true);
  assert.equal(isMissingTranscribeFileError(new Error("ENOENT: open failed")), true);
  assert.equal(isMissingTranscribeFileError("Whisper inference failed: -6"), false);
  assert.deepEqual(pruneMissingTranscribeFile(["a.wav", "b.wav"], "a.wav"), ["b.wav"]);
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
