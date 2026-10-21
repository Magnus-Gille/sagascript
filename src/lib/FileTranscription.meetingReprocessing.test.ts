import { mockWindows } from "@tauri-apps/api/mocks";
import { render, screen, waitFor, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import type { ComponentProps } from "svelte";
import { expect, test, vi } from "vitest";
import meetingJobFixtures from "../../contracts/transport/get_meeting_job.json";
import { deferred } from "../test/deferred";
import { backendError, mockIpc, type CommandHandlers, type IpcMock } from "../test/ipc";
import FileTranscription from "./FileTranscription.svelte";
import type { MeetingJobSnapshot } from "./api";
import type { MeetingReview, MeetingReviewState, MeetingTranscript } from "./meeting-types";
import type { ProposalState, ReprocessingResult, SelectedReprocessingPlan } from "./meeting-reprocessing-types";
import type { UpdateRecoveryMeeting } from "./update-recovery";
import type { FileJob } from "./transcription-queue";

const importedTranscript = meetingJobFixtures.completed.transcript as unknown as MeetingTranscript;
const completedReprocessingSnapshot = meetingJobFixtures.reprocessing as unknown as MeetingJobSnapshot & {
  reprocessing: ReprocessingResult;
};
const reprocessingResult = completedReprocessingSnapshot.reprocessing;

if (!importedTranscript || !reprocessingResult) {
  throw new Error("Meeting transport fixtures need a completed transcript and reprocessing result.");
}

const currentTranscript: MeetingTranscript = {
  ...importedTranscript,
  speakers: importedTranscript.speakers.map((speaker) => (speaker.id === "s1" ? { ...speaker, label: "Astrid" } : speaker)),
  segments: importedTranscript.segments.map((segment) =>
    segment.id === "seg-000002" ? { ...segment, text: "Tack, då börjar vi nu." } : segment,
  ),
};
const currentReview: MeetingReview = reprocessingResult.proposal.previous;
const currentReviewState: MeetingReviewState = { review: currentReview, transcript: currentTranscript };

const [renameOperation, editOperation] = currentReview.batches[0].operations;
const proposalState: ProposalState = {
  proposal: reprocessingResult.proposal,
  preview: {
    candidate: { ...currentReview, revision: "candidate-review-revision" },
    steps: [
      { original: renameOperation, mapped: [renameOperation], status: "applied" },
      { original: editOperation, mapped: null, status: "conflict" },
    ],
  },
  candidate: reprocessingResult.proposal.proposed,
};

const selectedPlan: SelectedReprocessingPlan = {
  file_path: "/recordings/meeting.wav",
  cache_path: null,
  cache_output: null,
  plan: {
    schema_version: 1,
    mode: "recluster",
    context: {
      source_sha256: currentReview.original.source_sha256,
      previous_revision: currentReview.revision,
      transcription_context_sha256: "c".repeat(64),
      analysis_context_sha256: "d".repeat(64),
      cache_sha256: null,
    },
    threshold: 0.34,
    required_work: reprocessingResult.required_work,
    revision: "selected-plan-revision",
  },
};

const reprocessingJobSnapshot: MeetingJobSnapshot = {
  ...completedReprocessingSnapshot,
  id: "reprocessing-job",
};

function recoveryFor(
  jobId: string,
  reviewState: MeetingReviewState = currentReviewState,
  proposal: ProposalState | null = null,
): UpdateRecoveryMeeting {
  const transcript = reviewState.transcript;
  return {
    job_id: jobId,
    path: "/recordings/meeting.wav",
    review: reviewState,
    editor_draft: {
      labels: Object.fromEntries(transcript.speakers.map((speaker) => [speaker.id, speaker.label])),
      mergeTargets: {},
      texts: Object.fromEntries(transcript.segments.map((segment) => [segment.id, segment.text])),
      speakers: Object.fromEntries(transcript.segments.map((segment) => [segment.id, segment.speaker])),
    },
    proposal,
  };
}

function componentProps(
  recovery: UpdateRecoveryMeeting | null,
  jobId = recovery?.job_id ?? "meeting-file",
  status: FileJob["status"] = "completed",
): ComponentProps<typeof FileTranscription> {
  const job: FileJob = {
    id: jobId,
    path: recovery?.path ?? "/recordings/meeting.wav",
    diarize: true,
    prompt: "Use the recorded speaker names",
    profileId: "interview-profile",
    status,
  };
  return {
    job,
    otherBusy: false,
    active: true,
    initialRecoveryMeeting: recovery,
    onComplete: () => undefined,
    onBusyChange: () => undefined,
    onMissingFile: () => undefined,
    onRetry: () => true,
    retryRefusal: "",
    onDownloadModel: async () => undefined,
    downloadBusy: false,
    updatePreparing: false,
    onAttentionChange: () => undefined,
  };
}

function meetingBackend(overrides: CommandHandlers = {}): CommandHandlers {
  return {
    plan_meeting_reprocessing: () => selectedPlan,
    begin_meeting_reprocessing: () => "reprocessing-job",
    get_meeting_job: () => reprocessingJobSnapshot,
    preview_meeting_proposal: () => proposalState,
    save_meeting_proposal: () => true,
    set_update_result_pending: () => undefined,
    ...overrides,
  };
}

async function renderRecoveredMeeting(recovery: UpdateRecoveryMeeting, handlers: CommandHandlers = meetingBackend()) {
  const ipc = mockIpc(handlers);
  mockWindows("main");
  const props = componentProps(recovery);
  const view = render(FileTranscription, { props });
  await screen.findByRole("heading", { name: "Reprocess a meeting" });
  await screen.findByDisplayValue(currentTranscript.segments[1].text);
  return { ipc, props, view };
}

async function planAndExecute(user: ReturnType<typeof userEvent.setup>, ipc: IpcMock): Promise<void> {
  await user.click(screen.getByRole("button", { name: "Plan reprocessing" }));
  await screen.findByRole("heading", { name: "Selected plan" });
  await user.click(screen.getByRole("button", { name: "Execute selected Recluster plan" }));
  await waitFor(() => expect(ipc.callsTo("get_meeting_job")).toHaveLength(1));
  await screen.findByRole("heading", { name: "Reprocessing result" });
}

test("planning uses the active review, while a visible dirty draft blocks planning and execution", async () => {
  const recovery = recoveryFor("meeting-file");
  const { ipc } = await renderRecoveredMeeting(recovery);
  const user = userEvent.setup();

  await user.click(screen.getByRole("button", { name: "Plan reprocessing" }));
  await screen.findByRole("heading", { name: "Selected plan" });

  expect(ipc.callsTo("plan_meeting_reprocessing")).toEqual([
    {
      previous: currentReview,
      mode: "recluster",
      threshold: 0.34,
      saveCache: false,
      prompt: "Use the recorded speaker names",
      profileId: "interview-profile",
    },
  ]);
  expect(screen.getByRole("textbox", { name: "Edit transcript segment at 0:03" })).toHaveValue(currentTranscript.segments[1].text);

  const transcriptDraft = screen.getByRole("textbox", { name: "Edit transcript segment at 0:03" });
  await user.clear(transcriptDraft);
  await user.type(transcriptDraft, "Unapplied words from the editor");

  expect(screen.getByText(/Unapplied edits are not included/)).toBeInTheDocument();
  expect(screen.getByText(/Apply or discard the unsaved edits in Meeting review/)).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Plan reprocessing" })).toBeDisabled();
  expect(screen.getByRole("button", { name: "Execute selected Recluster plan" })).toBeDisabled();

  expect(ipc.callsTo("plan_meeting_reprocessing")).toHaveLength(1);
  expect(ipc.callsTo("begin_meeting_reprocessing")).toHaveLength(0);
});

test("a completed reprocessing job presents its migration proposal beside the unchanged active review", async () => {
  const { ipc } = await renderRecoveredMeeting(recoveryFor("meeting-file"));
  const user = userEvent.setup();

  await planAndExecute(user, ipc);

  expect(screen.queryByRole("heading", { name: "Selected plan" })).toBeNull();
  expect(await screen.findByText("Conflict needs a decision")).toBeInTheDocument();
  expect(screen.getByText("Applied automatically")).toBeInTheDocument();
  expect(screen.getByRole("textbox", { name: "Edit transcript segment at 0:03" })).toHaveValue(currentTranscript.segments[1].text);
  expect(screen.getByRole("heading", { name: "2. Review migration proposal" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Accept proposal" })).toBeDisabled();
  expect(ipc.callsTo("begin_meeting_reprocessing")).toHaveLength(1);
  expect(ipc.callsTo("preview_meeting_proposal")).toEqual([{ proposal: reprocessingResult.proposal }]);
  expect(ipc.callsTo("accept_meeting_proposal")).toHaveLength(0);
  expect(ipc.calls.map((call) => call.command)).toEqual([
    "plan_meeting_reprocessing",
    "begin_meeting_reprocessing",
    "get_meeting_job",
    "preview_meeting_proposal",
  ]);
});

test("a failed preview keeps the completed result available to save and retry", async () => {
  let previews = 0;
  const { ipc } = await renderRecoveredMeeting(
    recoveryFor("meeting-file"),
    meetingBackend({
      preview_meeting_proposal: () => {
        previews += 1;
        if (previews === 1) throw backendError("meeting_stale", "Preview is temporarily unavailable.");
        return proposalState;
      },
    }),
  );
  const user = userEvent.setup();

  await planAndExecute(user, ipc);

  expect(screen.queryByRole("heading", { name: "Selected plan" })).toBeNull();
  expect(await screen.findByText("The completed proposal is retained. Retry its preview or save it to open later.")).toBeInTheDocument();
  expect(screen.queryByRole("heading", { name: "2. Review migration proposal" })).toBeNull();
  expect(screen.getByRole("textbox", { name: "Edit transcript segment at 0:03" })).toHaveValue(currentTranscript.segments[1].text);

  await user.click(screen.getByRole("button", { name: "Save retained proposal" }));
  await waitFor(() => expect(ipc.callsTo("save_meeting_proposal")).toEqual([{ proposal: reprocessingResult.proposal }]));

  await user.click(screen.getByRole("button", { name: "Retry proposal preview" }));
  expect(await screen.findByRole("heading", { name: "2. Review migration proposal" })).toBeInTheDocument();
  expect(screen.getByText("Conflict needs a decision")).toBeInTheDocument();
  expect(screen.getByRole("textbox", { name: "Edit transcript segment at 0:03" })).toHaveValue(currentTranscript.segments[1].text);
  expect(ipc.callsTo("preview_meeting_proposal")).toHaveLength(2);
  expect(ipc.calls.map((call) => call.command)).toEqual([
    "plan_meeting_reprocessing",
    "begin_meeting_reprocessing",
    "get_meeting_job",
    "preview_meeting_proposal",
    "save_meeting_proposal",
    "set_update_result_pending",
    "preview_meeting_proposal",
  ]);
});

test("a late acceptance response cannot replace a newer recovered meeting", async () => {
  const readyReview: MeetingReview = {
    ...currentReview,
    batches: [],
    revision: "acceptance-review-revision",
  };
  const proposedTranscript: MeetingTranscript = {
    ...currentTranscript,
    segments: currentTranscript.segments.map((segment, index) =>
      index === 0 ? { ...segment, text: "Proposed transcript text." } : segment,
    ),
  };
  const acceptedProposalState: ProposalState = {
    proposal: {
      schema_version: 1,
      previous: readyReview,
      proposed: proposedTranscript,
      generation: 2,
      resolutions: [],
      revision: "acceptance-proposal-revision",
    },
    preview: {
      candidate: { ...readyReview, revision: "acceptance-candidate-revision" },
      steps: [],
    },
    candidate: proposedTranscript,
  };
  const replacementTranscript: MeetingTranscript = {
    ...currentTranscript,
    segments: currentTranscript.segments.map((segment, index) =>
      index === 0 ? { ...segment, text: "A separately recovered meeting." } : segment,
    ),
  };
  const replacementReview: MeetingReview = {
    ...readyReview,
    revision: "replacement-review-revision",
  };
  const replacementRecovery = recoveryFor("replacement-file", { review: replacementReview, transcript: replacementTranscript });
  const lateAcceptedState: MeetingReviewState = {
    review: { ...readyReview, revision: "late-accepted-review" },
    transcript: {
      ...proposedTranscript,
      segments: proposedTranscript.segments.map((segment, index) =>
        index === 0 ? { ...segment, text: "Late result from the old proposal." } : segment,
      ),
    },
  };
  const acceptResponse = deferred<MeetingReviewState>();
  const { ipc, view, props } = await renderRecoveredMeeting(
    recoveryFor("meeting-file", { review: readyReview, transcript: currentTranscript }, acceptedProposalState),
    meetingBackend({
      accept_meeting_proposal: () => acceptResponse.promise,
    }),
  );
  const user = userEvent.setup();

  await user.click(screen.getByRole("button", { name: "Save proposal" }));
  await screen.findByText("Reprocessing proposal saved.");
  await user.click(screen.getByRole("button", { name: "Accept proposal" }));
  await waitFor(() => expect(ipc.callsTo("accept_meeting_proposal")).toHaveLength(1));
  expect(ipc.callsTo("accept_meeting_proposal")[0].currentReview.revision).toBe(readyReview.revision);

  await view.rerender({
    ...props,
    job: { ...props.job, id: "replacement-file" },
    initialRecoveryMeeting: replacementRecovery,
  });
  expect(await screen.findByDisplayValue("A separately recovered meeting.")).toBeInTheDocument();

  acceptResponse.resolve(lateAcceptedState);

  expect(await screen.findByRole("alert")).toHaveTextContent("The review changed before acceptance.");
  expect(screen.getByRole("textbox", { name: "Edit transcript segment at 0:00" })).toHaveValue("A separately recovered meeting.");
  expect(screen.queryByDisplayValue("Late result from the old proposal.")).toBeNull();
});

test("the rendered meeting clock interpolates native time and ignores a stale poll", async () => {
  vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
  let now = 100;
  const performanceNow = vi.spyOn(performance, "now").mockImplementation(() => now);
  const stalePoll = deferred<MeetingJobSnapshot>();
  const terminalPoll = deferred<MeetingJobSnapshot>();
  let pollCount = 0;
  const completedSnapshot: MeetingJobSnapshot = {
    id: "clock-job",
    status: "completed",
    phase: "finalizing",
    elapsed_ms: 126_000,
    phase_elapsed_ms: 1_000,
    error: null,
    transcript: null,
  };

  try {
    const ipc = mockIpc({
      begin_meeting_file: () => "clock-job",
      get_meeting_job: ({ jobId }) => {
        pollCount += 1;
        if (pollCount === 1) {
          return {
            id: jobId,
            status: "running",
            phase: "analyzing",
            elapsed_ms: 125_000,
            phase_elapsed_ms: 65_000,
            error: null,
            transcript: null,
          };
        }
        if (pollCount === 2) return stalePoll.promise;
        if (pollCount === 3) return terminalPoll.promise;
        return completedSnapshot;
      },
      create_meeting_review: () => {
        throw new Error("This timer test does not expect transcript initialization.");
      },
    });
    mockWindows("main");
    render(FileTranscription, {
      props: componentProps(null, "clock-file", "running"),
    });

    const progress = await screen.findByLabelText("Meeting progress");
    await within(progress).findByText("Elapsed: 02:05");
    expect(within(progress).getByText("This stage: 01:05")).toBeInTheDocument();

    const waitForPollCount = async (expected: number): Promise<void> => {
      const deadline = Date.now() + 2_000;
      while (ipc.callsTo("get_meeting_job").length < expected && Date.now() < deadline) {
        await new Promise((resolve) => window.setTimeout(resolve, 5));
      }
      expect(ipc.callsTo("get_meeting_job")).toHaveLength(expected);
    };
    await waitForPollCount(2);

    now = 1_100;
    await vi.advanceTimersByTimeAsync(1_000);
    expect(within(progress).getByText("Elapsed: 02:06")).toBeInTheDocument();

    stalePoll.resolve({
      id: "clock-job",
      status: "running",
      phase: "analyzing",
      elapsed_ms: 120_000,
      phase_elapsed_ms: 60_000,
      error: null,
      transcript: null,
    });

    await waitForPollCount(3);
    expect(within(progress).getByText("Elapsed: 02:06")).toBeInTheDocument();
    expect(within(progress).getByRole("heading", { name: "Processing meeting" })).toBeInTheDocument();

    terminalPoll.resolve(completedSnapshot);
    expect(await within(progress).findByText("Meeting processing complete")).toBeInTheDocument();
    expect(within(progress).getByText("Elapsed: 02:06")).toBeInTheDocument();
    expect(within(progress).getByText("This stage: 00:01")).toBeInTheDocument();
  } finally {
    performanceNow.mockRestore();
    vi.useRealTimers();
  }
});
