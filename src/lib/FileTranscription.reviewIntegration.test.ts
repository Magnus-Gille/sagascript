import { mockWindows } from "@tauri-apps/api/mocks";
import { render, screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import type { ComponentProps } from "svelte";
import { expect, test, vi } from "vitest";
import fixtures from "../../contracts/transport/get_meeting_job.json";
import { deferred } from "../test/deferred";
import { backendError, mockIpc, type CommandHandlers } from "../test/ipc";
import FileTranscription from "./FileTranscription.svelte";
import type { MeetingAudioAttachment, MeetingReviewState } from "./meeting-types";
import type { UpdateRecoveryMeeting } from "./update-recovery";

const review = fixtures.reprocessing.reprocessing.proposal.previous as MeetingReviewState["review"];
const state: MeetingReviewState = { review, transcript: review.original };
const recovery: UpdateRecoveryMeeting = {
  job_id: "retained-review",
  path: "/recordings/retained.wav",
  review: state,
  editor_draft: {
    labels: Object.fromEntries(state.transcript.speakers.map((speaker) => [speaker.id, speaker.label])),
    mergeTargets: {},
    texts: Object.fromEntries(state.transcript.segments.map((segment) => [segment.id, segment.text])),
    speakers: Object.fromEntries(state.transcript.segments.map((segment) => [segment.id, segment.speaker])),
  },
  proposal: null,
};

async function renderReview(handlers: CommandHandlers) {
  const ipc = mockIpc({ set_update_result_pending: () => undefined, ...handlers });
  mockWindows("main");
  const onMeetingRecoveryChange = vi.fn();
  const props: ComponentProps<typeof FileTranscription> = {
    job: { id: recovery.job_id, path: recovery.path, diarize: true, prompt: "Speaker names", profileId: "default", status: "completed" },
    otherBusy: false,
    active: true,
    initialRecoveryMeeting: recovery,
    onComplete: vi.fn(),
    onBusyChange: vi.fn(),
    onMissingFile: vi.fn(),
    onRetry: () => true,
    retryRefusal: "",
    onDownloadModel: async () => undefined,
    downloadBusy: false,
    updatePreparing: false,
    onAttentionChange: vi.fn(),
    onMeetingRecoveryChange,
  };
  const view = render(FileTranscription, { props });
  await screen.findByRole("button", { name: "Save review" });
  return { ipc, view, props, onMeetingRecoveryChange };
}

test.each([
  ["Save review", "json"],
  ["Plain text", "plain"],
  ["Markdown", "markdown"],
  ["Review JSON", "json"],
  ["SRT subtitles", "srt"],
  ["WebVTT subtitles", "vtt"],
] as const)("%s persists the active review and clears its recovery only after success", async (name, format) => {
  const saved = deferred<boolean>();
  const { ipc, onMeetingRecoveryChange } = await renderReview({ save_meeting_review: () => saved.promise });
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name }));
  await waitFor(() => expect(ipc.callsTo("save_meeting_review")).toEqual([{ review, format }]));
  expect(onMeetingRecoveryChange.mock.calls).not.toContainEqual([null]);
  expect(ipc.callsTo("set_update_result_pending")).not.toContainEqual({ resultId: "meeting:retained-review", pending: false });
  saved.resolve(true);
  await waitFor(() => expect(onMeetingRecoveryChange).toHaveBeenLastCalledWith(null));
  expect(ipc.callsTo("set_update_result_pending")).toContainEqual({ resultId: "meeting:retained-review", pending: false });
});

test("a queued plan waits for a pending undo and still runs after that action rejects", async () => {
  const undone = deferred<MeetingReviewState>();
  const { ipc } = await renderReview({ undo_meeting_review: () => undone.promise, plan_meeting_reprocessing: () => null });
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: "Undo" }));
  await waitFor(() => expect(ipc.callsTo("undo_meeting_review")).toEqual([{ review, expectedRevision: review.revision }]));
  await user.click(screen.getByRole("button", { name: "Plan reprocessing" }));
  expect(ipc.callsTo("plan_meeting_reprocessing")).toHaveLength(0);
  undone.reject(backendError("review_conflict", "The correction could not be undone."));
  await screen.findByText("The correction could not be undone.");
  await waitFor(() => expect(ipc.callsTo("plan_meeting_reprocessing")).toHaveLength(1));
  expect(ipc.callsTo("plan_meeting_reprocessing")[0]).toMatchObject({ previous: review, prompt: "Speaker names", profileId: "default" });
  expect(screen.getByDisplayValue(state.transcript.segments[0].text)).toBeInTheDocument();
});

test("a late audio attachment is detached when its review unmounts before the native picker returns", async () => {
  const attached = deferred<MeetingAudioAttachment | null>();
  const { ipc, view } = await renderReview({ attach_meeting_audio: () => attached.promise, detach_meeting_audio: () => undefined });
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: /Attach.*audio/i }));
  await waitFor(() => expect(ipc.callsTo("attach_meeting_audio")).toEqual([{ sourceSha256: review.original.source_sha256 }]));
  view.unmount();
  attached.resolve({ token: "late-test-token", mime: "audio/wav" });
  await waitFor(() => expect(ipc.callsTo("detach_meeting_audio")).toEqual([{ token: "late-test-token" }]));
});
