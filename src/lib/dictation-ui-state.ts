export type BackendDictationState =
  | "idle"
  | "recording"
  | "loading_model"
  | "transcribing";

export type DictateButtonAction = "start" | "stop" | "blocked";

export function dictateButtonAction(
  backendState: BackendDictationState,
  testOwnsRecording: boolean,
): DictateButtonAction {
  if (backendState === "idle") return "start";
  if (backendState === "recording" && testOwnsRecording) return "stop";
  return "blocked";
}

export function retainTestRecordingOwnership(
  backendState: BackendDictationState,
  testOwnsRecording: boolean,
): boolean {
  return backendState === "recording" && testOwnsRecording;
}
