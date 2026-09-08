import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { proposalIsFullyApplied } from "../src/lib/meeting-reprocessing-state.js";

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

test("save cancellation does not claim success and dirty drafts block mutations", () => {
  assert.match(source, /onSave: \(\) => Promise<boolean>/);
  assert.match(source, /const saved = await runAction\("save", onSave\);/);
  assert.match(source, /if \(saved\) notice = "Reprocessing proposal saved\.";/);
  assert.match(source, /disabled=\{isBusy\(\) \|\| draftDirty\} onclick=\{\(\) => void plan\(\)\}/);
  assert.match(source, /disabled=\{isBusy\(\) \|\| draftDirty\} onclick=\{\(\) => void execute\(\)\}/);
  assert.match(source, /disabled=\{isBusy\(\) \|\| draftDirty \|\| !canAccept\(\)\}/);
  assert.match(source, /disabled=\{isBusy\(\) \|\| draftDirty \|\| !validResolution\(index, step\.original\)\}/);
  assert.match(source, /if \(draftDirty \|\| !selected\) return;/);
  assert.match(source, /if \(draftDirty \|\| !canAccept\(\)\) return;/);
  assert.match(source, /return proposalIsFullyApplied\(proposal\);/);
});
