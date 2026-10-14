import { expect, test } from "vitest";
import { createFileJobs, fileJobName, nextQueuedFile, updateFileJob, type FileJobOptions } from "./transcription-queue";

const options: FileJobOptions = { diarize: true, prompt: "Use names", profileId: "profile-a" };

test("a single file becomes one queued job with captured options", () => {
  const jobs = createFileJobs(["/recordings/one.wav"], options, () => "job-1");

  expect(jobs).toStrictEqual([
    {
      id: "job-1",
      path: "/recordings/one.wav",
      diarize: true,
      prompt: "Use names",
      profileId: "profile-a",
      status: "queued",
    },
  ]);
});

test("three files are selected in order as each prior job reaches a terminal state", () => {
  const jobs = createFileJobs(
    ["one.wav", "two.wav", "three.wav"],
    options,
    (() => {
      let next = 0;
      return () => `job-${++next}`;
    })(),
  );

  expect(nextQueuedFile(jobs, false)?.id).toBe("job-1");
  const running = updateFileJob(jobs, "job-1", "running");
  expect(nextQueuedFile(running, false)).toBe(null);
  const completed = updateFileJob(running, "job-1", "completed");
  expect(nextQueuedFile(completed, false)?.id).toBe("job-2");
  const secondRunning = updateFileJob(completed, "job-2", "running");
  const secondCompleted = updateFileJob(secondRunning, "job-2", "completed");
  expect(nextQueuedFile(secondCompleted, false)?.id).toBe("job-3");
});

test("a newly appended file waits behind a running job", () => {
  const initial = createFileJobs(
    ["one.wav", "two.wav"],
    options,
    (() => {
      let next = 0;
      return () => `job-${++next}`;
    })(),
  );
  const running = updateFileJob(initial, "job-1", "running");
  const appended = [...running, ...createFileJobs(["three.wav"], options, () => "job-3")];

  expect(nextQueuedFile(appended, false)).toBe(null);
  expect(nextQueuedFile(appended, true)).toBe(null);
  const afterCompletion = updateFileJob(appended, "job-1", "completed");
  expect(nextQueuedFile(afterCompletion, false)?.id).toBe("job-2");
});

test("a failed file is terminal and the next queued file can run", () => {
  const jobs = createFileJobs(
    ["one.wav", "two.wav"],
    options,
    (() => {
      let next = 0;
      return () => `job-${++next}`;
    })(),
  );
  const running = updateFileJob(jobs, "job-1", "running");
  const failed = updateFileJob(running, "job-1", "failed");

  expect(nextQueuedFile(failed, false)?.id).toBe("job-2");
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

  expect(jobs.map((job) => job.id)).toStrictEqual(["job-1", "job-2", "job-3"]);
  expect(jobs.map((job) => job.path)).toStrictEqual(["/a/shared.wav", "C:\\b\\shared.wav", "/a/shared.wav"]);
  expect(jobs[0]).not.toBe(jobs[2]);
  expect(fileJobName(jobs[0].path)).toBe("shared.wav");
  expect(fileJobName(jobs[1].path)).toBe("shared.wav");
});

test("job options are copied into each job and source changes do not leak", () => {
  const captured: FileJobOptions = { diarize: false, prompt: null, profileId: null };
  const jobs = createFileJobs(
    ["one.wav", "two.wav"],
    captured,
    (() => {
      let next = 0;
      return () => `job-${++next}`;
    })(),
  );

  captured.diarize = true;
  captured.prompt = "changed";
  captured.profileId = "changed-profile";

  expect(jobs[0].diarize).toBe(false);
  expect(jobs[0].prompt).toBe(null);
  expect(jobs[0].profileId).toBe(null);
  expect(jobs[1].diarize).toBe(false);
  expect(jobs[1].prompt).toBe(null);
  expect(jobs[1].profileId).toBe(null);
});

test("empty paths are omitted while nonempty paths preserve order", () => {
  const jobs = createFileJobs(
    ["", "one.wav", "", "two.wav"],
    options,
    (() => {
      let next = 0;
      return () => `job-${++next}`;
    })(),
  );

  expect(jobs.map((job) => job.path)).toStrictEqual(["one.wav", "two.wav"]);
});

test("nextQueuedFile respects busy state and running jobs", () => {
  const jobs = createFileJobs(
    ["one.wav", "two.wav"],
    options,
    (() => {
      let next = 0;
      return () => `job-${++next}`;
    })(),
  );

  expect(nextQueuedFile(jobs, true)).toBe(null);
  expect(nextQueuedFile(updateFileJob(jobs, "job-1", "running"), false)).toBe(null);
});

test("updateFileJob returns a new array and retains unrelated job objects", () => {
  const jobs = createFileJobs(
    ["one.wav", "two.wav"],
    options,
    (() => {
      let next = 0;
      return () => `job-${++next}`;
    })(),
  );
  const updated = updateFileJob(jobs, "job-1", "cancelled");

  expect(updated).not.toBe(jobs);
  expect(updated[0]).not.toBe(jobs[0]);
  expect(updated[0].status).toBe("cancelled");
  expect(updated[1]).toBe(jobs[1]);
  expect(jobs.map((job) => job.status)).toStrictEqual(["queued", "queued"]);
});
