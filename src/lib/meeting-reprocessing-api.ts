import { invoke } from "@tauri-apps/api/core";
import type { CorrectionOperation, MeetingReview, MeetingReviewState } from "./meeting-types";
import type {
  MeetingReprocessingProposal, ProposalState, ReprocessingMode, SelectedReprocessingPlan,
} from "./meeting-reprocessing-types";

export function planMeetingReprocessing(
  previous: MeetingReview, mode: ReprocessingMode, threshold: number,
  saveCache: boolean, prompt: string | null, profileId: string | null,
): Promise<SelectedReprocessingPlan | null> {
  return invoke("plan_meeting_reprocessing", { previous, mode, threshold, saveCache, prompt, profileId });
}
export function beginMeetingReprocessing(
  selected: SelectedReprocessingPlan, previous: MeetingReview,
  prompt: string | null, profileId: string | null,
): Promise<string> {
  return invoke("begin_meeting_reprocessing", {
    filePath: selected.file_path, prompt, profileId,
    request: {
      plan: selected.plan, expected_revision: selected.plan.revision, previous,
      cache_path: selected.cache_path, cache_output: selected.cache_output,
    },
  });
}
export function previewMeetingProposal(proposal: MeetingReprocessingProposal): Promise<ProposalState> {
  return invoke("preview_meeting_proposal", { proposal });
}
export function resolveMeetingProposal(
  proposal: MeetingReprocessingProposal, index: number, operations: CorrectionOperation[],
): Promise<ProposalState> {
  return invoke("resolve_meeting_proposal", { proposal, expectedRevision: proposal.revision, index, operations });
}
export function acceptMeetingProposal(
  proposal: MeetingReprocessingProposal, currentReview: MeetingReview,
): Promise<MeetingReviewState> {
  return invoke("accept_meeting_proposal", { proposal, currentReview });
}
export function saveMeetingProposal(proposal: MeetingReprocessingProposal): Promise<boolean> {
  return invoke("save_meeting_proposal", { proposal });
}
export function openMeetingProposal(): Promise<ProposalState | null> {
  return invoke("open_meeting_proposal");
}
