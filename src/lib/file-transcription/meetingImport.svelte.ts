import { untrack } from "svelte";
import { toErrorMessage, type BackendErrorRecovery } from "../errors";
import {
  planMeetingReprocessing,
  beginMeetingReprocessing,
  previewMeetingProposal,
  resolveMeetingProposal,
  acceptMeetingProposal,
  saveMeetingProposal,
  openMeetingProposal,
} from "../meeting-reprocessing-api";
import type { ReprocessingMode, SelectedReprocessingPlan, ProposalState, ReprocessingResult } from "../meeting-reprocessing-types";
import { acceptMeetingProposalForCurrentReview, executeMeetingPlan } from "../meeting-reprocessing-flow";
import {
  setUpdateResultPending,
  beginMeetingFile,
  getMeetingJob,
  cancelMeetingJob,
  createMeetingReview,
  applyMeetingCorrections,
  undoMeetingReview,
  resetMeetingReview,
  openMeetingReview,
  saveMeetingReview,
  attachMeetingAudio,
  detachMeetingAudio,
  type MeetingJobStatus,
  type MeetingJobSnapshot,
} from "../api";
import type { MeetingReviewDraftSnapshot } from "../meeting-review-drafts";
import type {
  CorrectionOperation,
  MeetingDraftState,
  MeetingAudioAttachment,
  MeetingExportFormat,
  MeetingReview as MeetingReviewDocument,
  MeetingReviewState,
  MeetingTranscript,
} from "../meeting-types";
import type { UpdateRecoveryMeeting } from "../update-recovery";
import { pollMeetingJob as pollMeetingJobClient } from "../meeting-job-client";
import type { FileJob } from "../transcription-queue";

/**
 * What the meeting import reads from, and writes to, the component that owns
 * it. Props are read through getters so a changed prop is seen at once; the
 * run state shared with the plain transcription stays in the component.
 */
export interface MeetingImportHost {
  readonly job: FileJob;
  readonly otherBusy: boolean;
  readonly initialRecoveryMeeting: UpdateRecoveryMeeting | null;
  readonly onMeetingRecoveryChange: (entry: UpdateRecoveryMeeting | null) => void;
  transcribing: boolean;
  started: boolean;
  transcribeError: string;
  transcribeRecovery: BackendErrorRecovery | null;
  transcriptionResult: string;
}

const emptyMeetingDraft = (): MeetingDraftState => ({
  labels: {},
  mergeTargets: {},
  texts: {},
  speakers: {},
});

/**
 * A file imported with speaker diarization: the meeting job and its clock,
 * polling, the review document and its action queue, export, audio
 * attachment, the recovery draft, and reprocessing.
 */
export class MeetingImport {
  private readonly host: MeetingImportHost;

  hydratedMeetingRecoveryJobId = $state<string | null>(null);
  meetingRecoveryHydrationClosed = false;
  meetingReview: MeetingReviewDocument | null = $state(null);
  meetingTranscript: MeetingTranscript | null = $state(null);
  meetingJobId: string | null = $state(null);
  meetingResultId: string | null = $state(null);
  meetingJobStatus: MeetingJobStatus | null = $state(null);
  meetingPhase: string = $state("");
  meetingElapsedMs = $state(0);
  meetingPhaseElapsedMs = $state(0);
  meetingLastCheckedAgoMs = $state<number | null>(null);
  meetingTimingAvailable = $state(false);
  meetingClock: ReturnType<typeof setInterval> | undefined;
  meetingClockBaseAt = 0;
  meetingElapsedBase = 0;
  meetingPhaseElapsedBase = 0;
  meetingLastCheckedAt: number | null = null;
  meetingError: string = $state("");
  meetingPollingFailed: boolean = $state(false);
  meetingPollGeneration = 0;
  meetingPollActive = $state(false);
  meetingDocumentRevision = $state(0);
  meetingReviewResetKey = $state(0);
  meetingReviewDraftDirty = $state(false);
  meetingReprocessingPlan = $state<SelectedReprocessingPlan | null>(null);
  meetingProposal = $state<ProposalState | null>(null);
  meetingReprocessingResult = $state<ReprocessingResult | null>(null);
  meetingReprocessingBusy = $state(false);
  meetingActionQueue: Promise<void> = Promise.resolve();
  meetingReviewInit: Promise<void> | null = $state(null);
  meetingRecoveryDraft = $state<MeetingReviewDraftSnapshot | null>(null);

