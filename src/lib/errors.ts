// How the frontend reads a failed command (ADR 0006, #360).
//
// A Rust command that returns `Result<T, CommandError>` rejects `invoke()` with
// a `BackendError`; one that still returns `Result<T, String>` rejects with a
// string (`CommandMap[C]["error"]` in ipc-contract.generated.ts says which).
// Branch on `code` with `isBackendError(error, code)` and never on the text of
// `message`: the text is for the user, can be reworded, and can be in another
// language one day. Show a failure with `toErrorMessage`.

/**
 * The error object of a command that returns `Result<T, CommandError>`
 * (src-tauri/src/command_error.rs). `code` is stable snake_case,
 * `message` is English text for the user without absolute paths, and
 * `retryable` says whether the user's action may be repeated (record again, run
 * the file again) and is expected to succeed. It does not say that the same
 * `invoke` may be repeated: `stop_and_transcribe` has consumed the audio, so a
 * second call resolves with an empty text. A timeout is not retryable.
 */
export interface BackendError {
  code: string;
  message: string;
  retryable: boolean;
}

/**
 * Whether `error` is a `BackendError`, and, when `code` is given, that one.
 * An `Error`, a string or a system error with a `code` of its own is not.
 */
export function isBackendError(error: unknown, code?: string): error is BackendError {
  if (typeof error !== "object" || error === null) return false;
  const candidate = error as Record<string, unknown>;
  if (typeof candidate.code !== "string" || typeof candidate.message !== "string" || typeof candidate.retryable !== "boolean") {
    return false;
  }
  return code === undefined || candidate.code === code;
}

/**
 * The text to show for anything a rejected promise can carry: a string (an
 * error of a command that is not typed yet), a `BackendError`, an `Error`, or
 * something unknown (`fallback`).
 */
export function toErrorMessage(error: unknown, fallback = "Unknown error"): string {
  if (typeof error === "string") return error.trim() ? error : fallback;
  if (typeof error === "object" && error !== null && "message" in error) {
    const { message } = error;
    if (typeof message === "string" && message.trim()) return message;
  }
  return fallback;
}

/** What the user can do about a failed transcription, from its `code`. */
export type BackendErrorRecovery = "retry" | "download_model";

/**
 * The offered way out of an error: download the model again when the engine's
 * model is missing or cannot be loaded (trying again would fail the same way),
 * try again when the backend says the user's action may be repeated, otherwise
 * nothing (`null`). "Try again" must restart the user's action (record again,
 * run the file again), never re-invoke the failed command: a repeated
 * `stop_and_transcribe` finds no recording and resolves with an empty text.
 */
export function backendErrorRecovery(error: unknown): BackendErrorRecovery | null {
  if (!isBackendError(error)) return null;
  if (error.code === "engine_model_missing" || error.code === "engine_model_load_failed") return "download_model";
  return error.retryable ? "retry" : null;
}

/**
 * What to offer after "Download the model again" itself failed: the download
 * again when it may pass the next time (the download failed, the engine's model
 * is still the problem, or the backend says the action may be repeated), and
 * otherwise nothing. A refusal such as `pianissimo_unavailable` or
 * `invalid_input` fails the same way every time, and offering the download
 * again for it would loop for ever. Always the download, never "Try again":
 * the person's action was the download.
 */
export function downloadFailureRecovery(error: unknown): BackendErrorRecovery | null {
  if (!isBackendError(error)) return null;
  if (error.code === "model_download_failed" || error.retryable || backendErrorRecovery(error) === "download_model") {
    return "download_model";
  }
  return null;
}
