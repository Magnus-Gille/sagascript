// The recorder's frontend types equal the schemas that will parse the Rust
// fixtures (SAGA-564), and the test fixtures parse as the types.
import { describe, expect, expectTypeOf, test } from "vitest";
import * as schemas from "../test/transport-schemas";
import type { Infer } from "../test/schema";
import { defaultFolder, finishedRecording, recoveryNote, sessionInfo, summary } from "../test/record";
import type { RecordingFinished, RecordingFolderInfo, RecordingSessionInfo, RecordingSummary, RecoveryNote } from "./record-types";

describe("recorder payload types", () => {
  test("each type equals its schema", () => {
    expectTypeOf<Infer<typeof schemas.recordingSessionInfo>>().toEqualTypeOf<RecordingSessionInfo>();
    expectTypeOf<Infer<typeof schemas.recordingSummary>>().toEqualTypeOf<RecordingSummary>();
    expectTypeOf<Infer<typeof schemas.recordingFinished>>().toEqualTypeOf<RecordingFinished>();
    expectTypeOf<Infer<typeof schemas.recoveryNote>>().toEqualTypeOf<RecoveryNote>();
    expectTypeOf<Infer<typeof schemas.recordingFolderInfo>>().toEqualTypeOf<RecordingFolderInfo>();
  });

  test("the test fixtures are what the schemas accept", () => {
    expect(schemas.recordingSessionInfo.parse(sessionInfo())).toEqual(sessionInfo());
    expect(schemas.recordingSummary.parse(summary())).toEqual(summary());
    expect(schemas.recordingFinished.parse(finishedRecording)).toEqual(finishedRecording);
    expect(schemas.recoveryNote.parse(recoveryNote())).toEqual(recoveryNote());
    expect(schemas.recordingFolderInfo.parse(defaultFolder)).toEqual(defaultFolder);
  });

  test("a field the frontend does not declare fails", () => {
    expect(() => schemas.recordingSessionInfo.parse({ ...sessionInfo(), extra: 1 })).toThrow(/extra/);
  });
});
