import { mockWindows } from "@tauri-apps/api/mocks";
import { render, screen, waitFor, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { tick, type ComponentProps } from "svelte";
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

const currentReview: MeetingReview = reprocessingResult.proposal.previous;
const currentTranscript: MeetingTranscript = {
  ...currentReview.original,
  speakers: currentReview.original.speakers.map((speaker) => (speaker.id === "s1" ? { ...speaker, label: "Astrid" } : speaker)),
  segments: currentReview.original.segments.map((segment) =>
    segment.id === "seg-000002" ? { ...segment, text: "Tack, då börjar vi nu." } : segment,
  ),
};
const currentReviewState: MeetingReviewState = { review: currentReview, transcript: currentTranscript };

const [renameOperation, editOperation] = currentReview.batches[0].operations;
const reprocessedTranscript: MeetingTranscript = {
  ...reprocessingResult.proposal.proposed,
  segments: reprocessingResult.proposal.proposed.segments.map((segment) =>
    segment.id === "seg-000002" ? { ...segment, text: "Tack, vi börjar med dagordningen." } : segment,
  ),
};
const reprocessingProposal = {
  ...reprocessingResult.proposal,
  proposed: reprocessedTranscript,
  revision: "reprocessing-proposal-revision",
};
const completedReprocessingResult: ReprocessingResult = {
  ...reprocessingResult,
  proposal: reprocessingProposal,
};
const previewCandidateReview: MeetingReview = {
  ...currentReview,
  original: reprocessedTranscript,
  original_revision: "f".repeat(64),
  generation: 1,
  batches: [{ operations: [renameOperation] }],
  revision: "candidate-review-revision",
};
const previewCandidateTranscript: MeetingTranscript = {
  ...reprocessedTranscript,
  speakers: reprocessedTranscript.speakers.map((speaker) => (speaker.id === "s1" ? { ...speaker, label: "Astrid" } : speaker)),
};
const proposalState: ProposalState = {
  proposal: reprocessingProposal,
  preview: {
    candidate: previewCandidateReview,
    steps: [
      { original: renameOperation, mapped: [renameOperation], status: "applied" },
      { original: editOperation, mapped: null, status: "conflict" },
    ],
  },
  candidate: previewCandidateTranscript,
};

function noCorrectionReview(transcript: MeetingTranscript, revision: string, originalRevision = "e".repeat(64)): MeetingReview {
  return {
    ...currentReview,
    original: transcript,
    original_revision: originalRevision,
    generation: 0,
    batches: [],
    revision,
  };
}

// `create_meeting_review` returns MeetingReview::new(importedTranscript): the
// fixture's original hash is retained, but the constructor starts at generation
// zero with no batches and computes this revision from its serde payload.
const importedReview = noCorrectionReview(
  importedTranscript,
  "668400f77910581ef62e7e03f6dc83de085e2e1d8f0a7b4186856be189c50b35",
  currentReview.original_revision,
);
const importedReviewState: MeetingReviewState = { review: importedReview, transcript: importedTranscript };

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

// These ordered fields match ReprocessingPlan::RevisionPayload and
// RequiredWork. The revision is SHA-256 of Rust's compact serde_json payload;
// its exact bytes are recorded in the companion fixture evidence. The
// previous_revision binds the plan to the fresh imported review.
const clockSelectedPlan: SelectedReprocessingPlan = {
  file_path: "/recordings/meeting.wav",
  cache_path: "/recordings/meeting-cache.json",
  cache_output: null,
  plan: {
    schema_version: 1,
    mode: "recluster",
    context: {
      source_sha256: importedTranscript.source_sha256,
      previous_revision: importedReview.revision,
      transcription_context_sha256: "c".repeat(64),
      analysis_context_sha256: "d".repeat(64),
      cache_sha256: "e".repeat(64),
    },
    threshold: 0.34,
    required_work: {
      decode_audio: false,
      transcription: false,
      language_detection: false,
      segmentation: false,
      embeddings: false,
      clustering: true,
    },
    revision: "ad99e84a2dd7fb67818d32505a744a7738f33f078abf63a7161d30c01a067ca9",
  },
};

const reprocessingJobSnapshot: MeetingJobSnapshot = {
  ...completedReprocessingSnapshot,
  id: "reprocessing-job",
  reprocessing: completedReprocessingResult,
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
  const recoveryChanges: Array<UpdateRecoveryMeeting | null> = [];
  const props = componentProps(recovery);
  props.onMeetingRecoveryChange = (entry) => recoveryChanges.push(entry);
  const view = render(FileTranscription, { props });
  await screen.findByRole("heading", { name: "Reprocess a meeting" });
  await screen.findByDisplayValue(recovery.review.transcript.segments[1].text);
  await tick();
  return { ipc, props, view, recoveryChanges };
}

function expectLatestActiveReview(recoveryChanges: Array<UpdateRecoveryMeeting | null>): void {
  const latest = recoveryChanges.at(-1);
  expect(latest).not.toBeNull();
  expect(latest?.review.review).toEqual(currentReview);
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
  const { ipc, recoveryChanges } = await renderRecoveredMeeting(recovery);
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
  await tick();
  expectLatestActiveReview(recoveryChanges);

  await user.click(screen.getByRole("button", { name: "Plan reprocessing" }));
  await waitFor(() => expect(ipc.callsTo("plan_meeting_reprocessing")).toHaveLength(2));
  expect(ipc.callsTo("plan_meeting_reprocessing")[1].previous).toEqual(currentReview);
  expect(ipc.callsTo("plan_meeting_reprocessing")[1].previous).not.toEqual(proposalState.preview.candidate);

  const transcriptDraft = screen.getByRole("textbox", { name: "Edit transcript segment at 0:03" });
  await user.clear(transcriptDraft);
  await user.type(transcriptDraft, "Unapplied words from the editor");

  expect(screen.getByText(/Unapplied edits are not included/)).toBeInTheDocument();
  expect(screen.getByText(/Apply or discard the unsaved edits in Meeting review/)).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Plan reprocessing" })).toBeDisabled();
  expect(screen.getByRole("button", { name: "Execute selected Recluster plan" })).toBeDisabled();

  expect(ipc.callsTo("plan_meeting_reprocessing")).toHaveLength(2);
  expect(ipc.callsTo("begin_meeting_reprocessing")).toHaveLength(0);
});

