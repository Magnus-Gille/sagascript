/**
 * Derive the Dictate tab's visible test controls from a backend lifecycle
 * event. Global hotkeys can change the recording state without a click in the
 * settings window, so this transition stays deliberately pure and testable.
 *
 * @param {{ recording: boolean, transcribing: boolean, error: string }} current
 * @param {unknown} event
 */
export function dictationTestStateForEvent(current, event) {
  switch (event) {
    case "recording":
      return { recording: true, transcribing: false, error: "" };
    case "transcribing":
    case "loading_model":
      return { recording: false, transcribing: true, error: current.error };
    case "idle":
      return { recording: false, transcribing: false, error: current.error };
    default:
      return current;
  }
}
