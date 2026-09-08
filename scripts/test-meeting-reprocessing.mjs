import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import {
  filterCandidateSegments,
  proposalIsCurrent,
  proposalIsFullyApplied,
  proposalWasExported,
  selectedPlanIsCurrent,
} from "../src/lib/meeting-reprocessing-state.js";

const source = await readFile(
  new URL("../src/lib/MeetingReprocessing.svelte", import.meta.url),
  "utf8",
);

test("execution uses the selected plan when plan controls have changed", () => {
  assert.match(source, /These controls set the next plan/);
  assert.match(
    source,
    /selected && \(mode !== selected\.plan\.mode \|\| threshold !== selected\.plan\.threshold\)/,
  );
  assert.match(source, /This is the plan that Execute will run/);
  assert.match(source, /selected\.plan\.mode\].label/);
  assert.match(source, /selected\.plan\.threshold\.toFixed\(2\)/);
  assert.match(source, /`Execute selected \$\{modeDescriptions\[selected\.plan\.mode\]\.label\} plan`/);
});

test("proposal conflicts stay explicit, ordered, and nonempty", () => {
  assert.match(source, /function firstConflictIndex\(\): number/);
  assert.match(source, /index === firstConflictIndex\(\)/);
  assert.match(source, /Resolve the first conflict before this step can be changed/);
  assert.match(source, /No correction is silently dropped/);
  assert.match(source, /function validResolution\(index: number, original: CorrectionOperation\): boolean/);
  assert.match(source, /if \(rows\.length === 0\) return false/);
  assert.match(source, /row\.text !== undefined \|\| row\.speaker_id !== undefined/);
  assert.match(source, /Add another candidate target/);
  assert.match(source, /Mapped disposition/);
});

function proposalWithStatuses(statuses, resolutions = []) {
  return {
    preview: { steps: statuses.map((status) => ({ status })) },
    proposal: { resolutions },
  };
}

test("proposal acceptance allows empty and automatic dispositions but blocks conflicts", () => {
  assert.equal(proposalIsFullyApplied(null), false);
  assert.equal(proposalIsFullyApplied(proposalWithStatuses([])), true);
  assert.equal(
    proposalIsFullyApplied(proposalWithStatuses(["applied"], [null])),
    true,
  );
  assert.equal(
    proposalIsFullyApplied(proposalWithStatuses(["applied", "applied"], [null, []])),
    true,
  );
  assert.equal(
    proposalIsFullyApplied(proposalWithStatuses(["applied", "conflict"], [null])),
    false,
  );
  assert.equal(
    proposalIsFullyApplied(proposalWithStatuses(["applied", "blocked"], [null])),
    false,
  );
});

function selectedPlan(previousRevision) {
  return { plan: { context: { previous_revision: previousRevision } } };
}

function proposalWithPrevious(previousRevision, revision = "proposal-revision") {
  return { proposal: { previous: { revision: previousRevision }, revision } };
}

test("plans and proposals become stale when the current review revision changes", () => {
  assert.equal(selectedPlanIsCurrent(selectedPlan("review-1"), "review-1"), true);
  assert.equal(selectedPlanIsCurrent(selectedPlan("review-1"), "review-2"), false);
  assert.equal(selectedPlanIsCurrent(selectedPlan("review-1"), null), false);
  assert.equal(proposalIsCurrent(proposalWithPrevious("review-1"), "review-1"), true);
  assert.equal(proposalIsCurrent(proposalWithPrevious("review-1"), "review-2"), false);
  assert.equal(proposalIsCurrent(proposalWithPrevious("review-1"), null), false);
});

test("only a successful save for the current proposal revision counts as exported", () => {
  const proposal = proposalWithPrevious("review-1", "proposal-1");
  assert.equal(proposalWasExported(proposal, null), false);
  assert.equal(proposalWasExported(proposal, "proposal-2"), false);
  assert.equal(proposalWasExported(proposal, "proposal-1"), true);
  assert.equal(proposalWasExported(null, "proposal-1"), false);
});

test("candidate filtering matches text, ids, and formatted time while retaining selected targets", () => {
  const segments = [
    { id: "seg-a", start: 0, end: 5.25, text: "Opening remarks" },
    { id: "seg-b", start: 754, end: 760, text: "Budget discussion" },
    { id: "seg-c", start: 1200, end: 1205, text: "Closing remarks" },
  ];
  assert.deepEqual(filterCandidateSegments(segments, "budget").map((segment) => segment.id), ["seg-b"]);
  assert.deepEqual(filterCandidateSegments(segments, "12:34").map((segment) => segment.id), ["seg-b"]);
  assert.deepEqual(filterCandidateSegments(segments, "seg-c").map((segment) => segment.id), ["seg-c"]);
  assert.deepEqual(filterCandidateSegments(segments, "missing", ["seg-a"]).map((segment) => segment.id), ["seg-a"]);
  assert.deepEqual(filterCandidateSegments(segments, "").map((segment) => segment.id), ["seg-a", "seg-b", "seg-c"]);
});

test("replacement and acceptance warn before discarding or replacing proposal state", () => {
  assert.match(source, /Accepting this proposal replaces the current review\. Save the proposal first/);
  assert.match(source, /Open a saved proposal and discard the current proposal resolutions/);
  assert.match(source, /Discard this reprocessing proposal and all of its resolutions/);
  assert.match(source, /proposalIsCurrent\(proposal, currentReviewRevision\)/);
});

test("save cancellation does not claim success and dirty drafts block mutations", () => {
  assert.match(source, /onSave: \(\) => Promise<boolean>/);
  assert.match(source, /const saved = await runAction\("save", onSave\);/);
  assert.match(source, /if \(saved && revision !== null && proposal\?\.proposal\.revision === revision\)/);
  assert.match(source, /notice = "Reprocessing proposal saved\.";/);
  assert.match(source, /disabled=\{isBusy\(\) \|\| draftDirty\} onclick=\{\(\) => void plan\(\)\}/);
  assert.match(source, /disabled=\{isBusy\(\) \|\| draftDirty \|\| !selectedPlanIsFresh\(\)\} onclick=\{\(\) => void execute\(\)\}/);
  assert.match(source, /disabled=\{isBusy\(\) \|\| draftDirty \|\| !canAccept\(\)\}/);
  assert.match(source, /disabled=\{isBusy\(\) \|\| draftDirty \|\| !validResolution\(index, step\.original\)\}/);
  assert.match(source, /if \(draftDirty \|\| !selected \|\| !selectedPlanIsCurrent\(selected, currentReviewRevision\)\) return;/);
  assert.match(source, /if \(draftDirty \|\| !canAccept\(\)\) return;/);
  assert.match(source, /return proposalIsFullyApplied\(proposal\);/);
});
