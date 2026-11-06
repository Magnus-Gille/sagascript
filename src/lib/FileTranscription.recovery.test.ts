// Update recovery inside FileTranscription.svelte (#361): a stored entry is applied to
// the job it belongs to, drafts and proposals reach MeetingReview, and only an explicit
// delivery (never unmounting) tells the owner to forget an entry.
import { mockWindows } from "@tauri-apps/api/mocks";
import { render, screen, waitFor } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import type { ComponentProps } from "svelte";
import { expect, test, vi } from "vitest";
import fixtures from "../../contracts/transport/get_meeting_job.json";
import { settle } from "../test/deferred";
import { mockIpc, type CommandHandlers } from "../test/ipc";
import FileTranscription from "./FileTranscription.svelte";
import type { ProposalState } from "./meeting-reprocessing-types";
import type { MeetingReviewState } from "./meeting-types";
import type { UpdateRecoveryFile, UpdateRecoveryMeeting } from "./update-recovery";

const review = fixtures.reprocessing.reprocessing.proposal.previous as MeetingReviewState["review"];
const state: MeetingReviewState = { review, transcript: review.original };
const firstSegment = review.original.segments[0];
const secondSegment = review.original.segments[1];

const proposal: ProposalState = {
  proposal: fixtures.reprocessing.reprocessing.proposal as ProposalState["proposal"],
  preview: { candidate: review, steps: [] },
  candidate: review.original,
};

function meetingRecovery(overrides: Partial<UpdateRecoveryMeeting> = {}): UpdateRecoveryMeeting {
  return {
    job_id: "job-1",
    path: "/recordings/meeting.wav",
    review: state,
    editor_draft: {
      labels: Object.fromEntries(state.transcript.speakers.map((speaker) => [speaker.id, speaker.label])),
      mergeTargets: {},
      texts: Object.fromEntries(state.transcript.segments.map((segment) => [segment.id, segment.text])),
      speakers: Object.fromEntries(state.transcript.segments.map((segment) => [segment.id, segment.speaker])),
    },
    proposal: null,
    ...overrides,
  };
}

const fileRecovery: UpdateRecoveryFile = { job_id: "job-1", path: "/recordings/note.wav", text: "Återställd text" };

function renderRecovery(extra: Partial<ComponentProps<typeof FileTranscription>>, handlers: CommandHandlers = {}) {
  const ipc = mockIpc({ set_update_result_pending: () => undefined, ...handlers });
  mockWindows("main");
  const onFileRecoveryChange = vi.fn();
  const onMeetingRecoveryChange = vi.fn();
  const props: ComponentProps<typeof FileTranscription> = {
    job: { id: "job-1", path: "/recordings/note.wav", diarize: false, prompt: "", profileId: "default", status: "queued" },
    otherBusy: false,
    active: true,
    onComplete: vi.fn(),
    onBusyChange: vi.fn(),
    onMissingFile: vi.fn(),
    onRetry: () => true,
    retryRefusal: "",
    onDownloadModel: async () => undefined,
    downloadBusy: false,
    updatePreparing: false,
    onAttentionChange: vi.fn(),
    onFileRecoveryChange,
    onMeetingRecoveryChange,
    ...extra,
  };
  const view = render(FileTranscription, { props });
  return { ipc, view, onFileRecoveryChange, onMeetingRecoveryChange };
}

test("a file recovery for this job shows its text without transcribing again", async () => {
  const { ipc } = renderRecovery({ initialRecoveryFile: fileRecovery });
  expect(await screen.findByDisplayValue("Återställd text")).toBeInTheDocument();
  await settle();
  expect(ipc.callsTo("transcribe_file")).toEqual([]);
});

test("a file recovery for another job is ignored", async () => {
  const { onFileRecoveryChange } = renderRecovery({ initialRecoveryFile: { ...fileRecovery, job_id: "other-job" } });
  await settle();
  expect(screen.queryByDisplayValue("Återställd text")).not.toBeInTheDocument();
  expect(onFileRecoveryChange).not.toHaveBeenCalledWith(expect.objectContaining({ text: "Återställd text" }));
});

test("a meeting recovery for another job is ignored", async () => {
  const { onMeetingRecoveryChange } = renderRecovery({ initialRecoveryMeeting: meetingRecovery({ job_id: "other-job" }) });
  await settle();
  expect(screen.queryByRole("button", { name: "Save review" })).not.toBeInTheDocument();
  expect(onMeetingRecoveryChange).not.toHaveBeenCalled();
});

