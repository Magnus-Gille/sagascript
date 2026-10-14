// The behaviour half of "polling is serialized, …" from
// scripts/test-meeting-integration.mjs (#361); its FileTranscription.svelte
// source assertions stay there until phase B.
import { expect, test } from "vitest";
import type { MeetingJobSnapshot } from "./api";
import { pollMeetingJob } from "./meeting-job-client";

test("polling is serialized, stale generations are ignored, and cancellation waits for terminal state", async () => {
  const snapshots: MeetingJobSnapshot[] = [
    { id: "job-1", status: "running", phase: "loading", error: null, transcript: null },
    { id: "job-1", status: "cancelling", phase: "cancelling", error: null, transcript: null },
    { id: "job-1", status: "cancelled", phase: "cancelled", error: null, transcript: null },
  ];
  const calls: string[] = [];
  let inFlight = 0;
  let maxInFlight = 0;
  const seen: string[] = [];
  let waits = 0;
  let busy = true;
  const fakeApi = {
    get: async () => {
      inFlight += 1;
      maxInFlight = Math.max(maxInFlight, inFlight);
      calls.push("get");
      const snapshot = snapshots.shift()!;
      inFlight -= 1;
      return snapshot;
    },
  };
  await pollMeetingJob({
    jobId: "job-1",
    get: fakeApi.get,
    isCurrent: () => true,
    onSnapshot: (snapshot) => {
      seen.push(snapshot.status);
      if (["completed", "cancelled", "failed"].includes(snapshot.status)) busy = false;
    },
    onFailure: (error) => expect.unreachable(String(error)),
    wait: async () => {
      waits += 1;
    },
  });
  expect(calls).toStrictEqual(["get", "get", "get"]);
  expect(seen).toStrictEqual(["running", "cancelling", "cancelled"]);
  expect(waits).toBe(2);
  expect(maxInFlight).toBe(1);
  expect(busy).toBe(false);

  let current = true;
  let staleSnapshots = 0;
  let staleFailure = 0;
  let release!: (snapshot: MeetingJobSnapshot) => void;
  const pending = new Promise<MeetingJobSnapshot>((resolve) => {
    release = resolve;
  });
  const stalePoll = pollMeetingJob({
    jobId: "job-stale",
    get: async () => pending,
    isCurrent: () => current,
    onSnapshot: () => {
      staleSnapshots += 1;
    },
    onFailure: () => {
      staleFailure += 1;
    },
    wait: async () => {},
  });
  current = false;
  release({ id: "job-stale", status: "completed", phase: "done", error: null, transcript: null });
  await stalePoll;
  expect(staleSnapshots).toBe(0);
  expect(staleFailure).toBe(0);

  let foreignError = "";
  let foreignSnapshots = 0;
  await pollMeetingJob({
    jobId: "job-owned",
    get: async () => ({ id: "job-foreign", status: "completed", phase: "done", error: null, transcript: null }),
    isCurrent: () => true,
    onSnapshot: () => {
      foreignSnapshots += 1;
    },
    onFailure: (error) => {
      foreignError = error instanceof Error ? error.message : String(error);
    },
    wait: async () => expect.unreachable("foreign snapshots must not wait"),
  });
  expect(foreignError).toMatch(/identity changed/);
  expect(foreignSnapshots).toBe(0);

  let retry = false;
  busy = true;
  await pollMeetingJob({
    jobId: "job-error",
    get: async () => {
      throw new Error("temporary status outage");
    },
    isCurrent: () => true,
    onSnapshot: () => expect.unreachable("failed status read must not produce a snapshot"),
    onFailure: () => {
      retry = true;
    },
    wait: async () => expect.unreachable("failed status read must not wait"),
  });
  expect(busy).toBe(true);
  expect(retry).toBe(true);

  let preservedTranscript: { source_sha256: string } = { source_sha256: "old-document" };
  let missingTranscriptError = "";
  await pollMeetingJob({
    jobId: "job-missing-transcript",
    get: async () => ({
      id: "job-missing-transcript",
      status: "completed",
      phase: "done",
      error: null,
      transcript: null,
    }),
    isCurrent: () => true,
    onSnapshot: (snapshot) => {
      if (snapshot.status === "completed" && snapshot.transcript) preservedTranscript = snapshot.transcript;
      else if (snapshot.status === "completed") missingTranscriptError = "Meeting completed without a transcript.";
    },
    onFailure: (error) => expect.unreachable(String(error)),
    wait: async () => expect.unreachable("terminal status must not wait"),
  });
  expect(preservedTranscript).toStrictEqual({ source_sha256: "old-document" });
  expect(missingTranscriptError).toBe("Meeting completed without a transcript.");
});