  constructor(host: MeetingImportHost) {
    this.host = host;
  }

  /** Body of the component's recovery-hydration effect. */
  hydrateRecovery = (): void => {
    const recovery = this.host.initialRecoveryMeeting;
    if (
      this.meetingRecoveryHydrationClosed ||
      !recovery ||
      recovery.job_id !== this.host.job.id ||
      this.hydratedMeetingRecoveryJobId === this.host.job.id
    )
      return;
    this.hydratedMeetingRecoveryJobId = this.host.job.id;
    this.host.started = true;
    const generation = ++this.meetingPollGeneration;
    this.meetingRecoveryDraft = {
      source_sha256: recovery.review.review.original.source_sha256,
      review_revision: recovery.review.review.revision,
      drafts: {
        labels: { ...recovery.editor_draft.labels },
        mergeTargets: { ...recovery.editor_draft.mergeTargets },
        texts: { ...recovery.editor_draft.texts },
        speakers: { ...recovery.editor_draft.speakers },
      },
    };
    this.acceptMeetingReview(recovery.review, generation, true);
    this.meetingResultId = recovery.job_id;
    this.meetingJobStatus = "completed";
    this.meetingReprocessingPlan = null;
    this.meetingProposal = recovery.proposal;
    this.meetingReprocessingResult = null;
  };

  /** Body of the component's effect that reports the recovery entry. */
  reportRecovery = (): void => {
    if (!this.meetingReview || !this.meetingTranscript) return;
    const entry: UpdateRecoveryMeeting = {
      job_id: this.host.job.id,
      path: this.host.job.path,
      review: { review: this.meetingReview, transcript: this.meetingTranscript },
      editor_draft: this.meetingRecoveryDraft?.drafts ?? emptyMeetingDraft(),
      proposal: this.meetingProposal,
    };
    untrack(() => this.host.onMeetingRecoveryChange(entry));
  };

  /** Body of the component's `onDestroy`. */
  destroy = (): void => {
    this.meetingPollGeneration += 1;
    this.stopMeetingClock();
  };

  updateMeetingClock = (now = performance.now()): void => {
    const sinceSnapshot = Math.max(0, now - this.meetingClockBaseAt);
    this.meetingElapsedMs = this.meetingElapsedBase + sinceSnapshot;
    this.meetingPhaseElapsedMs = this.meetingPhaseElapsedBase + sinceSnapshot;
    this.meetingLastCheckedAgoMs = this.meetingLastCheckedAt === null ? null : Math.max(0, now - this.meetingLastCheckedAt);
  };

  startMeetingClock = (): void => {
    this.stopMeetingClock();
    this.meetingClockBaseAt = performance.now();
    this.meetingElapsedBase = 0;
    this.meetingPhaseElapsedBase = 0;
    this.meetingLastCheckedAt = null;
    this.meetingElapsedMs = 0;
    this.meetingPhaseElapsedMs = 0;
    this.meetingLastCheckedAgoMs = null;
    this.meetingTimingAvailable = false;
    this.meetingClock = setInterval(this.updateMeetingClock, 250);
  };

  stopMeetingClock = (): void => {
    clearInterval(this.meetingClock);
    this.meetingClock = undefined;
  };

  acceptMeetingTiming = (snapshot: MeetingJobSnapshot): void => {
    const now = performance.now();
    this.updateMeetingClock(now);
    // Interpolate between polls without moving live clocks backwards when an
    // older snapshot arrives late. Terminal snapshots supply the exact worker
    // duration; a new phase starts from its own native clock.
    const active = snapshot.status === "running" || snapshot.status === "cancelling";
    const elapsed = snapshot.elapsed_ms ?? this.meetingElapsedMs;
    const phaseElapsed = snapshot.phase_elapsed_ms ?? (snapshot.phase === this.meetingPhase ? this.meetingPhaseElapsedMs : 0);
    this.meetingElapsedBase = active ? Math.max(this.meetingElapsedMs, elapsed) : elapsed;
    this.meetingPhaseElapsedBase =
      active && snapshot.phase === this.meetingPhase ? Math.max(this.meetingPhaseElapsedMs, phaseElapsed) : phaseElapsed;
    this.meetingTimingAvailable = typeof snapshot.elapsed_ms === "number" && typeof snapshot.phase_elapsed_ms === "number";
    this.meetingClockBaseAt = now;
    this.meetingLastCheckedAt = now;
    this.updateMeetingClock(now);
  };

