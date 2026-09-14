import type { MeetingJobSnapshot } from "./api";

export interface MeetingJobPollOptions {
  jobId: string;
  get: (jobId: string) => Promise<MeetingJobSnapshot>;
  isCurrent: () => boolean;
  onSnapshot: (snapshot: MeetingJobSnapshot) => void;
  onFailure: (error: unknown) => void;
  wait: () => Promise<void>;
}

const terminalStatuses = new Set(["completed", "cancelled", "failed"]);

/**
 * Poll one backend job without allowing overlapping requests. The caller owns
 * UI state and generation invalidation; this helper only enforces the job-id
 * boundary and the serialized request/response sequence.
 */
export async function pollMeetingJob(options: MeetingJobPollOptions): Promise<void> {
  while (options.isCurrent()) {
    let snapshot: MeetingJobSnapshot;
    try {
      snapshot = await options.get(options.jobId);
    } catch (error) {
      if (options.isCurrent()) options.onFailure(error);
      return;
    }

    if (!options.isCurrent()) return;
    if (snapshot.id !== options.jobId) {
      options.onFailure(new Error("Meeting job identity changed while polling."));
      return;
    }

    options.onSnapshot(snapshot);
    if (terminalStatuses.has(snapshot.status)) return;
    await options.wait();
  }
}
