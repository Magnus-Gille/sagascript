import type { MeetingReview, MeetingSegment } from "./meeting-types";
import type { MeetingReprocessingProposal, MigrationStep, ReprocessingPlan } from "./meeting-reprocessing-types";

// The parts of a ProposalState / SelectedReprocessingPlan each check reads;
// the full types satisfy them.
type ProposalPreview = { preview: { steps: readonly Pick<MigrationStep, "status">[] } };
type PlanContext = { plan: { context: Pick<ReprocessingPlan["context"], "previous_revision"> } };
type ProposalBase = { proposal: { previous: Pick<MeetingReview, "revision"> } };
type ProposalRevision = { proposal: Pick<MeetingReprocessingProposal, "revision"> };
type CandidateSegment = Pick<MeetingSegment, "id" | "start" | "end" | "text">;

/**
 * Return whether a proposal can be accepted.
 *
 * Automatic mappings intentionally leave their resolution entry null. The
 * preview status is the source of truth for both automatic and explicit
 * dispositions, and an empty correction list is already fully applied.
 */
export function proposalIsFullyApplied(proposal: ProposalPreview | null): boolean {
  return proposal !== null && proposal.preview.steps.every((step) => step.status === "applied");
}

export function selectedPlanIsCurrent(
  selected: PlanContext | null,
  currentReviewRevision: string | null | undefined,
): boolean {
  return selected !== null
    && typeof currentReviewRevision === "string"
    && selected.plan.context.previous_revision === currentReviewRevision;
}

export function proposalIsCurrent(
  proposal: ProposalBase | null,
  currentReviewRevision: string | null | undefined,
): boolean {
  return proposal !== null
    && typeof currentReviewRevision === "string"
    && proposal.proposal.previous.revision === currentReviewRevision;
}

export function proposalWasExported(
  proposal: ProposalRevision | null,
  exportedRevision: string | null,
): boolean {
  return proposal !== null
    && exportedRevision !== null
    && proposal.proposal.revision === exportedRevision;
}

function formatSegmentTimestamp(seconds: number): string {
  const safeSeconds = Number.isFinite(seconds) ? Math.max(0, seconds) : 0;
  const minutes = Math.floor(safeSeconds / 60);
  const remainder = (safeSeconds % 60).toFixed(2).padStart(5, "0");
  return `${minutes}:${remainder}`;
}

/**
 * Filter candidate segments by text, id, or formatted start/end time.
 * The selected ids are always retained so filtering never hides a draft target.
 */
export function filterCandidateSegments<Segment extends CandidateSegment>(
  segments: Segment[],
  query: string,
  selectedIds: readonly string[] = [],
): Segment[] {
  const normalizedQuery = query.trim().toLowerCase();
  if (!normalizedQuery) return segments;
  const selected = new Set(selectedIds);

  return segments.filter((segment) => {
    if (selected.has(segment.id)) return true;
    const start = formatSegmentTimestamp(segment.start);
    const end = formatSegmentTimestamp(segment.end);
    const searchText = [
      segment.id,
      segment.text,
      start,
      end,
      `${start}-${end}`,
      `${start}–${end}`,
    ].join(" ").toLowerCase();
    return searchText.includes(normalizedQuery);
  });
}
