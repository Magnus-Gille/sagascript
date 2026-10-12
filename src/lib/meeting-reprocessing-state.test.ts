// The module half of scripts/test-meeting-reprocessing.mjs (#361); the
// MeetingReprocessing.svelte source assertions stay there until phase B.
import { expect, test } from "vitest";
import type { MigrationStep } from "./meeting-reprocessing-types";
import {
  filterCandidateSegments,
  proposalIsCurrent,
  proposalIsFullyApplied,
  proposalWasExported,
  selectedPlanIsCurrent,
} from "./meeting-reprocessing-state";

function proposalWithStatuses(statuses: MigrationStep["status"][], resolutions: (unknown[] | null)[] = []) {
  return {
    preview: { steps: statuses.map((status) => ({ status })) },
    proposal: { resolutions },
  };
}

test("proposal acceptance allows empty and automatic dispositions but blocks conflicts", () => {
  expect(proposalIsFullyApplied(null)).toBe(false);
  expect(proposalIsFullyApplied(proposalWithStatuses([]))).toBe(true);
  expect(proposalIsFullyApplied(proposalWithStatuses(["applied"], [null]))).toBe(true);
  expect(proposalIsFullyApplied(proposalWithStatuses(["applied", "applied"], [null, []]))).toBe(true);
  expect(proposalIsFullyApplied(proposalWithStatuses(["applied", "conflict"], [null]))).toBe(false);
  expect(proposalIsFullyApplied(proposalWithStatuses(["applied", "blocked"], [null]))).toBe(false);
});

function selectedPlan(previousRevision: string) {
  return { plan: { context: { previous_revision: previousRevision } } };
}

function proposalWithPrevious(previousRevision: string, revision = "proposal-revision") {
  return { proposal: { previous: { revision: previousRevision }, revision } };
}

test("plans and proposals become stale when the current review revision changes", () => {
  expect(selectedPlanIsCurrent(selectedPlan("review-1"), "review-1")).toBe(true);
  expect(selectedPlanIsCurrent(selectedPlan("review-1"), "review-2")).toBe(false);
  expect(selectedPlanIsCurrent(selectedPlan("review-1"), null)).toBe(false);
  expect(proposalIsCurrent(proposalWithPrevious("review-1"), "review-1")).toBe(true);
  expect(proposalIsCurrent(proposalWithPrevious("review-1"), "review-2")).toBe(false);
  expect(proposalIsCurrent(proposalWithPrevious("review-1"), null)).toBe(false);
});

test("only a successful save for the current proposal revision counts as exported", () => {
  const proposal = proposalWithPrevious("review-1", "proposal-1");
  expect(proposalWasExported(proposal, null)).toBe(false);
  expect(proposalWasExported(proposal, "proposal-2")).toBe(false);
  expect(proposalWasExported(proposal, "proposal-1")).toBe(true);
  expect(proposalWasExported(null, "proposal-1")).toBe(false);
});

test("candidate filtering matches text, ids, and formatted time while retaining selected targets", () => {
  const segments = [
    { id: "seg-a", start: 0, end: 5.25, text: "Opening remarks" },
    { id: "seg-b", start: 754, end: 760, text: "Budget discussion" },
    { id: "seg-c", start: 1200, end: 1205, text: "Closing remarks" },
  ];
  expect(filterCandidateSegments(segments, "budget").map((segment) => segment.id)).toStrictEqual(["seg-b"]);
  expect(filterCandidateSegments(segments, "12:34").map((segment) => segment.id)).toStrictEqual(["seg-b"]);
  expect(filterCandidateSegments(segments, "seg-c").map((segment) => segment.id)).toStrictEqual(["seg-c"]);
  expect(filterCandidateSegments(segments, "missing", ["seg-a"]).map((segment) => segment.id)).toStrictEqual(["seg-a"]);
  expect(filterCandidateSegments(segments, "").map((segment) => segment.id)).toStrictEqual(["seg-a", "seg-b", "seg-c"]);
});
