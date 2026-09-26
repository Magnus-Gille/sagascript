import assert from "node:assert/strict";
import test from "node:test";
import ts from "typescript";
import { readFile } from "node:fs/promises";

const source = await readFile(new URL("../src/lib/update-recovery.ts", import.meta.url), "utf8");
const moduleSource = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 },
}).outputText;
const recovery = await import(`data:text/javascript;base64,${Buffer.from(moduleSource).toString("base64")}`);

const transcript = {
  schema_version: 1,
  source_sha256: "source-hash",
  language: "sv",
  model: "pianissimo-large",
  duration_seconds: 12.5,
  segments: [{ id: "seg-1", start: 0, end: 2, text: "Hej", speaker: "speaker-1" }],
  speakers: [{ id: "speaker-1", label: "Talare 1" }],
};

function completePayload() {
  return recovery.createUpdateRecoveryPayload({
    dictation: { text: "En osparad diktering" },
    file: { path: "/tmp/interview.wav", text: "En osparad filtranskribering" },
    meeting: {
      review: {
        review: {
          schema_version: 1,
          original: transcript,
          original_revision: "original-1",
          generation: 2,
          batches: [{ operations: [{ kind: "edit_segment", segment_id: "seg-1", text: "Hej där" }] }],
          revision: "review-1",
        },
        transcript,
      },
      editor_draft: {
        labels: { "speaker-1": "Magnus" },
        mergeTargets: {},
        texts: { "seg-1": "Hej där" },
        speakers: { "seg-1": "speaker-1" },
      },
      proposal: {
        proposal: {
          schema_version: 1,
          previous: {
            schema_version: 1,
            original: transcript,
            original_revision: "original-1",
            generation: 2,
            batches: [],
            revision: "review-1",
          },
          proposed: transcript,
          generation: 3,
          resolutions: [null],
          revision: "proposal-1",
        },
        preview: {
          candidate: {
            schema_version: 1,
            original: transcript,
            original_revision: "original-1",
            generation: 2,
            batches: [],
            revision: "candidate-1",
          },
          steps: [{
            original: { kind: "edit_segment", segment_id: "seg-1", text: "Hej" },
            mapped: null,
            status: "applied",
          }],
        },
        candidate: transcript,
      },
    },
  }, "2026-09-26T10:00:00.000Z");
}

test("payload round trips all unsaved result types", () => {
  const original = completePayload();
  const serialized = recovery.serializeUpdateRecoveryPayload(original);
  const parsed = recovery.parseUpdateRecoveryPayload(serialized);
  assert.deepEqual(parsed, original);
  assert.equal(parsed.meeting.editor_draft.texts["seg-1"], "Hej där");
  assert.equal(parsed.meeting.proposal.proposal.revision, "proposal-1");
});

test("untrusted input is normalized, bounded, and stripped to known fields", () => {
  const parsed = recovery.parseUpdateRecoveryPayload(JSON.stringify({
    schema_version: 1,
    saved_at: "saved",
    dictation: { text: "x".repeat(600_000), audio_bytes: "must be discarded" },
    file: { path: "/tmp/a", text: "file", raw_audio: [1, 2, 3] },
    meeting: null,
    unexpected: { token: "discard" },
  }));
  assert.equal(parsed.dictation.text.length, recovery.UPDATE_RECOVERY_LIMITS.maxTextLength);
  assert.deepEqual(parsed.file, { path: "/tmp/a", text: "file" });
  assert.equal(Object.hasOwn(parsed, "unexpected"), false);
  assert.equal(Object.hasOwn(parsed.dictation, "audio_bytes"), false);
});

test("invalid JSON, schema versions, and malformed nested results fail closed", () => {
  assert.equal(recovery.parseUpdateRecoveryPayload("not-json"), null);
  assert.equal(recovery.parseUpdateRecoveryPayload(JSON.stringify({ schema_version: 99 })), null);
  const parsed = recovery.parseUpdateRecoveryPayload(JSON.stringify({
    schema_version: 1,
    saved_at: "saved",
    dictation: { text: "kept" },
    file: { path: 42, text: "discarded" },
    meeting: { review: null, editor_draft: {} },
  }));
  assert.deepEqual(parsed.file, null);
  assert.deepEqual(parsed.meeting, null);
  assert.deepEqual(parsed.dictation, { text: "kept" });
});

test("oversized serialized payloads are rejected", () => {
  const payload = completePayload();
  payload.dictation.text = "x".repeat(recovery.UPDATE_RECOVERY_LIMITS.maxTextLength);
  assert.doesNotThrow(() => recovery.serializeUpdateRecoveryPayload(payload));
  assert.equal(
    recovery.parseUpdateRecoveryPayload("x".repeat(recovery.UPDATE_RECOVERY_LIMITS.maxPayloadBytes + 1)),
    null,
  );
});