  waitForMeetingPoll = (): Promise<void> => {
    return new Promise((resolve) => window.setTimeout(resolve, 500));
  };

  waitForMeetingActions = (): Promise<void> => {
    return this.meetingActionQueue;
  };

  setMeetingResultPending = async (pending: boolean): Promise<void> => {
    if (this.meetingResultId) {
      await setUpdateResultPending(`meeting:${this.meetingResultId}`, pending);
    }
  };

  enqueueMeetingAction = (action: (review: MeetingReviewDocument, revision: number) => Promise<boolean | void>): Promise<boolean> => {
    const queued = this.meetingActionQueue.then(async () => {
      const review = this.meetingReview;
      if (!review) return false;
      return (await action(review, this.meetingDocumentRevision)) !== false;
    });
    this.meetingActionQueue = queued.then(
      () => undefined,
      () => undefined,
    );
    return queued;
  };

  acceptMeetingReview = (state: MeetingReviewState, generation: number, replaceDocument = false): void => {
    if (generation !== this.meetingPollGeneration) return;
    this.meetingReview = state.review;
    this.meetingTranscript = state.transcript;
    this.meetingDocumentRevision += 1;
    if (replaceDocument) {
      this.meetingReviewResetKey += 1;
      this.meetingReprocessingPlan = null;
      this.meetingProposal = null;
      this.meetingReprocessingResult = null;
    }
    this.meetingError = "";
  };

  initializeMeetingReview = async (transcript: MeetingTranscript, generation: number): Promise<void> => {
    const state = await createMeetingReview(transcript);
    this.acceptMeetingReview(state, generation, true);
  };

  pollMeetingJob = async (jobId: string, generation: number): Promise<void> => {
    if (this.meetingPollActive) return;
    this.meetingPollActive = true;
    try {
      await pollMeetingJobClient({
        jobId,
        get: getMeetingJob,
        isCurrent: () => generation === this.meetingPollGeneration,
        onFailure: (error: unknown) => {
          this.meetingPollingFailed = true;
          this.meetingError = toErrorMessage(error, "Could not check meeting progress.") + " Retry the status check to continue.";
        },
        onSnapshot: (snapshot: MeetingJobSnapshot) => {
          if (generation !== this.meetingPollGeneration) return;
          this.acceptMeetingTiming(snapshot);
          this.meetingJobStatus = snapshot.status;
          this.meetingPhase = snapshot.phase;
          if (snapshot.status === "completed" || snapshot.status === "cancelled" || snapshot.status === "failed") {
            this.stopMeetingClock();
            this.meetingJobId = null;
            this.host.transcribing = false;
            this.meetingPollingFailed = false;

            if (snapshot.status === "completed" && snapshot.reprocessing) {
              const result = snapshot.reprocessing;
              // Keep the completed work even if the separate preview request fails.
              this.meetingReprocessingResult = result;
              this.meetingReprocessingPlan = null;
              this.meetingProposal = null;
              this.meetingReviewInit = previewMeetingProposal(result.proposal)
                .then((state) => {
                  if (generation !== this.meetingPollGeneration) return;
                  this.meetingProposal = state;
                  this.meetingReprocessingResult = result;
                  this.meetingReprocessingPlan = null;
                })
                .catch((error) => {
                  if (generation === this.meetingPollGeneration) {
                    this.meetingError = toErrorMessage(error, "Could not preview the proposal. The previous review is unchanged.");
                  }
                })
                .finally(() => {
                  if (generation === this.meetingPollGeneration) this.meetingReviewInit = null;
                });
            } else if (snapshot.status === "completed" && snapshot.transcript) {
              this.meetingReviewInit = this.initializeMeetingReview(snapshot.transcript, generation)
                .catch((error) => {
                  if (generation === this.meetingPollGeneration) {
                    this.meetingError = toErrorMessage(error, "Could not initialize the meeting review.");
                  }
                })
                .finally(() => {
                  if (generation === this.meetingPollGeneration) this.meetingReviewInit = null;
                });
            } else if (snapshot.status === "completed") {
              this.meetingError = "Meeting completed without a transcript. Try the import again.";
            } else {
              this.meetingError =
                snapshot.error ?? (snapshot.status === "cancelled" ? "Meeting import was cancelled." : "Meeting import failed.");
            }
          }
        },
        wait: this.waitForMeetingPoll,
      });
    } finally {
      this.meetingPollActive = false;
    }
  };

