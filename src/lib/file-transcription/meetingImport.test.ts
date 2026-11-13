// The meeting import on its own, without the component that renders it: the
// host's props are read live and the IPC calls are the ones the component makes.
import { expect, test, vi } from "vitest";
import fixtures from "../../../contracts/transport/get_meeting_job.json";
import { backendError, mockIpc } from "../../test/ipc";
import type { MeetingJobSnapshot } from "../api";
import type { MeetingReviewState } from "../meeting-types";
import type { FileJob } from "../transcription-queue";
import type { UpdateRecoveryMeeting } from "../update-recovery";
import { MeetingImport, type MeetingImportHost } from "./meetingImport.svelte";

const review = fixtures.reprocessing.reprocessing.proposal.previous as MeetingReviewState["review"];
const reviewState: MeetingReviewState = { review, transcript: review.original };
const jobId = "meeting-job";
const completedSnapshot: MeetingJobSnapshot = {
  ...(fixtures.completed as unknown as MeetingJobSnapshot),
  id: jobId,
  transcript: review.original,
};

function fakeHost(job: FileJob) {
  const onMeetingRecoveryChange = vi.fn<(entry: UpdateRecoveryMeeting | null) => void>();
  // A fake whose props the test changes, as a parent would.
  const host: MeetingImportHost & { job: FileJob; initialRecoveryMeeting: UpdateRecoveryMeeting | null } = {
    job,
    otherBusy: false,
    initialRecoveryMeeting: null,
    onMeetingRecoveryChange,
    transcribing: false,
    started: false,
    transcribeError: "earlier error",
    transcribeRecovery: "retry",
    transcriptionResult: "earlier text",
  };
  return { host, onMeetingRecoveryChange };
}

const meetingJob: FileJob = {
  id: "job-1",
  path: "/recordings/meeting.wav",
  diarize: true,
  prompt: "Names",
  profileId: "interview",
  status: "running",
};

test("an import starts the meeting job, polls it to completion and opens its review", async () => {
  const ipc = mockIpc({
    begin_meeting_file: () => jobId,
    get_meeting_job: () => completedSnapshot,
    create_meeting_review: () => reviewState,
  });
  const { host } = fakeHost(meetingJob);
  const meeting = new MeetingImport(host);

  await meeting.startMeetingFileTranscription(meetingJob.path, meetingJob.prompt, meetingJob.profileId);
  await vi.waitFor(() => expect(meeting.meetingReview).not.toBeNull());
  await meeting.meetingReviewInit;

  expect(ipc.calls.map((call) => call.command)).toEqual(["begin_meeting_file", "get_meeting_job", "create_meeting_review"]);
  expect(ipc.callsTo("begin_meeting_file")).toEqual([{ filePath: "/recordings/meeting.wav", prompt: "Names", profileId: "interview" }]);
  expect(meeting.meetingReview).toEqual(review);
  expect(meeting.meetingTranscript).toEqual(review.original);
  expect(meeting.meetingJobStatus).toBe("completed");
  expect(meeting.meetingError).toBe("");
  expect(host.transcribing).toBe(false);
  expect(host.transcribeError).toBe("");
  expect(host.transcribeRecovery).toBeNull();
  expect(host.transcriptionResult).toBe("");
  meeting.destroy();
});

test("a failed start reports the error and leaves nothing running", async () => {
  mockIpc({
    begin_meeting_file: () => {
      throw backendError("busy", "Finish the current dictation before importing a meeting.", true);
    },
  });
  const { host } = fakeHost(meetingJob);
  const meeting = new MeetingImport(host);

  await meeting.startMeetingFileTranscription(meetingJob.path, null, null);

  expect(meeting.meetingError).toBe("Finish the current dictation before importing a meeting.");
  expect(meeting.meetingJobStatus).toBe("failed");
  expect(meeting.meetingPhase).toBe("Failed");
  expect(host.transcribing).toBe(false);
});

test("a stored recovery for this job is restored once and reported with the current job", () => {
  const ipc = mockIpc({});
  const { host, onMeetingRecoveryChange } = fakeHost({ ...meetingJob, status: "queued" });
  host.initialRecoveryMeeting = {
    job_id: "job-1",
    path: "/recordings/meeting.wav",
    review: reviewState,
    editor_draft: { labels: {}, mergeTargets: {}, texts: {}, speakers: {} },
    proposal: null,
  };
  const meeting = new MeetingImport(host);

  meeting.hydrateRecovery();
  const restored = meeting.meetingReview;
  meeting.hydrateRecovery();

  expect(host.started).toBe(true);
  expect(meeting.meetingReview).toBe(restored);
  expect(meeting.meetingReview).toEqual(review);
  expect(meeting.meetingJobStatus).toBe("completed");

  // The job prop is read when it is needed, not when the import was created.
  host.job = { ...host.job, path: "/recordings/renamed.wav" };
  meeting.reportRecovery();
  expect(onMeetingRecoveryChange).toHaveBeenLastCalledWith(
    expect.objectContaining({
      job_id: "job-1",
      path: "/recordings/renamed.wav",
      proposal: null,
    }),
  );
  expect(ipc.calls).toEqual([]);
});

test("a recovery for another job is ignored", () => {
  mockIpc({});
  const { host } = fakeHost(meetingJob);
  host.initialRecoveryMeeting = {
    job_id: "other-job",
    path: "/recordings/other.wav",
    review: reviewState,
    editor_draft: { labels: {}, mergeTargets: {}, texts: {}, speakers: {} },
    proposal: null,
  };
  const meeting = new MeetingImport(host);

  meeting.hydrateRecovery();

  expect(host.started).toBe(false);
  expect(meeting.meetingReview).toBeNull();
});
