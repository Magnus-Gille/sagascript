import type { MeetingJobStatus } from "./api";

export const meetingPhases = [
  "preparing",
  "decoding",
  "loading_model",
  "analyzing",
  "clustering",
  "finalizing",
] as const;

export type MeetingProgressPhase = (typeof meetingPhases)[number];

const phaseLabels: Record<MeetingProgressPhase, string> = {
  preparing: "Preparing the recording",
  decoding: "Decoding audio",
  loading_model: "Loading speech model",
  analyzing: "Analyzing speech",
  clustering: "Identifying speakers",
  finalizing: "Finalizing transcript",
};

const phaseDetails: Record<MeetingProgressPhase, string> = {
  preparing: "Preparing the recording for local processing.",
  decoding: "Reading the audio track.",
  loading_model: "Loading the local speech model.",
  analyzing: "Transcribing speech and identifying speakers. This can take several minutes for long recordings.",
  clustering: "Grouping speech by speaker.",
  finalizing: "Writing the transcript and speaker labels.",
};

export function phaseLabel(phase: string): string {
  return phaseLabels[phase as MeetingProgressPhase] ?? "Processing meeting";
}

export function phaseDetail(phase: string): string {
  return phaseDetails[phase as MeetingProgressPhase] ?? "Processing the meeting locally.";
}

export function statusTitle(status: MeetingJobStatus): string {
  switch (status) {
    case "completed":
      return "Meeting processing complete";
    case "cancelled":
      return "Meeting processing cancelled";
    case "failed":
      return "Meeting processing failed";
    case "cancelling":
      return "Cancelling meeting processing";
    case "running":
      return "Starting meeting processing";
  }
}

function nonNegativeMilliseconds(value: number): number {
  return Number.isFinite(value) ? Math.max(0, Math.floor(value)) : 0;
}

export function formatDuration(milliseconds: number): string {
  const totalSeconds = Math.floor(nonNegativeMilliseconds(milliseconds) / 1000);
  const seconds = totalSeconds % 60;
  const minutes = Math.floor(totalSeconds / 60) % 60;
  const hours = Math.floor(totalSeconds / 3600);
  const paddedSeconds = String(seconds).padStart(2, "0");
  const paddedMinutes = String(minutes).padStart(2, "0");
  return hours > 0 ? `${hours}:${paddedMinutes}:${paddedSeconds}` : `${paddedMinutes}:${paddedSeconds}`;
}

function formatSecondsAgo(milliseconds: number): string {
  const seconds = Math.max(1, Math.floor(nonNegativeMilliseconds(milliseconds) / 1000));
  return `${seconds} second${seconds === 1 ? "" : "s"} ago`;
}

function formatResponseAge(milliseconds: number): string {
  const seconds = Math.max(1, Math.floor(nonNegativeMilliseconds(milliseconds) / 1000));
  return `${seconds} second${seconds === 1 ? "" : "s"}`;
}

export function statusCheckMessage(lastCheckedAgoMs: number | null, pollingFailed: boolean): string {
  if (pollingFailed) return "Status check failed — processing may still be running.";
  if (lastCheckedAgoMs === null || !Number.isFinite(lastCheckedAgoMs)) {
    return "No status response yet — processing may still be running.";
  }
  if (lastCheckedAgoMs > 10_000) {
    return `No status response for ${formatResponseAge(lastCheckedAgoMs)} — processing may still be running.`;
  }
  return lastCheckedAgoMs < 1_000 ? "Status checked just now" : `Status checked ${formatSecondsAgo(lastCheckedAgoMs)}`;
}
