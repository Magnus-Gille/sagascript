import { describe, expect, expectTypeOf, test } from "vitest";
import type { PreflightReport } from "./acceptance";
import reportFixture from "../../src-tauri/crates/sagascript-core/src/acceptance/report.fixture.json";
import { array, boolean, literal, nullable, number, object, string, type Infer, type Schema } from "../test/schema";

const observationStatus = literal("observed", "unknown", "unsupported", "not_observed", "invalid", "error");
const observationSource = literal("generated_metadata", "current_process", "persisted_fixture", "live_gui", "not_observed");
const microphonePermissionStatus = literal("authorized", "denied", "restricted", "not_determined", "unsupported", "unknown");
const accessibilityStatus = literal("trusted", "not_trusted", "unsupported");
const checkStatus = literal("passed", "blocked");

function observation<T>(value: Schema<T>) {
  return object({
    status: observationStatus,
    source: observationSource,
    value: nullable(value),
  });
}

const preflightReport = object({
  schema_version: number,
  scope: observationSource,
  expected_revision: string,
  platform: string,
  architecture: string,
  build: object({
    version: string,
    revision: observation(string),
    dirty: observation(boolean),
  }),
  process: object({
    microphone: object({
      status: observationStatus,
      source: observationSource,
      value: nullable(microphonePermissionStatus),
    }),
    accessibility: object({
      status: observationStatus,
      source: observationSource,
      value: nullable(accessibilityStatus),
    }),
    in_app_bundle: observation(boolean),
    binary_sha256: observation(string),
  }),
  onboarding: observation(boolean),
  engine: object({
    status: observationStatus,
    source: observationSource,
    value: literal(null),
  }),
  model: object({
    status: observationStatus,
    source: observationSource,
    value: literal(null),
  }),
  gui_session: object({
    status: observationStatus,
    source: observationSource,
    value: literal(null),
  }),
  summary: object({
    status: checkStatus,
    checks: array(object({ name: string, status: checkStatus })),
    remaining_manual_steps: array(string),
  }),
});

describe("acceptance preflight transport contract", () => {
  test("the report schema exactly matches the frontend type", () => {
    expectTypeOf<Infer<typeof preflightReport>>().toEqualTypeOf<PreflightReport>();
  });

  test("the shared Rust report fixture parses without unknown fields", () => {
    expect(preflightReport.parse(reportFixture)).toEqual(reportFixture);
  });

  test("all observation status wire values are accepted", () => {
    for (const status of ["observed", "unknown", "unsupported", "not_observed", "invalid", "error"] as const) {
      expect(observationStatus.parse(status)).toBe(status);
    }
  });
});
