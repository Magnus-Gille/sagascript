import { expect, test, vi } from "vitest";
import meetingJobFixtures from "../../contracts/transport/get_meeting_job.json";
import { deferred } from "../test/deferred";
import type { MeetingReview, MeetingReviewState } from "./meeting-types";
import type { ProposalState, ReprocessingResult, SelectedReprocessingPlan } from "./meeting-reprocessing-types";
import { acceptMeetingProposalForCurrentReview, executeMeetingPlan } from "./meeting-reprocessing-flow";

const reprocessing = meetingJobFixtures.reprocessing.reprocessing as unknown as ReprocessingResult;
const review = reprocessing.proposal.previous;
const proposal: ProposalState = {
  proposal: reprocessing.proposal,
  preview: { candidate: reprocessing.proposal.previous, steps: [] },
  candidate: reprocessing.proposal.proposed,
};
const plan: SelectedReprocessingPlan = {
  file_path: "/recordings/meeting.wav",
  cache_path: null,
  cache_output: null,
  plan: {
    schema_version: 1,
    mode: "recluster",
    context: {
      source_sha256: review.original.source_sha256,
      previous_revision: review.revision,
      transcription_context_sha256: "c".repeat(64),
      analysis_context_sha256: "d".repeat(64),
      cache_sha256: null,
    },
    threshold: 0.34,
    required_work: reprocessing.required_work,
    revision: "selected-plan-revision",
  },
};

test("executeMeetingPlan waits, rereads, validates, and executes captured references", async () => {
  const queue = deferred<void>();
  const execute = vi.fn(async (selected: SelectedReprocessingPlan, currentReview: MeetingReview) => {
    expect(selected.plan.context).toBe(plan.plan.context);
    expect(currentReview).toBe(review);
  });
  const read = vi.fn(() => ({
    otherBusy: false,
    transcribing: false,
    reprocessingBusy: false,
    draftDirty: false,
    plan,
    review,
  }));

  const pending = executeMeetingPlan({ read, waitForActions: () => queue.promise, execute });
  expect(execute).not.toHaveBeenCalled();
  queue.resolve();
  await pending;

  expect(read).toHaveBeenCalledTimes(2);
  expect(execute).toHaveBeenCalledTimes(1);
  expect(execute.mock.calls[0][0]).toBe(plan);
  expect(execute.mock.calls[0][1]).toBe(review);
});

test("executeMeetingPlan rejects a plan bound to a different review", async () => {
  const invalidPlan = {
    ...plan,
    plan: { ...plan.plan, context: { ...plan.plan.context, previous_revision: "stale-review" } },
  };
  const execute = vi.fn(async () => undefined);

  await expect(
    executeMeetingPlan({
      read: () => ({
        otherBusy: false,
        transcribing: false,
        reprocessingBusy: false,
        draftDirty: false,
        plan: invalidPlan,
        review,
      }),
      waitForActions: async () => undefined,
      execute,
    }),
  ).rejects.toThrow("The review changed. Prepare a new plan; your corrections have been kept.");
  expect(execute).not.toHaveBeenCalled();
});

for (const [label, busyState] of [
  ["draft dirty", { draftDirty: true }],
  ["reprocessing busy", { reprocessingBusy: true }],
] as const) {
  test(`executeMeetingPlan blocks when ${label} changes while waiting`, async () => {
    const queue = deferred<void>();
    const execute = vi.fn(async () => undefined);
    let state = {
      otherBusy: false,
      transcribing: false,
      reprocessingBusy: false,
      draftDirty: false,
      plan,
      review,
    };
    const pending = executeMeetingPlan({ read: () => state, waitForActions: () => queue.promise, execute });
    state = { ...state, ...busyState };
    queue.resolve();
    await pending;

    expect(execute).not.toHaveBeenCalled();
  });
}

for (const [label, invalidate] of [
  [
    "document revision",
    (state: { documentRevision: number; pollGeneration: number }) => {
      state.documentRevision += 1;
    },
  ],
  [
    "poll generation",
    (state: { documentRevision: number; pollGeneration: number }) => {
      state.pollGeneration += 1;
    },
  ],
] as const) {
  test(`acceptMeetingProposalForCurrentReview rejects a changed ${label} and never applies`, async () => {
    const response = deferred<MeetingReviewState>();
    const state = { documentRevision: 4, pollGeneration: 9, draftDirty: false, proposal };
    const accept = vi.fn(() => response.promise);
    const apply = vi.fn();
    const pending = acceptMeetingProposalForCurrentReview({
      revision: state.documentRevision,
      review,
      read: () => state,
      accept,
      apply,
    });

    expect(accept).toHaveBeenCalledWith(proposal, review);
    invalidate(state);
    response.resolve({ review, transcript: review.original });
    await expect(pending).rejects.toThrow("The review changed before acceptance. It has not been replaced.");
    expect(apply).not.toHaveBeenCalled();
  });
}

test("acceptMeetingProposalForCurrentReview applies the captured generation after acceptance", async () => {
  const acceptedState = { review, transcript: review.original };
  const state = { documentRevision: 4, pollGeneration: 9, draftDirty: false, proposal };
  const accept = vi.fn(async () => acceptedState);
  const apply = vi.fn();

  await expect(
    acceptMeetingProposalForCurrentReview({
      revision: state.documentRevision,
      review,
      read: () => state,
      accept,
      apply,
    }),
  ).resolves.toBe(true);

  expect(accept).toHaveBeenCalledWith(proposal, review);
  expect(apply).toHaveBeenCalledWith(acceptedState, state.pollGeneration);
});