  startMeetingFileTranscription = async (filePath: string, prompt: string | null, profileId: string | null): Promise<void> => {
    if (this.host.transcribing) return;
    if (this.meetingReview && !window.confirm("Start a new meeting review and replace the current review if it completes?")) return;
    // `initialRecoveryMeeting` is also updated by the live recovery callback
    // after this job completes. Do not mistake that in-session echo for a
    // startup restore: hydrating it would invalidate this poll generation and
    // could leave `meetingReviewInit` set forever.
    this.meetingRecoveryHydrationClosed = true;
    if (this.meetingResultId) {
      try {
        await this.setMeetingResultPending(false);
      } catch (error) {
        this.meetingError = toErrorMessage(error, "Could not preserve the current meeting result.");
        return;
      }
    }
    this.meetingResultId = null;
    const generation = ++this.meetingPollGeneration;
    // Keep the previous review and its unsaved drafts mounted until a NEW
    // document succeeds. Pending edits finish before the import starts below;
    // successful replacement still invalidates any stale action revision.
    this.host.transcribing = true;

    this.host.transcribeError = "";
    this.host.transcribeRecovery = null;
    this.host.transcriptionResult = "";
    this.meetingError = "";
    this.meetingPollingFailed = false;
    this.meetingJobId = null;
    this.meetingJobStatus = "running";
    this.meetingPhase = "Starting";
    this.startMeetingClock();
    try {
      await this.waitForMeetingActions();
      if (generation !== this.meetingPollGeneration) return;
      const jobId = await beginMeetingFile(filePath, prompt, profileId);
      if (generation !== this.meetingPollGeneration) return;
      if (!jobId) throw new Error("Meeting import did not return a job ID.");
      this.meetingJobId = jobId;
      this.meetingResultId = jobId;
      this.meetingJobStatus = "running";
      void this.pollMeetingJob(jobId, generation);
    } catch (error) {
      if (generation !== this.meetingPollGeneration) return;
      this.host.transcribing = false;
      this.meetingJobId = null;
      this.meetingJobStatus = "failed";
      this.meetingPhase = "Failed";
      this.updateMeetingClock();
      this.stopMeetingClock();
      this.meetingError = toErrorMessage(error, "Could not start meeting import.");
    }
  };

  cancelMeetingImport = async (): Promise<void> => {
    const jobId = this.meetingJobId;
    const generation = this.meetingPollGeneration;
    if (!jobId || this.meetingJobStatus === "cancelling") return;
    try {
      const accepted = await cancelMeetingJob(jobId);
      if (generation !== this.meetingPollGeneration || this.meetingJobId !== jobId) return;
      if (accepted) {
        this.meetingJobStatus = "cancelling";
        this.meetingError = "";
      } else {
        this.meetingError = "The meeting has already finished. Its final status is being retrieved.";
      }
    } catch (error) {
      if (generation !== this.meetingPollGeneration || this.meetingJobId !== jobId) return;
      this.meetingError = toErrorMessage(error, "Could not request cancellation. The meeting is still running.");
    }
  };

  retryMeetingPolling = (): void => {
    if (!this.meetingJobId || this.meetingPollActive) return;
    this.meetingPollingFailed = false;
    this.meetingError = "";
    this.host.transcribing = true;
    void this.pollMeetingJob(this.meetingJobId, this.meetingPollGeneration);
  };

  exportMeetingReview = async (format: MeetingExportFormat): Promise<boolean> => {
    const saved = await this.enqueueMeetingAction((review) => saveMeetingReview(review, format));
    if (saved) {
      await this.setMeetingResultPending(false);
      this.host.onMeetingRecoveryChange(null);
    }
    return saved;
  };

  applyMeetingReviewOperations = async (operations: CorrectionOperation[]): Promise<void> => {
    await this.enqueueMeetingAction(async (review, revision) => {
      const corrections = {
        schema_version: 1,
        source_sha256: review.original.source_sha256,
        original_revision: review.original_revision,
        expected_revision: review.revision,
        operations,
      };
      const state = await applyMeetingCorrections(review, corrections);
      if (revision === this.meetingDocumentRevision) this.acceptMeetingReview(state, this.meetingPollGeneration);
    });
  };

