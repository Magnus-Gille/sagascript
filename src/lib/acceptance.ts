/** Read-only diagnostics reported by the acceptance preflight command. */
export type ObservationStatus = "observed" | "unknown" | "unsupported" | "not_observed" | "invalid" | "error";

export type ObservationSource = "generated_metadata" | "current_process" | "persisted_fixture" | "live_gui" | "not_observed";

export type AccessibilityStatus = "trusted" | "not_trusted" | "unsupported";
export type MicrophonePermissionStatus = "authorized" | "denied" | "restricted" | "not_determined" | "unsupported" | "unknown";
export type CheckStatus = "passed" | "blocked";

export interface Observation<T> {
  status: ObservationStatus;
  source: ObservationSource;
  value: T | null;
}

export interface BuildObservation {
  version: string;
  revision: Observation<string>;
  dirty: Observation<boolean>;
}

export interface ProcessObservations {
  microphone: Observation<MicrophonePermissionStatus>;
  accessibility: Observation<AccessibilityStatus>;
  in_app_bundle: Observation<boolean>;
  binary_sha256: Observation<string>;
}

export interface PreflightCheck {
  name: string;
  status: CheckStatus;
}

export interface PreflightSummary {
  status: CheckStatus;
  checks: PreflightCheck[];
  remaining_manual_steps: string[];
}

export interface PreflightReport {
  schema_version: number;
  scope: ObservationSource;
  expected_revision: string;
  platform: string;
  architecture: string;
  build: BuildObservation;
  process: ProcessObservations;
  onboarding: Observation<boolean>;
  engine: Observation<null>;
  model: Observation<null>;
  gui_session: Observation<null>;
  summary: PreflightSummary;
}
