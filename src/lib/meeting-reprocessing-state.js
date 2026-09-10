/**
 * Return whether a proposal can be accepted.
 *
 * Automatic mappings intentionally leave their resolution entry null. The
 * preview status is the source of truth for both automatic and explicit
 * dispositions, and an empty correction list is already fully applied.
 *
 * @param {{ preview: { steps: Array<{ status: string }> } } | null} proposal
 */
export function proposalIsFullyApplied(proposal) {
  return proposal !== null && proposal.preview.steps.every((step) => step.status === "applied");
}

/**
 * @param {{ plan: { context: { previous_revision: string } } } | null} selected
 * @param {string | null | undefined} currentReviewRevision
 */
export function selectedPlanIsCurrent(selected, currentReviewRevision) {
  return selected !== null
    && typeof currentReviewRevision === "string"
    && selected.plan.context.previous_revision === currentReviewRevision;
}

/**
 * @param {{ proposal: { previous: { revision: string } } } | null} proposal
 * @param {string | null | undefined} currentReviewRevision
 */
export function proposalIsCurrent(proposal, currentReviewRevision) {
  return proposal !== null
    && typeof currentReviewRevision === "string"
    && proposal.proposal.previous.revision === currentReviewRevision;
}

/**
 * @param {{ proposal: { revision: string } } | null} proposal
 * @param {string | null} exportedRevision
 */
export function proposalWasExported(proposal, exportedRevision) {
  return proposal !== null
    && exportedRevision !== null
    && proposal.proposal.revision === exportedRevision;
}

/** @param {number} seconds */
function formatSegmentTimestamp(seconds) {
  const safeSeconds = Number.isFinite(seconds) ? Math.max(0, seconds) : 0;
  const minutes = Math.floor(safeSeconds / 60);
  const remainder = (safeSeconds % 60).toFixed(2).padStart(5, "0");
  return `${minutes}:${remainder}`;
}

/**
 * Filter candidate segments by text, id, or formatted start/end time.
 * The selected ids are always retained so filtering never hides a draft target.
 *
 * @param {Array<{ id: string, start: number, end: number, text: string, speaker: string }>} segments
 * @param {string} query
 * @param {string[]} selectedIds
 */
export function filterCandidateSegments(segments, query, selectedIds = []) {
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