  undoMeetingReviewChanges = async (): Promise<void> => {
    await this.enqueueMeetingAction(async (review, revision) => {
      const state = await undoMeetingReview(review, review.revision);
      if (revision === this.meetingDocumentRevision) this.acceptMeetingReview(state, this.meetingPollGeneration);
    });
  };

  resetMeetingReviewChanges = async (): Promise<void> => {
    await this.enqueueMeetingAction(async (review, revision) => {
      const state = await resetMeetingReview(review, review.revision);
      if (revision === this.meetingDocumentRevision) this.acceptMeetingReview(state, this.meetingPollGeneration);
    });
  };

  saveCurrentMeetingReview = async (): Promise<boolean> => {
    const saved = await this.enqueueMeetingAction((review) => saveMeetingReview(review, "json"));
    if (saved) {
      await this.setMeetingResultPending(false);
      this.host.onMeetingRecoveryChange(null);
    }
    return saved;
  };

  attachCurrentMeetingAudio = async (): Promise<MeetingAudioAttachment | null> => {
    const review = this.meetingReview;
    const transcript = this.meetingTranscript;
    const generation = this.meetingPollGeneration;
    const resetKey = this.meetingReviewResetKey;
    if (!review || !transcript) return null;
    const attachment = await attachMeetingAudio(transcript.source_sha256);
    const stillCurrent =
      generation === this.meetingPollGeneration &&
      resetKey === this.meetingReviewResetKey &&
      this.meetingReview?.original_revision === review.original_revision &&
      this.meetingTranscript?.source_sha256 === transcript.source_sha256;
    if (!stillCurrent) {
      if (attachment) await detachMeetingAudio(attachment.token).catch(() => undefined);
      return null;
    }
    return attachment;
  };

  onMeetingReviewDraftDirtyChange = (dirty: boolean): void => {
    this.meetingReviewDraftDirty = dirty;
    if (dirty) void this.setMeetingResultPending(true);
  };

  onMeetingReviewDraftSnapshotChange = (snapshot: MeetingReviewDraftSnapshot): void => {
    this.meetingRecoveryDraft = snapshot;
    if (!this.meetingReview || !this.meetingTranscript) return;
    const review = this.meetingReview;
    const transcript = this.meetingTranscript;
    untrack(() =>
      this.host.onMeetingRecoveryChange({
        job_id: this.host.job.id,
        path: this.host.job.path,
        review: { review, transcript },
        editor_draft: snapshot.drafts,
        proposal: this.meetingProposal,
      }),
    );
  };

  detachCurrentMeetingAudio = async (token: string): Promise<void> => {
    await detachMeetingAudio(token);
  };

  openSavedMeetingReview = async (): Promise<void> => {
    if (this.host.transcribing) return;
    if (this.meetingReview && !window.confirm("Open a saved review and replace the current review if it succeeds?")) return;
    const generation = ++this.meetingPollGeneration;
    this.meetingError = "";
    try {
      await this.waitForMeetingActions();
      const state = await openMeetingReview();
      if (generation !== this.meetingPollGeneration || !state) return;
      this.acceptMeetingReview(state, generation, true);
      if (this.meetingResultId) await this.setMeetingResultPending(false);
      this.meetingResultId = null;
    } catch (error) {
      if (generation === this.meetingPollGeneration) this.meetingError = toErrorMessage(error, "Could not open the meeting review.");
    }
  };

  planCurrentMeeting = async (mode: ReprocessingMode, threshold: number, saveCache: boolean): Promise<void> => {
    if (this.host.otherBusy || this.host.transcribing || this.meetingReprocessingBusy || this.meetingReviewDraftDirty) return;
    this.meetingReprocessingBusy = true;
    try {
      await this.enqueueMeetingAction(async (review, revision) => {
        const generation = this.meetingPollGeneration;
        const selected = await planMeetingReprocessing(review, mode, threshold, saveCache, this.host.job.prompt, this.host.job.profileId);
        if (selected && revision === this.meetingDocumentRevision && generation === this.meetingPollGeneration) {
          this.meetingReprocessingPlan = selected;
        }
      });
    } finally {
      this.meetingReprocessingBusy = false;
    }
  };

