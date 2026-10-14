import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

// Source-text assertions over MeetingReprocessing.svelte, left for phase B of
// #361. The meeting-reprocessing-state tests moved to
// src/lib/meeting-reprocessing-state.test.ts (Vitest).

const source = await readFile(new URL("../src/lib/MeetingReprocessing.svelte", import.meta.url), "utf8");

test("execution uses the selected plan when plan controls have changed", () => {
  assert.match(source, /These controls set the next plan/);
  assert.match(source, /selected && \(mode !== selected\.plan\.mode \|\| threshold !== selected\.plan\.threshold\)/);
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
