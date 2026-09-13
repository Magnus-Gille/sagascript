export type PlainTranscriptionCancelState = {
  transcribing: boolean;
  meetingJobStatus: string | null;
  cancellingPlain: boolean;
};

const MISSING_FILE_HINTS = [
  "no such file",
  "not found",
  "enoent",
  "does not exist",
  "failed to open",
  "could not open",
];

/**
 * Display name for the one-click re-run button (#235 follow-up): just the
 * file name, never the full path.
 */
export function transcribeBaseName(path: string | null): string {
  if (!path) return "";
  return path.split(/[\\/]/).pop() ?? "";
}

/**
 * Display value for the Transcribe-tab progress (#237). The backend only
 * reports percentages during active decoding, so a raw 0 renders as "hung"
 * through model load + warmup. Floor at 1% while a run is in flight (the
 * backend emits 1% at inference start, so the floor only covers genuine
 * pre-first-report silence) and reset to 0 when idle.
 */
export function displayTranscribeProgress(reported: number, transcribing: boolean): number {
  if (!transcribing) return 0;
  if (!Number.isFinite(reported)) return 1;
  return Math.max(1, Math.min(100, Math.floor(reported)));
}

/**
 * Smart Save… dialog defaults (#238): the audio file's folder + its
 * basename with a .txt extension, so "same folder" is one Enter press while
 * any other location stays one dialog away. Paths only — never content.
 */
export function transcribeSaveDefaults(sourcePath: string | null): {
  fileName: string;
  directory: string | null;
} {
  if (!sourcePath || !sourcePath.trim()) {
    return { fileName: "transcription.txt", directory: null };
  }
  const trimmed = sourcePath.trim();
  const base = trimmed.split(/[\\/]/).pop() ?? "";
  const stem = base.replace(/\.[^.]+$/, "");
  const safe = stem.replace(/[\\/:*?"<>|]/g, "_").trim() || "transcription";
  const dirIdx = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  return {
    fileName: `${safe}.txt`,
    directory: dirIdx > 0 ? trimmed.slice(0, dirIdx) : null,
  };
}

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
 * Whether the one-click re-run control is available (#235): a previous file
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
 * Used to forget the remembered file gracefully without crashing.
 */
export function isMissingTranscribeFileError(error: unknown): boolean {
  const message = typeof error === "string" ? error : (error as Error)?.message ?? "";
  const lowered = message.toLowerCase();
  return MISSING_FILE_HINTS.some((hint) => lowered.includes(hint));
}
