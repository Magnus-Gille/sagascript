import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import ts from "typescript";

const source = await readFile(
  new URL("../src/lib/transcription-queue.ts", import.meta.url),
  "utf8",
);
const moduleSource = ts.transpileModule(source, {
  compilerOptions: {
    module: ts.ModuleKind.ESNext,
    target: ts.ScriptTarget.ES2022,
  },
}).outputText;
const {
  createFileJobs,
  fileJobName,
  nextQueuedFile,
  updateFileJob,
} = await import(`data:text/javascript;base64,${Buffer.from(moduleSource).toString("base64")}`);

const options = { diarize: true, prompt: "Use names", profileId: "profile-a" };

test("a single file becomes one queued job with captured options", () => {
  const jobs = createFileJobs(["/recordings/one.wav"], options, () => "job-1");

  assert.deepEqual(jobs, [{
    id: "job-1",
    path: "/recordings/one.wav",
    diarize: true,
    prompt: "Use names",
    profileId: "profile-a",
    status: "queued",
  }]);
});

test("three files are selected in order as each prior job reaches a terminal state", () => {
  const jobs = createFileJobs(["one.wav", "two.wav", "three.wav"], options, (() => {
    let next = 0;
    return () => `job-${++next}`;
  })());

  assert.equal(nextQueuedFile(jobs, false)?.id, "job-1");
  const running = updateFileJob(jobs, "job-1", "running");
  assert.equal(nextQueuedFile(running, false), null);
  const completed = updateFileJob(running, "job-1", "completed");
  assert.equal(nextQueuedFile(completed, false)?.id, "job-2");
  const secondRunning = updateFileJob(completed, "job-2", "running");
  const secondCompleted = updateFileJob(secondRunning, "job-2", "completed");
  assert.equal(nextQueuedFile(secondCompleted, false)?.id, "job-3");
});

test("a newly appended file waits behind a running job", () => {
  const initial = createFileJobs(["one.wav", "two.wav"], options, (() => {
    let next = 0;
    return () => `job-${++next}`;
  })());
  const running = updateFileJob(initial, "job-1", "running");
  const appended = [
    ...running,
    ...createFileJobs(["three.wav"], options, () => "job-3"),
  ];

  assert.equal(nextQueuedFile(appended, false), null);
  assert.equal(nextQueuedFile(appended, true), null);
  const afterCompletion = updateFileJob(appended, "job-1", "completed");
  assert.equal(nextQueuedFile(afterCompletion, false)?.id, "job-2");
});

test("a failed file is terminal and the next queued file can run", () => {
  const jobs = createFileJobs(["one.wav", "two.wav"], options, (() => {
    let next = 0;
    return () => `job-${++next}`;
  })());
  const running = updateFileJob(jobs, "job-1", "running");
  const failed = updateFileJob(running, "job-1", "failed");

  assert.equal(nextQueuedFile(failed, false)?.id, "job-2");
});

test("duplicate paths remain independent jobs, including duplicate basenames", () => {
  const jobs = createFileJobs(
    ["/a/shared.wav", "C:\\b\\shared.wav", "/a/shared.wav"],
    options,
    (() => {
      let next = 0;
      return () => `job-${++next}`;
    })(),
  );

  assert.deepEqual(jobs.map((job) => job.id), ["job-1", "job-2", "job-3"]);
  assert.deepEqual(jobs.map((job) => job.path), ["/a/shared.wav", "C:\\b\\shared.wav", "/a/shared.wav"]);
  assert.notEqual(jobs[0], jobs[2]);
  assert.equal(fileJobName(jobs[0].path), "shared.wav");
  assert.equal(fileJobName(jobs[1].path), "shared.wav");
});

test("job options are copied into each job and source changes do not leak", () => {
  const captured = { diarize: false, prompt: null, profileId: null };
  const jobs = createFileJobs(["one.wav", "two.wav"], captured, (() => {
    let next = 0;
    return () => `job-${++next}`;
  })());

  captured.diarize = true;
  captured.prompt = "changed";
  captured.profileId = "changed-profile";

  assert.equal(jobs[0].diarize, false);
  assert.equal(jobs[0].prompt, null);
  assert.equal(jobs[0].profileId, null);
  assert.equal(jobs[1].diarize, false);
  assert.equal(jobs[1].prompt, null);
  assert.equal(jobs[1].profileId, null);
});

test("empty paths are omitted while nonempty paths preserve order", () => {
  const jobs = createFileJobs(["", "one.wav", "", "two.wav"], options, (() => {
    let next = 0;
    return () => `job-${++next}`;
  })());

  assert.deepEqual(jobs.map((job) => job.path), ["one.wav", "two.wav"]);
});

test("nextQueuedFile respects busy state and running jobs", () => {
  const jobs = createFileJobs(["one.wav", "two.wav"], options, (() => {
    let next = 0;
    return () => `job-${++next}`;
  })());

  assert.equal(nextQueuedFile(jobs, true), null);
  assert.equal(nextQueuedFile(updateFileJob(jobs, "job-1", "running"), false), null);
});

test("updateFileJob returns a new array and retains unrelated job objects", () => {
  const jobs = createFileJobs(["one.wav", "two.wav"], options, (() => {
    let next = 0;
    return () => `job-${++next}`;
  })());
  const updated = updateFileJob(jobs, "job-1", "cancelled");

  assert.notEqual(updated, jobs);
  assert.notEqual(updated[0], jobs[0]);
  assert.equal(updated[0].status, "cancelled");
  assert.equal(updated[1], jobs[1]);
  assert.deepEqual(jobs.map((job) => job.status), ["queued", "queued"]);
});
