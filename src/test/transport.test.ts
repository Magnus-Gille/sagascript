// The Rust–TypeScript transport contract (#347, contracts/README.md).
//
// Rust writes the shared fixtures in contracts/transport/ from real payloads
// (`cargo test contract` in src-tauri/ fails while they are stale). This suite
// proves each schema equal to its frontend type, parses every fixture with it,
// and feeds the payloads to the modules that consume them. A renamed field,
// changed type, enum value or event name on either side fails here or in
// svelte-check.
import { describe, expect, expectTypeOf, test } from "vitest";
import type { MeetingJobSnapshot, Settings } from "../lib/api";
import type { EventMap, EventName } from "../lib/events";
import { pollMeetingJob } from "../lib/meeting-job-client";
import { meetingPhases } from "../lib/meeting-progress";
import { initialMeetingDrafts, reconcileMeetingDrafts, type CorrectionOperation } from "../lib/meeting-types";
import { initialOverlayState, reduceOverlay, type OverlayState } from "../lib/overlay-state";
import { acceptRunProgress, startStages, type StageState } from "../lib/transcribe-stages";
import eventsFixture from "../../contracts/transport/events.json";
import meetingJobFixture from "../../contracts/transport/get_meeting_job.json";
import settingsFixture from "../../contracts/transport/get_settings.json";
import { ContractError, type Infer, type Schema } from "./schema";
import * as schemas from "./transport-schemas";

type EventPayloadSchemas = typeof schemas.eventPayloads;

/**
 * Parses every `{ variant: payload }` entry of a fixture on first use, so a
 * mismatch fails each test that needs the payloads (with the JSON path)
 * rather than the whole file.
 */
function variantsOf<T>(fixture: Record<string, unknown>, schema: Schema<T>, name: string): () => Map<string, T> {
  let parsed: Map<string, T> | undefined;
  return () =>
    (parsed ??= new Map(Object.entries(fixture).map(([variant, payload]) => [variant, schema.parse(payload, `${name}:${variant}`)])));
}

test("each schema is exactly the frontend type it mirrors", () => {
  // Type-level (svelte-check): renaming, adding or retyping a field in the
  // TypeScript type without the schema, or the reverse, is an error here.
  expectTypeOf<Infer<typeof schemas.settings>>().toEqualTypeOf<Settings>();
  expectTypeOf<Infer<typeof schemas.meetingJobSnapshot>>().toEqualTypeOf<MeetingJobSnapshot>();
  expectTypeOf<{ [E in keyof EventPayloadSchemas]: Infer<EventPayloadSchemas[E]> }>().toEqualTypeOf<EventMap>();
});

test("the suite reads every shared fixture", () => {
  const onDisk = Object.keys(import.meta.glob("../../contracts/transport/*.json")).map((path) => path.split("/").at(-1));
  expect(onDisk.sort()).toEqual(["events.json", "get_meeting_job.json", "get_settings.json"]);
});

describe("get_settings", () => {
  const fixture: Record<string, unknown> = settingsFixture;
  const variants = variantsOf(fixture, schemas.settings, "get_settings.json");

  test.each(Object.keys(fixture))("%s parses as Settings", (variant) => {
    schemas.settings.parse(fixture[variant], `get_settings.json:${variant}`);
  });

  test("the fixtures cover optional and per-profile fields", () => {
    expect([...variants().keys()].sort()).toEqual(["customised", "default"]);
    const customised = variants().get("customised")!;
    const [withShortcuts, withoutShortcuts] = customised.hotkey_profiles;
    expect(withShortcuts.push_to_talk_shortcut).toBeTypeOf("string");
    expect(withShortcuts.toggle_shortcut).toBeTypeOf("string");
    expect(withoutShortcuts).not.toHaveProperty("push_to_talk_shortcut");
    expect(withoutShortcuts).not.toHaveProperty("toggle_shortcut");
    expect(Object.keys(customised.profile_models).length).toBeGreaterThan(0);
    // A fresh install's implicit legacy profile arrives explicit.
    expect(variants().get("default")!.hotkey_profiles).toHaveLength(1);
  });
});

