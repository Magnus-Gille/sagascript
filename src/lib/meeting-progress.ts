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

export type StatusCheckState = "healthy" | "waiting" | "stale" | "failed";

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

export function statusCheckState(lastCheckedAgoMs: number | null, pollingFailed: boolean): StatusCheckState {
  if (pollingFailed) return "failed";
  if (lastCheckedAgoMs === null || !Number.isFinite(lastCheckedAgoMs)) return "waiting";
  return lastCheckedAgoMs > 10_000 ? "stale" : "healthy";
}

export function statusCheckAnnouncement(state: StatusCheckState): string {
  switch (state) {
    case "healthy":
      return "Status checks are up to date.";
    case "waiting":
      return "Waiting for the first status response; processing may still be running.";
    case "stale":
      return "No recent status response — processing may still be running.";
    case "failed":
      return "Status check failed — processing may still be running.";
  }
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
      return "Processing meeting";
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
  switch (statusCheckState(lastCheckedAgoMs, pollingFailed)) {
    case "failed":
      return "Status check failed — processing may still be running.";
    case "waiting":
      return "No status response yet — processing may still be running.";
    case "stale":
      return `No status response for ${formatResponseAge(lastCheckedAgoMs ?? 0)} — processing may still be running.`;
    case "healthy":
      return lastCheckedAgoMs !== null && lastCheckedAgoMs >= 1_000
        ? `Status checked ${formatSecondsAgo(lastCheckedAgoMs)}`
        : "Status checked just now";
  }
}