test("a completed reprocessing job presents its migration proposal beside the unchanged active review", async () => {
  const { ipc, recoveryChanges } = await renderRecoveredMeeting(recoveryFor("meeting-file"));
  const user = userEvent.setup();

  await planAndExecute(user, ipc);

  expect(screen.queryByRole("heading", { name: "Selected plan" })).toBeNull();
  expect(await screen.findByText("Conflict needs a decision")).toBeInTheDocument();
  expect(screen.getByText("Applied automatically")).toBeInTheDocument();
  expect(screen.getByRole("textbox", { name: "Edit transcript segment at 0:03" })).toHaveValue(currentTranscript.segments[1].text);
  expect(screen.getByRole("heading", { name: "2. Review migration proposal" })).toBeInTheDocument();
  expect(screen.getByRole("button", { name: "Accept proposal" })).toBeDisabled();
  expect(screen.queryByText(/This proposal was made from an older review revision/)).toBeNull();
  await tick();
  expectLatestActiveReview(recoveryChanges);
  expect(ipc.callsTo("begin_meeting_reprocessing")).toHaveLength(1);
  expect(ipc.callsTo("preview_meeting_proposal")).toEqual([{ proposal: reprocessingProposal }]);
  expect(ipc.callsTo("accept_meeting_proposal")).toHaveLength(0);
  expect(ipc.calls.map((call) => call.command)).toEqual([
    "plan_meeting_reprocessing",
    "begin_meeting_reprocessing",
    "get_meeting_job",
    "preview_meeting_proposal",
  ]);

  await user.click(screen.getByRole("button", { name: "Plan reprocessing" }));
  await waitFor(() => expect(ipc.callsTo("plan_meeting_reprocessing")).toHaveLength(2));
  expect(ipc.callsTo("plan_meeting_reprocessing")[1].previous).toEqual(currentReview);
  expect(ipc.callsTo("plan_meeting_reprocessing")[1].previous).not.toEqual(proposalState.preview.candidate);
});

