export type PlainTranscriptionCancelState = {
  transcribing: boolean;
  meetingJobStatus: string | null;
  cancellingPlain: boolean;
};

export const MAX_RECENT_TRANSCRIBE_FILES = 5;

const MISSING_FILE_HINTS = [
  "no such file",
  "not found",
  "enoent",
  "does not exist",
  "failed to open",
  "could not open",
];

/**
 * Whether the plain (non-diarized) transcription Stop/Cancel control is
 * enabled. Mirrors the meeting-cancel semantics from #234: only the plain
 * path (meetingJobStatus === null) while a run is in flight and not already
 * cancelling.
 */
export function canCancelPlainTranscription(state: PlainTranscriptionCancelState): boolean {
  return state.transcribing && state.meetingJobStatus === null && !state.cancellingPlain;
}

/**
 * Push a file path onto the session-only recent-files list (#235).
 * Paths only — never audio or transcript content. Most-recent-first,
 * deduplicated by exact string, capped at MAX_RECENT_TRANSCRIBE_FILES.
 * Pure: returns a new array, never mutates the input.
 */
export function pushRecentTranscribeFile(list: string[], file: string): string[] {
  const trimmed = file.trim();
  if (!trimmed) return [...list];
  const rest = list.filter((entry) => entry !== trimmed);
  return [trimmed, ...rest].slice(0, MAX_RECENT_TRANSCRIBE_FILES);
}

/**
 * Stage a recent entry for a new run with settings still editable (#235).
 * Returns the path when it is still in the list, otherwise null so the
 * caller can show a graceful missing-file message instead of crashing.
 */
export function stageRecentTranscribeFile(list: string[], file: string): string | null {
  return list.includes(file) ? file : null;
}

/**
 * Whether the one-click retry control is available (#235): a previous file
 * exists and no transcription, reprocessing, or review-init work is busy.
 */
export function canRetryTranscribeFile(
  transcribing: boolean,
  reprocessingBusy: boolean,
  reviewInitBusy: boolean,
  lastFile: string | null,
): boolean {
  if (transcribing || reprocessingBusy || reviewInitBusy) return false;
  return typeof lastFile === "string" && lastFile.trim().length > 0;
}

/**
 * Whether a transcription failure looks like a missing/moved file (#235).
 * Used to prune the session-only recent list gracefully without crashing.
 */
export function isMissingTranscribeFileError(error: unknown): boolean {
  const message = typeof error === "string" ? error : (error as Error)?.message ?? "";
  const lowered = message.toLowerCase();
  return MISSING_FILE_HINTS.some((hint) => lowered.includes(hint));
}

/**
 * Drop a missing file from the recent list (#235). Pure: returns a new array.
 */
export function pruneMissingTranscribeFile(list: string[], file: string): string[] {
  return list.filter((entry) => entry !== file);
}