describe("get_meeting_job", () => {
  const fixture: Record<string, unknown> = meetingJobFixture;
  const variants = variantsOf(fixture, schemas.meetingJobSnapshot, "get_meeting_job.json");
  const snapshots = () => [...variants().values()];

  test.each(Object.keys(fixture))("%s parses as MeetingJobSnapshot", (variant) => {
    schemas.meetingJobSnapshot.parse(fixture[variant], `get_meeting_job.json:${variant}`);
  });

  test("the fixtures cover every job status", () => {
    expect(new Set(snapshots().map((snapshot) => snapshot.status))).toEqual(
      new Set(["running", "cancelling", "completed", "cancelled", "failed"]),
    );
  });

  test("the fixtures cover nullable fields both ways and the feature-gated reprocessing result", () => {
    const completed = variants().get("completed")!;
    expect(completed.transcript?.diarization).toBeTruthy();
    expect(completed.transcript?.speaker_hint).toEqual({ exact: 2 });
    expect(variants().get("completed_plain")!.transcript).not.toHaveProperty("diarization");
    expect(variants().get("failed")!.error).toBeTypeOf("string");
    expect(variants().get("cancelled")!.error).toBeNull();
    const reprocessing = variants().get("reprocessing")!;
    expect(reprocessing.status).toBe("completed");
    expect(reprocessing.transcript).toBeNull();
    expect(reprocessing.reprocessing?.proposal.resolutions).toContainEqual(null);
    expect(snapshots().filter((snapshot) => snapshot.reprocessing === null).length).toBeGreaterThan(0);
  });

  test("every phase Rust reports has a label", () => {
    for (const snapshot of snapshots()) expect(meetingPhases).toContain(snapshot.phase);
  });

  test("polling stops exactly at the terminal statuses", async () => {
    for (const [variant, snapshot] of variants()) {
      let waits = 0;
      let polls = 0;
      await pollMeetingJob({
        jobId: snapshot.id,
        get: async () => {
          polls += 1;
          return snapshot;
        },
        isCurrent: () => polls < 2,
        onSnapshot: () => undefined,
        onFailure: (error) => expect.unreachable(`${variant}: ${String(error)}`),
        wait: async () => {
          waits += 1;
        },
      });
      const terminal = ["completed", "cancelled", "failed"].includes(snapshot.status);
      expect({ variant, waits }).toEqual({ variant, waits: terminal ? 0 : 1 });
    }
  });

  test("a correction field Rust sends as null is not part of the edit", () => {
    // Regression (#347): Rust serializes an unchanged EditSegment field as
    // null, and reconcileMeetingDrafts treated null as "edited".
    const proposal = variants().get("reprocessing")!.reprocessing!.proposal;
    const operations: CorrectionOperation[] = proposal.previous.batches.at(-1)!.operations;
    const edit = operations.find((operation) => operation.kind === "edit_segment")!;
    expect(edit).toMatchObject({ speaker_id: null });
    const before = proposal.previous.original;
    const after = {
      ...before,
      segments: before.segments.map((segment) => (segment.id === edit.segment_id ? { ...segment, text: edit.text! } : segment)),
    };
    const drafts = initialMeetingDrafts(before);
    drafts.speakers[edit.segment_id] = "an unsaved speaker choice";
    const reconciled = reconcileMeetingDrafts(before, after, drafts, [edit]);
    expect(reconciled.texts[edit.segment_id]).toBe(edit.text);
    expect(reconciled.speakers[edit.segment_id]).toBe("an unsaved speaker choice");
  });
});

describe("events", () => {
  const catalogue = eventsFixture as Record<string, { consumer: string; reason?: string; scope?: string; samples: unknown[] }>;
  const listened = Object.keys(schemas.eventPayloads).sort();

  test("the frontend listens for exactly the events Rust marks as GUI events", () => {
    const gui = Object.entries(catalogue)
      .filter(([, entry]) => entry.consumer === "gui")
      .map(([name]) => name)
      .sort();
    expect(listened).toEqual(gui);
    // Type level: every EventMap key is an event name in the Rust catalogue.
    expectTypeOf<EventName>().toExtend<keyof typeof eventsFixture>();
  });

  test("internal events are named legacy exceptions with a reason", () => {
    const internal = Object.entries(catalogue).filter(([, entry]) => entry.consumer !== "gui");
    for (const [name, entry] of internal) {
      expect({ name, consumer: entry.consumer }).toEqual({ name, consumer: "internal" });
      expect(entry.reason, name).toMatch(/\S/);
    }
  });

  test.each(listened)("every %s sample parses as its EventMap payload", (name) => {
    const entry = catalogue[name];
    const schema: Schema<unknown> = schemas.eventPayloads[name as EventName];
    expect(entry.samples.length).toBeGreaterThan(0);
    entry.samples.forEach((sample, index) => schema.parse(sample, `events.json:${name}[${index}]`));
  });

  test("plain-transcription progress drives the stages and stays scoped to its run", () => {
    const samples = catalogue["plain-transcription-progress"].samples.map((sample) =>
      schemas.eventPayloads["plain-transcription-progress"].parse(sample),
    );
    const runId = samples[0].runId;
    let state: StageState = startStages();
    for (const sample of samples.filter((event) => event.runId === runId)) {
      const next = acceptRunProgress(state, runId, sample);
      expect(next.index).toBeGreaterThanOrEqual(state.index);
      state = next;
    }
    expect(state).toMatchObject({ status: "running", index: 2, phase: "finalizing" });
    const foreign = samples.find((event) => event.runId !== runId)!;
    expect(acceptRunProgress(state, runId, foreign)).toBe(state);
  });

  test("the overlay understands every dictation state and engine load state", () => {
    const states = catalogue["state-changed"].samples.map((sample) => schemas.eventPayloads["state-changed"].parse(sample));
    for (const value of states) {
      const next = reduceOverlay(initialOverlayState, { type: "state", value });
      // settings_reloaded is not a dictation state; the overlay ignores it.
      expect(next.phase).toBe(value === "settings_reloaded" ? initialOverlayState.phase : value);
    }
    const loads = catalogue["engine-load-state"].samples.map((sample) => schemas.eventPayloads["engine-load-state"].parse(sample));
    for (const payload of loads) {
      const working: OverlayState = {
        ...initialOverlayState,
        phase: "transcribing",
        engine: payload.state === "loading" ? "ready" : "loading",
      };
      const next = reduceOverlay(working, { type: "engine", value: payload.state, source: payload.source });
      // A background warm-up failure is not this dictation's error.
      expect(next.engine).toBe(payload.state === "failed" && payload.source === "warm" ? "unknown" : payload.state);
    }
  });

  test("scoped events carry their scope field", () => {
    for (const [name, entry] of Object.entries(catalogue)) {
      if (!entry.scope) continue;
      for (const sample of entry.samples) expect(sample, name).toHaveProperty(entry.scope);
    }
  });
});

test("a schema names the path of the first mismatch", () => {
  const sample = structuredClone(meetingJobFixture.completed) as Record<string, unknown>;
  sample.state = sample.status;
  delete sample.status;
  expect(() => schemas.meetingJobSnapshot.parse(sample, "snapshot")).toThrow(ContractError);
  expect(() => schemas.meetingJobSnapshot.parse(sample, "snapshot")).toThrow("snapshot.status: missing");
});
