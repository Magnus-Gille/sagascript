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