test("a failed preview keeps the completed result available to save and retry", async () => {
  let previews = 0;
  const { ipc, recoveryChanges } = await renderRecoveredMeeting(
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
  await tick();
  expectLatestActiveReview(recoveryChanges);
  expect(screen.queryByRole("heading", { name: "2. Review migration proposal" })).toBeNull();
  expect(screen.getByRole("textbox", { name: "Edit transcript segment at 0:03" })).toHaveValue(currentTranscript.segments[1].text);

  await user.click(screen.getByRole("button", { name: "Save retained proposal" }));
  await waitFor(() => expect(ipc.callsTo("save_meeting_proposal")).toEqual([{ proposal: reprocessingProposal }]));

  await user.click(screen.getByRole("button", { name: "Retry proposal preview" }));
  expect(await screen.findByRole("heading", { name: "2. Review migration proposal" })).toBeInTheDocument();
  expect(screen.getByText("Conflict needs a decision")).toBeInTheDocument();
  expect(screen.queryByText(/This proposal was made from an older review revision/)).toBeNull();
  expect(screen.getByRole("textbox", { name: "Edit transcript segment at 0:03" })).toHaveValue(currentTranscript.segments[1].text);
  await tick();
  expectLatestActiveReview(recoveryChanges);
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

  await user.click(screen.getByRole("button", { name: "Plan reprocessing" }));
  await waitFor(() => expect(ipc.callsTo("plan_meeting_reprocessing")).toHaveLength(2));
  expect(ipc.callsTo("plan_meeting_reprocessing")[1].previous).toEqual(currentReview);
  expect(ipc.callsTo("plan_meeting_reprocessing")[1].previous).not.toEqual(proposalState.preview.candidate);
});

test("a late acceptance response cannot replace a newer recovered meeting", async () => {
  const readyTranscript = currentReview.original;
  const readyReview = noCorrectionReview(readyTranscript, "acceptance-review-revision", currentReview.original_revision);
  const proposedTranscript: MeetingTranscript = {
    ...readyTranscript,
    segments: readyTranscript.segments.map((segment, index) =>
      index === 0 ? { ...segment, text: "Late result from the old proposal." } : segment,
    ),
  };
  const acceptanceCandidateReview = noCorrectionReview(proposedTranscript, "acceptance-candidate-revision");
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
      candidate: acceptanceCandidateReview,
      steps: [],
    },
    candidate: proposedTranscript,
  };
  const replacementTranscript: MeetingTranscript = {
    ...readyTranscript,
    segments: readyTranscript.segments.map((segment, index) =>
      index === 0 ? { ...segment, text: "A separately recovered meeting." } : segment,
    ),
  };
  const replacementReview = noCorrectionReview(replacementTranscript, "replacement-review-revision");
  const replacementRecovery = recoveryFor("replacement-file", {
    review: replacementReview,
    transcript: replacementTranscript,
  });
  const lateAcceptedState: MeetingReviewState = {
    review: { ...acceptanceCandidateReview, revision: "late-accepted-review" },
    transcript: proposedTranscript,
  };
  const acceptResponse = deferred<MeetingReviewState>();
  const { ipc, view, props } = await renderRecoveredMeeting(
    recoveryFor("meeting-file", { review: readyReview, transcript: readyTranscript }, acceptedProposalState),
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
  const beginResponse = deferred<string>();
  const stalePoll = deferred<MeetingJobSnapshot>();
  const terminalPoll = deferred<MeetingJobSnapshot>();
  let pollCount = 0;
  let view: ReturnType<typeof render> | undefined;
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
      begin_meeting_file: () => beginResponse.promise,
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
    view = render(FileTranscription, {
      props: componentProps(null, "clock-file", "running"),
    });

    const progress = await screen.findByLabelText("Meeting progress");
    expect(within(progress).getByText("Elapsed: 00:00")).toBeInTheDocument();
    expect(within(progress).getByText("This stage: 00:00")).toBeInTheDocument();
    expect(within(progress).getByText("No status response yet — processing may still be running.")).toBeInTheDocument();
    expect(within(progress).queryByText("Status checked just now")).toBeNull();

    beginResponse.resolve("clock-job");
    await within(progress).findByText("Elapsed: 02:05");
    expect(within(progress).getByText("This stage: 01:05")).toBeInTheDocument();
    expect(within(progress).getByText("Status checked just now")).toBeInTheDocument();

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
    expect(within(progress).getByText("This stage: 01:06")).toBeInTheDocument();
    expect(within(progress).getByText("Status checked 1 second ago")).toBeInTheDocument();

    now = 2_100;
    await vi.advanceTimersByTimeAsync(1_000);
    expect(ipc.callsTo("get_meeting_job")).toHaveLength(2);
    expect(within(progress).getByText("Elapsed: 02:07")).toBeInTheDocument();
    expect(within(progress).getByText("This stage: 01:07")).toBeInTheDocument();

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
    // The third request can only start after the serialized poll loop has applied the stale response.
    expect(within(progress).getByText("Elapsed: 02:07")).toBeInTheDocument();
    expect(within(progress).getByText("This stage: 01:07")).toBeInTheDocument();
    expect(within(progress).getByText("Status checked just now")).toBeInTheDocument();
    expect(within(progress).getByRole("heading", { name: "Processing meeting" })).toBeInTheDocument();

    terminalPoll.resolve(completedSnapshot);
    expect(await within(progress).findByText("Meeting processing complete")).toBeInTheDocument();
    expect(within(progress).getByText("Elapsed: 02:06")).toBeInTheDocument();
    expect(within(progress).getByText("This stage: 00:01")).toBeInTheDocument();
    expect(within(progress).queryByText("Elapsed: 02:07")).toBeNull();

    now = 3_100;
    await vi.advanceTimersByTimeAsync(1_000);
    expect(within(progress).getByText("Elapsed: 02:06")).toBeInTheDocument();
    expect(within(progress).getByText("This stage: 00:01")).toBeInTheDocument();
  } finally {
    view?.unmount();
    beginResponse.resolve("clock-job");
    stalePoll.resolve({
      id: "clock-job",
      status: "running",
      phase: "analyzing",
      elapsed_ms: 120_000,
      phase_elapsed_ms: 60_000,
      error: null,
      transcript: null,
    });
    terminalPoll.resolve(completedSnapshot);
    await tick();
    performanceNow.mockRestore();
    vi.useRealTimers();
  }
});