  executeCurrentMeetingPlan = async (): Promise<void> => {
    await executeMeetingPlan({
      read: () => ({
        otherBusy: this.host.otherBusy,
        transcribing: this.host.transcribing,
        reprocessingBusy: this.meetingReprocessingBusy,
        draftDirty: this.meetingReviewDraftDirty,
        plan: this.meetingReprocessingPlan,
        review: this.meetingReview,
      }),
      waitForActions: this.waitForMeetingActions,
      execute: async (selected, review) => {
        const generation = ++this.meetingPollGeneration;
        this.host.transcribing = true;
        this.meetingError = "";
        this.meetingPollingFailed = false;
        this.meetingJobStatus = "running";
        this.meetingPhase = "Preparing reprocessing";
        this.startMeetingClock();
        try {
          const id = await beginMeetingReprocessing(selected, review, this.host.job.prompt, this.host.job.profileId);
          if (!id) throw new Error("Reprocessing did not return a job ID.");
          this.meetingJobId = id;
          this.meetingResultId = id;
          void this.pollMeetingJob(id, generation);
        } catch (error) {
          this.host.transcribing = false;
          this.meetingJobId = null;
          this.meetingJobStatus = "failed";
          this.meetingError = toErrorMessage(error, "Reprocessing failed; the previous review is unchanged.");
          this.updateMeetingClock();
          this.stopMeetingClock();
          throw error;
        }
      },
    });
  };

  resolveCurrentMeetingProposal = async (index: number, operations: CorrectionOperation[]): Promise<void> => {
    const state = this.meetingProposal;
    if (!state || this.host.transcribing || this.meetingReprocessingBusy) return;
    this.meetingReprocessingBusy = true;
    try {
      const next = await resolveMeetingProposal(state.proposal, index, operations);
      if (this.meetingProposal?.proposal.revision === state.proposal.revision) this.meetingProposal = next;
    } finally {
      this.meetingReprocessingBusy = false;
    }
  };

  acceptCurrentMeetingProposal = async (): Promise<void> => {
    if (this.host.otherBusy || this.host.transcribing || this.meetingReprocessingBusy || this.meetingReviewDraftDirty) return;
    this.meetingReprocessingBusy = true;
    try {
      await this.enqueueMeetingAction(async (review, revision) => {
        return acceptMeetingProposalForCurrentReview({
          revision,
          review,
          read: () => ({
            documentRevision: this.meetingDocumentRevision,
            pollGeneration: this.meetingPollGeneration,
            draftDirty: this.meetingReviewDraftDirty,
            proposal: this.meetingProposal,
          }),
          accept: (proposal, currentReview) => acceptMeetingProposal(proposal.proposal, currentReview),
          apply: (state, generation) => this.acceptMeetingReview(state, generation, true),
        });
      });
    } finally {
      this.meetingReprocessingBusy = false;
    }
  };

  saveCurrentMeetingProposal = async (): Promise<boolean> => {
    const proposal = this.meetingProposal?.proposal ?? this.meetingReprocessingResult?.proposal;
    const saved = proposal ? await saveMeetingProposal(proposal) : false;
    if (saved) await this.setMeetingResultPending(false);
    return saved;
  };

  retryCurrentMeetingProposalPreview = async (): Promise<void> => {
    const result = this.meetingReprocessingResult;
    if (!result || this.meetingProposal || this.host.transcribing || this.meetingReprocessingBusy || this.meetingReviewInit) return;
    const generation = this.meetingPollGeneration;
    this.meetingReprocessingBusy = true;
    this.meetingError = "";
    try {
      const state = await previewMeetingProposal(result.proposal);
      if (generation === this.meetingPollGeneration && this.meetingReprocessingResult === result) this.meetingProposal = state;
    } catch (error) {
      this.meetingError = toErrorMessage(
        error,
        "Could not preview the proposal. Save it to retry later; the previous review is unchanged.",
      );
    } finally {
      this.meetingReprocessingBusy = false;
    }
  };

  openCurrentMeetingProposal = async (): Promise<void> => {
    if (this.host.otherBusy || this.host.transcribing || this.meetingReprocessingBusy) return;
    this.meetingReprocessingBusy = true;
    const generation = this.meetingPollGeneration;
    try {
      const state = await openMeetingProposal();
      if (state && generation === this.meetingPollGeneration) {
        this.meetingProposal = state;
        this.meetingReprocessingResult = null;
        this.meetingReprocessingPlan = null;
      }
    } finally {
      this.meetingReprocessingBusy = false;
    }
  };
}
