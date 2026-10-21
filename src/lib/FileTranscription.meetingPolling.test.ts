import { mockWindows } from "@tauri-apps/api/mocks";
import { render, screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import type { ComponentProps } from "svelte";
import { expect, test, vi } from "vitest";
import meetingJobFixtures from "../../contracts/transport/get_meeting_job.json";
import { deferred } from "../test/deferred";
import { backendError, mockIpc, type CommandHandlers } from "../test/ipc";
import FileTranscription from "./FileTranscription.svelte";
import type { MeetingJobSnapshot } from "./api";
import type { MeetingReview, MeetingReviewState } from "./meeting-types";
import type { ReprocessingResult } from "./meeting-reprocessing-types";
import type { FileJob } from "./transcription-queue";

const jobId = "meeting-poll-job";
const completedSnapshot = {
  ...(meetingJobFixtures.completed as unknown as MeetingJobSnapshot),
  id: jobId,
};
const runningSnapshot = {
  ...(meetingJobFixtures.running_preparing as unknown as MeetingJobSnapshot),
  id: jobId,
};
const cancelledSnapshot = {
  ...(meetingJobFixtures.cancelled as unknown as MeetingJobSnapshot),
  id: jobId,
};
const reprocessingFixture = meetingJobFixtures.reprocessing as unknown as MeetingJobSnapshot & {
  reprocessing: ReprocessingResult;
};
const fixtureReview = reprocessingFixture.reprocessing.proposal.previous;
const completedTranscript = completedSnapshot.transcript;
if (!completedTranscript) throw new Error("The completed meeting transport fixture needs a transcript.");

// Rust `MeetingReview::new` initializes generation 0 with no batches. Keep
// this payload in RevisionPayload's source field order: serde_json hashes its
// compact serialized bytes. The transport's reprocessing review shares the
// exact original transcript, so its original hash binds this initial review.
// SHA-256 of Rust's compact, source-ordered RevisionPayload JSON:
// {"schema_version":1,"original_revision":"7201d4801197c586ca6dafaa51ca6d40509b4646142653870f937ec9a7c38253","generation":0,"batches":[]}
const completedOriginalRevision = "7201d4801197c586ca6dafaa51ca6d40509b4646142653870f937ec9a7c38253";
const completedReviewRevision = "668400f77910581ef62e7e03f6dc83de085e2e1d8f0a7b4186856be189c50b35";
const completedReview: MeetingReview = {
  schema_version: 1,
  original: completedTranscript,
  original_revision: completedOriginalRevision,
  generation: 0,
  batches: [],
  revision: completedReviewRevision,
};
const completedReviewState: MeetingReviewState = {
  review: completedReview,
  transcript: completedTranscript,
};

function componentProps(): ComponentProps<typeof FileTranscription> {
  const job: FileJob = {
    id: "meeting-file",
    path: "/recordings/meeting.wav",
    diarize: true,
    prompt: "Use the recorded speaker names",
    profileId: "interview-profile",
    status: "running",
  };
  return {
    job,
    otherBusy: false,
    active: true,
    initialRecoveryMeeting: null,
    onComplete: vi.fn(),
    onBusyChange: () => undefined,
    onMissingFile: vi.fn(),
    onRetry: () => true,
    retryRefusal: "",
    onDownloadModel: async () => undefined,
    downloadBusy: false,
    updatePreparing: false,
    onAttentionChange: vi.fn(),
  };
}

function renderRunningMeeting(handlers: CommandHandlers) {
  const ipc = mockIpc(handlers);
  const onBusyChange = vi.fn((id: string, busy: boolean) => [id, busy] as const);
  const props = componentProps();
  props.onBusyChange = onBusyChange;
  mockWindows("main");
  const view = render(FileTranscription, { props });
  return { ipc, props, view, onBusyChange };
}

test("a failed meeting status check retries the same job and consumes its completed transport fixture", async () => {
  let pollCount = 0;
  const { ipc, props, onBusyChange } = renderRunningMeeting({
    begin_meeting_file: () => jobId,
    get_meeting_job: () => {
      pollCount += 1;
      if (pollCount === 1) throw backendError("engine_busy", "Status check temporarily unavailable", true);
      return completedSnapshot;
    },
    create_meeting_review: () => completedReviewState,
  });
  const user = userEvent.setup();

  await waitFor(() => expect(ipc.callsTo("get_meeting_job")).toHaveLength(1));
  expect(await screen.findByText(/Retry the status check to continue\./)).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Retry status check" })).toBeInTheDocument();
  expect(screen.getByRole("progressbar", { name: "Meeting processing progress" })).toHaveAttribute(
    "aria-valuetext",
    "Meeting processing is in progress; completion percentage is unavailable",
  );
  expect(props.onComplete).not.toHaveBeenCalled();
  expect(onBusyChange).toHaveBeenLastCalledWith("meeting-file", true);
  expect(onBusyChange.mock.calls).not.toContainEqual(["meeting-file", false]);
  expect(props.onAttentionChange).toHaveBeenLastCalledWith("meeting-file", true);

  await user.click(screen.getByRole("button", { name: "Retry status check" }));
  await screen.findByDisplayValue(completedTranscript.segments[1].text);
  await waitFor(() => expect(props.onComplete).toHaveBeenCalledTimes(1));

  expect(ipc.callsTo("begin_meeting_file")).toHaveLength(1);
  expect(ipc.callsTo("get_meeting_job")).toEqual([{ jobId }, { jobId }]);
  expect(ipc.callsTo("create_meeting_review")).toEqual([{ transcript: completedSnapshot.transcript }]);
  expect(fixtureReview.original_revision).toBe(completedOriginalRevision);
  expect(fixtureReview.original).toEqual(completedSnapshot.transcript);
  expect(completedReview).toMatchObject({
    original: completedSnapshot.transcript,
    original_revision: completedOriginalRevision,
    generation: 0,
    batches: [],
    revision: completedReviewRevision,
  });
  expect(completedReviewState.review.original).toEqual(completedSnapshot.transcript);
  expect(props.onComplete).toHaveBeenCalledWith("meeting-file", "completed");
  expect(onBusyChange).toHaveBeenLastCalledWith("meeting-file", false);
  expect(props.onAttentionChange).toHaveBeenLastCalledWith("meeting-file", false);
});

test("meeting cancellation remains busy until the same job reaches its cancelled terminal snapshot", async () => {
  const terminalPoll = deferred<MeetingJobSnapshot>();
  const cancelAcknowledgement = deferred<boolean>();
  let pollCount = 0;
  const { ipc, props, view, onBusyChange } = renderRunningMeeting({
    begin_meeting_file: () => jobId,
    get_meeting_job: () => {
      pollCount += 1;
      return pollCount === 1 ? runningSnapshot : terminalPoll.promise;
    },
    cancel_meeting_job: () => cancelAcknowledgement.promise,
  });
  const user = userEvent.setup();

  try {
    await screen.findByRole("button", { name: "Cancel meeting" });
    await waitFor(() => expect(ipc.callsTo("get_meeting_job")).toHaveLength(2));
    await user.click(screen.getByRole("button", { name: "Cancel meeting" }));
    await waitFor(() => expect(ipc.callsTo("cancel_meeting_job")).toEqual([{ jobId }]));

    expect(props.onComplete).not.toHaveBeenCalled();
    expect(onBusyChange).toHaveBeenLastCalledWith("meeting-file", true);
    expect(onBusyChange.mock.calls).not.toContainEqual(["meeting-file", false]);

    cancelAcknowledgement.resolve(true);
    expect(await screen.findByRole("button", { name: "Cancelling…" })).toBeDisabled();
    expect(screen.getByRole("progressbar", { name: "Meeting processing progress" })).toHaveAttribute(
      "aria-valuetext",
      "Cancellation requested; waiting for the worker",
    );
    expect(props.onComplete).not.toHaveBeenCalled();
    expect(onBusyChange).toHaveBeenLastCalledWith("meeting-file", true);
    expect(onBusyChange.mock.calls).not.toContainEqual(["meeting-file", false]);

    terminalPoll.resolve(cancelledSnapshot);
    await waitFor(() => expect(props.onComplete).toHaveBeenCalledTimes(1));

    expect(screen.getByRole("progressbar", { name: "Meeting processing progress" })).toHaveAttribute(
      "aria-valuetext",
      "Meeting processing cancelled",
    );
    expect(ipc.callsTo("begin_meeting_file")).toHaveLength(1);
    expect(ipc.callsTo("get_meeting_job")).toEqual([{ jobId }, { jobId }]);
    expect(props.onComplete).toHaveBeenCalledWith("meeting-file", "cancelled");
    expect(onBusyChange).toHaveBeenLastCalledWith("meeting-file", false);
  } finally {
    cancelAcknowledgement.resolve(true);
    terminalPoll.resolve(cancelledSnapshot);
    view.unmount();
  }
});

test("a completed meeting without a transcript reports the import error and releases the busy queue", async () => {
  const completedWithoutTranscript: MeetingJobSnapshot = {
    ...(meetingJobFixtures.completed as unknown as MeetingJobSnapshot),
    id: jobId,
    transcript: null,
  };
  const { ipc, props, onBusyChange } = renderRunningMeeting({
    begin_meeting_file: () => jobId,
    get_meeting_job: () => completedWithoutTranscript,
  });

  expect(await screen.findByText("Meeting completed without a transcript. Try the import again.")).toBeInTheDocument();
  await waitFor(() => expect(props.onComplete).toHaveBeenCalledTimes(1));

  expect(ipc.callsTo("create_meeting_review")).toHaveLength(0);
  expect(screen.queryByRole("textbox", { name: /Edit transcript segment/ })).toBeNull();
  expect(props.onComplete).toHaveBeenCalledWith("meeting-file", "failed");
  expect(onBusyChange).toHaveBeenLastCalledWith("meeting-file", false);
});