test("reprocessing resets the same rendered meeting clock before accepting its native timing", async () => {
  vi.useFakeTimers({ toFake: ["setInterval", "clearInterval"] });
  let now = 100;
  const performanceNow = vi.spyOn(performance, "now").mockImplementation(() => now);
  const delayedInitialPoll = deferred<MeetingJobSnapshot>();
  const delayedInitialTerminalPoll = deferred<MeetingJobSnapshot>();
  const secondRunBegin = deferred<string>();
  const delayedReprocessingPoll = deferred<MeetingJobSnapshot>();
  let pollCount = 0;
  let view: ReturnType<typeof render> | undefined;

  const waitForPollCount = async (ipc: IpcMock, expected: number): Promise<void> => {
    const deadline = Date.now() + 2_000;
    while (ipc.callsTo("get_meeting_job").length < expected && Date.now() < deadline) {
      await new Promise((resolve) => window.setTimeout(resolve, 5));
    }
    expect(ipc.callsTo("get_meeting_job")).toHaveLength(expected);
  };

  try {
    const ipc = mockIpc({
      begin_meeting_file: () => "import-job",
      begin_meeting_reprocessing: () => secondRunBegin.promise,
      create_meeting_review: () => importedReviewState,
      plan_meeting_reprocessing: () => clockSelectedPlan,
      get_meeting_job: ({ jobId }) => {
        pollCount += 1;
        if (pollCount === 1) {
          return {
            id: jobId,
            status: "running",
            phase: "analyzing",
            elapsed_ms: 5_000,
            phase_elapsed_ms: 3_000,
            error: null,
            transcript: null,
          };
        }
        if (pollCount === 2) return delayedInitialPoll.promise;
        if (pollCount === 3) return delayedInitialTerminalPoll.promise;
        if (pollCount === 4) return delayedReprocessingPoll.promise;
        return {
          id: jobId,
          status: "completed",
          phase: "finalizing",
          elapsed_ms: 25_000,
          phase_elapsed_ms: 12_000,
          error: null,
          transcript: null,
        };
      },
    });
    mockWindows("main");
    view = render(FileTranscription, {
      props: componentProps(null, "same-clock-file", "running"),
    });

    const progress = await screen.findByLabelText("Meeting progress");
    await within(progress).findByText("Elapsed: 00:05");
    expect(within(progress).getByText("This stage: 00:03")).toBeInTheDocument();
    await waitForPollCount(ipc, 2);

    now = 3_100;
    await vi.advanceTimersByTimeAsync(3_000);
    expect(within(progress).getByText("Elapsed: 00:08")).toBeInTheDocument();
    expect(within(progress).getByText("This stage: 00:06")).toBeInTheDocument();
    expect(within(progress).getByText("Status checked 3 seconds ago")).toBeInTheDocument();

    delayedInitialPoll.resolve({
      id: "import-job",
      status: "running",
      phase: "analyzing",
      elapsed_ms: 4_000,
      phase_elapsed_ms: 2_000,
      error: null,
      transcript: null,
    });
    await waitForPollCount(ipc, 3);
    expect(within(progress).getByText("Elapsed: 00:08")).toBeInTheDocument();
    expect(within(progress).getByText("This stage: 00:06")).toBeInTheDocument();
    expect(within(progress).getByText("Status checked just now")).toBeInTheDocument();

    delayedInitialTerminalPoll.resolve({
      id: "import-job",
      status: "completed",
      phase: "finalizing",
      elapsed_ms: 9_000,
      phase_elapsed_ms: 1_000,
      error: null,
      transcript: importedTranscript,
    });

    expect(await screen.findByRole("heading", { name: "Reprocess a meeting" })).toBeInTheDocument();
    expect(ipc.callsTo("create_meeting_review")).toEqual([{ transcript: importedTranscript }]);
    expect(screen.getByRole("textbox", { name: "Edit transcript segment at 0:03" })).toHaveValue(importedTranscript.segments[1].text);
    expect(await within(progress).findByText("Meeting processing complete")).toBeInTheDocument();
    expect(within(progress).getByText("Elapsed: 00:09")).toBeInTheDocument();
    expect(within(progress).getByText("This stage: 00:01")).toBeInTheDocument();

    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Plan reprocessing" }));
    await screen.findByRole("heading", { name: "Selected plan" });
    expect(ipc.callsTo("plan_meeting_reprocessing")).toEqual([
      {
        previous: importedReview,
        mode: "recluster",
        threshold: 0.34,
        saveCache: false,
        prompt: "Use the recorded speaker names",
        profileId: "interview-profile",
      },
    ]);
    expect(clockSelectedPlan.plan.context.previous_revision).toBe(importedReview.revision);
    expect(clockSelectedPlan.plan.revision).toBe("ad99e84a2dd7fb67818d32505a744a7738f33f078abf63a7161d30c01a067ca9");
    await user.click(screen.getByRole("button", { name: "Execute selected Recluster plan" }));

    expect(within(progress).getByRole("heading", { name: "Processing meeting" })).toBeInTheDocument();
    expect(within(progress).getByText("Elapsed: 00:00")).toBeInTheDocument();
    expect(within(progress).getByText("This stage: 00:00")).toBeInTheDocument();
    expect(within(progress).getByText("No status response yet — processing may still be running.")).toBeInTheDocument();
    expect(within(progress).queryByText("Status checked just now")).toBeNull();

    secondRunBegin.resolve("reprocessing-job");
    await waitForPollCount(ipc, 4);
    expect(ipc.callsTo("begin_meeting_reprocessing")).toHaveLength(1);
    expect(ipc.callsTo("begin_meeting_reprocessing")[0]).toMatchObject({
      filePath: clockSelectedPlan.file_path,
      request: {
        plan: clockSelectedPlan.plan,
        expected_revision: clockSelectedPlan.plan.revision,
        previous: importedReview,
        cache_path: clockSelectedPlan.cache_path,
      },
    });
    expect(within(progress).getByText("Elapsed: 00:00")).toBeInTheDocument();
    expect(within(progress).getByText("This stage: 00:00")).toBeInTheDocument();
    expect(within(progress).getByText("No status response yet — processing may still be running.")).toBeInTheDocument();

    delayedReprocessingPoll.resolve({
      id: "reprocessing-job",
      status: "running",
      phase: "decoding",
      elapsed_ms: 25_000,
      phase_elapsed_ms: 12_000,
      error: null,
      transcript: null,
    });
    await within(progress).findByText("Elapsed: 00:25");
    expect(within(progress).getByText("This stage: 00:12")).toBeInTheDocument();
    expect(within(progress).getByText("Status checked just now")).toBeInTheDocument();
    expect(within(progress).getByText("Decoding audio")).toBeInTheDocument();
    expect(within(progress).getByText("Reading the audio track.")).toBeInTheDocument();
  } finally {
    view?.unmount();
    delayedInitialPoll.resolve({
      id: "import-job",
      status: "running",
      phase: "analyzing",
      elapsed_ms: 4_000,
      phase_elapsed_ms: 2_000,
      error: null,
      transcript: null,
    });
    delayedInitialTerminalPoll.resolve({
      id: "import-job",
      status: "completed",
      phase: "finalizing",
      elapsed_ms: 9_000,
      phase_elapsed_ms: 1_000,
      error: null,
      transcript: importedTranscript,
    });
    secondRunBegin.resolve("reprocessing-job");
    delayedReprocessingPoll.resolve({
      id: "reprocessing-job",
      status: "completed",
      phase: "finalizing",
      elapsed_ms: 25_000,
      phase_elapsed_ms: 12_000,
      error: null,
      transcript: null,
    });
    await tick();
    performanceNow.mockRestore();
    vi.useRealTimers();
  }
});
