import type { MeetingReview, MeetingReviewState } from "./meeting-types";
import type { ProposalState, SelectedReprocessingPlan } from "./meeting-reprocessing-types";

interface MeetingExecutionState {
  otherBusy: boolean;
  transcribing: boolean;
  reprocessingBusy: boolean;
  draftDirty: boolean;
  plan: SelectedReprocessingPlan | null;
  review: MeetingReview | null;
}

interface ExecuteMeetingPlanOptions {
  read: () => MeetingExecutionState;
  waitForActions: () => Promise<void>;
  execute: (selected: SelectedReprocessingPlan, review: MeetingReview) => Promise<void>;
}

function canExecuteMeetingPlan(state: MeetingExecutionState): boolean {
  return !state.otherBusy && !state.transcribing && !state.reprocessingBusy && !state.draftDirty;
}

export async function executeMeetingPlan({ read, waitForActions, execute }: ExecuteMeetingPlanOptions): Promise<void> {
  const beforeWait = read();
  if (!canExecuteMeetingPlan(beforeWait) || !beforeWait.plan) return;

  await waitForActions();

  const selectedState = read();
  const selected = selectedState.plan;
  const review = selectedState.review;
  if (!review || !selected || !canExecuteMeetingPlan(selectedState)) return;
  if (selected.plan.context.previous_revision !== review.revision) {
    throw new Error("The review changed. Prepare a new plan; your corrections have been kept.");
  }

  // Invoke the state transition in this continuation, without an intervening await.
  await execute(selected, review);
}

interface MeetingProposalAcceptanceState {
  documentRevision: number;
  pollGeneration: number;
  draftDirty: boolean;
  proposal: ProposalState | null;
}

interface AcceptMeetingProposalOptions {
  revision: number;
  review: MeetingReview;
  read: () => MeetingProposalAcceptanceState;
  accept: (proposal: ProposalState, review: MeetingReview) => Promise<MeetingReviewState>;
  apply: (state: MeetingReviewState, generation: number) => void;
}

export async function acceptMeetingProposalForCurrentReview({
  revision,
  review,
  read,
  accept,
  apply,
}: AcceptMeetingProposalOptions): Promise<boolean> {
  const beforeAccept = read();
  const proposal = beforeAccept.proposal;
  const generation = beforeAccept.pollGeneration;
  if (!proposal || beforeAccept.draftDirty) return false;

  const state = await accept(proposal, review);
  const current = read();
  if (
    revision !== current.documentRevision ||
    generation !== current.pollGeneration ||
    current.draftDirty ||
    proposal.proposal.revision !== current.proposal?.proposal.revision
  ) {
    throw new Error("The review changed before acceptance. It has not been replaced.");
  }

  // Apply immediately after validating the captured proposal and review generation.
  apply(state, generation);
  return true;
}