test("a meeting recovery for this job opens as a meeting review and republishes it", async () => {
  const { onMeetingRecoveryChange } = renderRecovery({ initialRecoveryMeeting: meetingRecovery() });
  expect(await screen.findByRole("button", { name: "Save review" })).toBeInTheDocument();
  await waitFor(() => expect(onMeetingRecoveryChange).toHaveBeenCalled());
  expect(onMeetingRecoveryChange.mock.lastCall?.[0]).toMatchObject({ job_id: "job-1", review: state, proposal: null });
});

test("a recovered meeting restores its editor drafts and republishes the edited drafts", async () => {
  const edited = meetingRecovery();
  edited.editor_draft.texts[firstSegment.id] = "Återställd redigering";
  edited.editor_draft.labels["s1"] = "Alice";
  const { onMeetingRecoveryChange } = renderRecovery({ initialRecoveryMeeting: edited });
  expect(await screen.findByDisplayValue("Återställd redigering")).toBeInTheDocument();
  expect(screen.getByRole("textbox", { name: "Rename Talare 1" })).toHaveValue("Alice");
  await waitFor(() => {
    const entry = onMeetingRecoveryChange.mock.lastCall?.[0] as UpdateRecoveryMeeting;
    expect(entry.editor_draft.texts[firstSegment.id]).toBe("Återställd redigering");
    expect(entry.editor_draft.labels["s1"]).toBe("Alice");
  });

  // A further edit in the review reaches the owner as the new draft.
  const user = userEvent.setup();
  const second = screen.getByDisplayValue(secondSegment.text);
  await user.clear(second);
  await user.type(second, "Ny andra rad");
  await waitFor(() => {
    const entry = onMeetingRecoveryChange.mock.lastCall?.[0] as UpdateRecoveryMeeting;
    expect(entry.editor_draft.texts[secondSegment.id]).toBe("Ny andra rad");
    expect(entry.editor_draft.texts[firstSegment.id]).toBe("Återställd redigering");
  });
});

test("a recovered proposal is kept and republished with the review", async () => {
  const { onMeetingRecoveryChange } = renderRecovery({ initialRecoveryMeeting: meetingRecovery({ proposal }) });
  await screen.findByRole("button", { name: "Save review" });
  await waitFor(() => expect(onMeetingRecoveryChange).toHaveBeenCalled());
  expect(onMeetingRecoveryChange.mock.lastCall?.[0].proposal).toEqual(proposal);
});

test("copying or saving a recovered file result clears its recovery entry", async () => {
  const { ipc, onFileRecoveryChange } = renderRecovery(
    { initialRecoveryFile: fileRecovery },
    { copy_transcription_text: () => undefined, save_transcription_text: () => true },
  );
  await screen.findByDisplayValue("Återställd text");
  const user = userEvent.setup();
  expect(onFileRecoveryChange).not.toHaveBeenCalledWith(null);
  await user.click(screen.getByRole("button", { name: "Copy" }));
  await waitFor(() => expect(onFileRecoveryChange).toHaveBeenCalledWith(null));
  expect(ipc.callsTo("copy_transcription_text")).toEqual([{ text: "Återställd text" }]);

  onFileRecoveryChange.mockClear();
  await user.click(screen.getByRole("button", { name: /^Save/ }));
  await waitFor(() => expect(onFileRecoveryChange).toHaveBeenCalledWith(null));
  expect(ipc.callsTo("save_transcription_text")).toHaveLength(1);
});

test("saving a recovered meeting review clears its recovery entry", async () => {
  const { onMeetingRecoveryChange } = renderRecovery({ initialRecoveryMeeting: meetingRecovery() }, { save_meeting_review: () => true });
  await userEvent.setup().click(await screen.findByRole("button", { name: "Save review" }));
  await waitFor(() => expect(onMeetingRecoveryChange).toHaveBeenLastCalledWith(null));
});

test("unmounting a recovered file result keeps its recovery entry", async () => {
  const file = renderRecovery({ initialRecoveryFile: fileRecovery });
  await screen.findByDisplayValue("Återställd text");
  await settle();
  file.onFileRecoveryChange.mockClear();
  file.view.unmount();
  await settle();
  expect(file.onFileRecoveryChange).not.toHaveBeenCalled();
  expect(file.onMeetingRecoveryChange).not.toHaveBeenCalled();
});

test("unmounting a recovered meeting review keeps its recovery entry", async () => {
  const meeting = renderRecovery({ initialRecoveryMeeting: meetingRecovery() });
  await screen.findByRole("button", { name: "Save review" });
  await settle();
  meeting.onMeetingRecoveryChange.mockClear();
  meeting.view.unmount();
  await settle();
  expect(meeting.onMeetingRecoveryChange).not.toHaveBeenCalled();
});
