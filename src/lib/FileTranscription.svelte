<script lang="ts">
  import MeetingReprocessing from "./MeetingReprocessing.svelte";
  import {
    planMeetingReprocessing, beginMeetingReprocessing, previewMeetingProposal,
    resolveMeetingProposal, acceptMeetingProposal, saveMeetingProposal, openMeetingProposal,
  } from "./meeting-reprocessing-api";
  import type {
    ReprocessingMode, SelectedReprocessingPlan, ProposalState, ReprocessingResult,
  } from "./meeting-reprocessing-types";
  import { onDestroy, tick, untrack } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import TranscriptionStages from "./TranscriptionStages.svelte";
  import { initialStages, startStages, acceptRunProgress, finishStages } from "./transcribe-stages";
  import { canCancelPlainTranscription, transcribeSaveDefaults, isMissingTranscribeFileError } from "./transcribe-ui-state";
  import {
    transcribeFile, cancelFileTranscription, copyTranscriptionText, saveTranscriptionText,
    setUpdateResultPending,
    beginMeetingFile, getMeetingJob, cancelMeetingJob,
    createMeetingReview, applyMeetingCorrections, undoMeetingReview,
    resetMeetingReview, openMeetingReview, saveMeetingReview,
    attachMeetingAudio, detachMeetingAudio,
    type MeetingJobStatus, type MeetingJobSnapshot,
  } from "./api";
  import MeetingReview from "./MeetingReview.svelte";
  import type { MeetingReviewDraftSnapshot } from "./MeetingReview.svelte";
  import type {
    CorrectionOperation,
    MeetingDraftState,
    MeetingAudioAttachment,
    MeetingExportFormat,
    MeetingReview as MeetingReviewDocument,
    MeetingReviewState,
    MeetingTranscript,
  } from "./meeting-types";
  import type { UpdateRecoveryFile, UpdateRecoveryMeeting } from "./update-recovery";
  import { pollMeetingJob as pollMeetingJobClient } from "./meeting-job-client";
  import type { FileJob, FileJobStatus } from "./transcription-queue";
  let {
    job, otherBusy, active, openReview = false, initialRecoveryFile = null, initialRecoveryMeeting = null,
    onComplete, onBusyChange, onMissingFile, onAttentionChange,
    onFileRecoveryChange = () => undefined, onMeetingRecoveryChange = () => undefined,
  }: {
    job: FileJob;
    otherBusy: boolean;
    active: boolean;
    openReview?: boolean;
    initialRecoveryFile?: UpdateRecoveryFile | null;
    initialRecoveryMeeting?: UpdateRecoveryMeeting | null;
    onComplete: (id: string, status: FileJobStatus) => void;
    onBusyChange: (id: string, busy: boolean) => void;
    onMissingFile: (path: string) => void;
    onAttentionChange: (id: string, needsRetry: boolean) => void;
    onFileRecoveryChange?: (entry: UpdateRecoveryFile | null) => void;
    onMeetingRecoveryChange?: (entry: UpdateRecoveryMeeting | null) => void;
  } = $props();
  let started = $state(false);
  let starting = $state(false);
  let hydratedFileRecoveryJobId = $state<string | null>(null);
  let hydratedMeetingRecoveryJobId = $state<string | null>(null);

  $effect(() => {
    if (job.status === "running" && !started) {
      started = true;
      starting = true;
      void (openReview ? openSavedMeetingReview() : handleFileTranscription(job.path))
        .finally(() => { starting = false; });
    }
  });

  $effect(() => {
    onBusyChange(job.id, starting || transcribing || meetingReprocessingBusy || meetingReviewInit !== null || meetingPollActive);
  });

  $effect(() => { onAttentionChange(job.id, meetingPollingFailed); });

  // A polling error is recoverable: hold the queue until a terminal snapshot
  // is retrieved. Await review initialization before releasing the next file.
  $effect(() => {
    if (!started || starting || job.status !== "running" || transcribing
      || meetingReviewInit !== null || meetingPollActive || meetingPollingFailed) return;
    onComplete(job.id, meetingJobStatus === "cancelled" || plainStages.status === "cancelled" ? "cancelled"
      : transcribeError || meetingError ? "failed"
      : openReview && !meetingReview ? "cancelled" : "completed");
  });

  let transcribing: boolean = $state(false);
  let plainStages = $state(initialStages());
  let cancellingPlain = $state(false);
  let plainRunId: string | null = null;
  let plainRequestStarted = false;
  let transcribeElapsedSec = $state(0);
  let resultActionMessage = $state("");
  let transcribeResultSection: HTMLDivElement | undefined = $state();
  let saveResultButton: HTMLButtonElement | undefined = $state();

  async function revealTranscriptionResult(): Promise<void> {
    await tick();
    if (active) {
      transcribeResultSection?.scrollIntoView({ behavior: "smooth", block: "nearest" });
      saveResultButton?.focus({ preventScroll: true });
    }
  }
  let transcriptionProgress: number = $state(0);
  let transcriptionResult: string = $state("");
  let transcribeError: string = $state("");
  let meetingReview: MeetingReviewDocument | null = $state(null);
  let meetingTranscript: MeetingTranscript | null = $state(null);
  let meetingJobId: string | null = $state(null);
  let meetingResultId: string | null = $state(null);
  let meetingJobStatus: MeetingJobStatus | null = $state(null);
  let meetingPhase: string = $state("");
  let meetingError: string = $state("");
  let meetingPollingFailed: boolean = $state(false);
  let meetingPollGeneration = 0;
  let meetingPollActive = $state(false);
  let meetingDocumentRevision = $state(0);
  let meetingReviewResetKey = $state(0);
  let meetingReviewDraftDirty = $state(false);
  let meetingReprocessingPlan = $state<SelectedReprocessingPlan | null>(null);
  let meetingProposal = $state<ProposalState | null>(null);
  let meetingReprocessingResult = $state<ReprocessingResult | null>(null);
  let meetingReprocessingBusy = $state(false);
  let meetingActionQueue: Promise<void> = Promise.resolve();
  let meetingReviewInit: Promise<void> | null = $state(null);
  let meetingRecoveryDraft = $state<MeetingReviewDraftSnapshot | null>(null);

  const emptyMeetingDraft = (): MeetingDraftState => ({
    labels: {}, mergeTargets: {}, texts: {}, speakers: {},
  });

  // Recovery entries belong to a queued job. Hydrate them only when the job ID
  // matches, and mark the component as started so a recovered result cannot be
  // accidentally submitted to the transcription backend again.
  $effect(() => {
    const recovery = initialRecoveryFile;
    if (!recovery || recovery.job_id !== job.id || hydratedFileRecoveryJobId === job.id) return;
    hydratedFileRecoveryJobId = job.id;
    started = true;
    transcriptionResult = recovery.text;
    plainStages = finishStages(startStages(), "completed");
  });

  $effect(() => {
    const recovery = initialRecoveryMeeting;
    if (!recovery || recovery.job_id !== job.id || hydratedMeetingRecoveryJobId === job.id) return;
    hydratedMeetingRecoveryJobId = job.id;
    started = true;
    const generation = ++meetingPollGeneration;
    meetingRecoveryDraft = {
      source_sha256: recovery.review.review.original.source_sha256,
      review_revision: recovery.review.review.revision,
      drafts: {
        labels: { ...recovery.editor_draft.labels },
        mergeTargets: { ...recovery.editor_draft.mergeTargets },
        texts: { ...recovery.editor_draft.texts },
        speakers: { ...recovery.editor_draft.speakers },
      },
    };
    acceptMeetingReview(recovery.review, generation, true);
    meetingResultId = recovery.job_id;
    meetingJobStatus = "completed";
    meetingReprocessingPlan = null;
    meetingProposal = recovery.proposal;
    meetingReprocessingResult = null;
  });

  $effect(() => {
    if (transcriptionResult.trim()) {
      untrack(() => onFileRecoveryChange({ job_id: job.id, path: job.path, text: transcriptionResult }));
    }
  });

  $effect(() => {
    if (!meetingReview || !meetingTranscript) return;
    const entry: UpdateRecoveryMeeting = {
      job_id: job.id,
      path: job.path,
      review: { review: meetingReview, transcript: meetingTranscript },
      editor_draft: meetingRecoveryDraft?.drafts ?? emptyMeetingDraft(),
      proposal: meetingProposal,
    };
    untrack(() => onMeetingRecoveryChange(entry));
  });

  onDestroy(() => {
    meetingPollGeneration += 1;
  });

  function meetingFailureText(value: unknown, fallback: string): string {
    return typeof value === "string" ? value : value instanceof Error ? value.message : fallback;
  }

  function meetingStageText(): string {
    if (meetingJobStatus === "cancelling") return "Cancelling meeting…";
    if (meetingJobStatus === "running") return meetingPhase ? `Meeting: ${meetingPhase}` : "Starting meeting…";
    return meetingPhase || "Meeting import";
  }

  function waitForMeetingPoll(): Promise<void> {
    return new Promise((resolve) => window.setTimeout(resolve, 500));
  }

  function waitForMeetingActions(): Promise<void> {
    return meetingActionQueue;
  }

  async function setMeetingResultPending(pending: boolean): Promise<void> {
    if (meetingResultId) {
      await setUpdateResultPending(`meeting:${meetingResultId}`, pending);
    }
  }

  function enqueueMeetingAction(action: (review: MeetingReviewDocument, revision: number) => Promise<boolean | void>): Promise<boolean> {
    const queued = meetingActionQueue.then(async () => {
      const review = meetingReview;
      if (!review) return false;
      return (await action(review, meetingDocumentRevision)) !== false;
    });
    meetingActionQueue = queued.then(() => undefined, () => undefined);
    return queued;
  }

  function acceptMeetingReview(state: MeetingReviewState, generation: number, replaceDocument = false): void {
    if (generation !== meetingPollGeneration) return;
    meetingReview = state.review;
    meetingTranscript = state.transcript;
    meetingDocumentRevision += 1;
    if (replaceDocument) {
      meetingReviewResetKey += 1;
      meetingReprocessingPlan = null;
      meetingProposal = null;
      meetingReprocessingResult = null;
    }
    meetingError = "";
  }

  async function initializeMeetingReview(transcript: MeetingTranscript, generation: number): Promise<void> {
    const state = await createMeetingReview(transcript);
    acceptMeetingReview(state, generation, true);
  }

  async function pollMeetingJob(jobId: string, generation: number): Promise<void> {
    if (meetingPollActive) return;
    meetingPollActive = true;
    try {
      await pollMeetingJobClient({
        jobId,
        get: getMeetingJob,
        isCurrent: () => generation === meetingPollGeneration,
        onFailure: (error: unknown) => {
          meetingPollingFailed = true;
          meetingError = meetingFailureText(error, "Could not check meeting progress.")
            + " Retry the status check to continue.";
        },
        onSnapshot: (snapshot: MeetingJobSnapshot) => {
          if (generation !== meetingPollGeneration) return;
          meetingJobStatus = snapshot.status;
          meetingPhase = snapshot.phase;
          if (snapshot.status === "completed" || snapshot.status === "cancelled" || snapshot.status === "failed") {
            meetingJobId = null;
            transcribing = false;
            meetingPollingFailed = false;
            transcriptionProgress = 0;
            if (snapshot.status === "completed" && snapshot.reprocessing) {
              const result = snapshot.reprocessing;
              // Keep the completed work even if the separate preview request fails.
              meetingReprocessingResult = result;
              meetingReprocessingPlan = null;
              meetingProposal = null;
              meetingReviewInit = previewMeetingProposal(result.proposal)
                .then((state) => {
                  if (generation !== meetingPollGeneration) return;
                  meetingProposal = state;
                  meetingReprocessingResult = result;
                  meetingReprocessingPlan = null;
                })
                .catch((error) => {
                  if (generation === meetingPollGeneration) {
                    meetingError = meetingFailureText(error, "Could not preview the proposal. The previous review is unchanged.");
                  }
                })
                .finally(() => { if (generation === meetingPollGeneration) meetingReviewInit = null; });
            } else if (snapshot.status === "completed" && snapshot.transcript) {
              meetingReviewInit = initializeMeetingReview(snapshot.transcript, generation)
                .catch((error) => {
                  if (generation === meetingPollGeneration) {
                    meetingError = meetingFailureText(error, "Could not initialize the meeting review.");
                  }
                })
                .finally(() => {
                  if (generation === meetingPollGeneration) meetingReviewInit = null;
                });
            } else if (snapshot.status === "completed") {
              meetingError = "Meeting completed without a transcript. Try the import again.";
            } else {
              meetingError = snapshot.error
                ?? (snapshot.status === "cancelled" ? "Meeting import was cancelled." : "Meeting import failed.");
            }
          }
        },
        wait: waitForMeetingPoll,
      });
    } finally {
      meetingPollActive = false;
    }
  }

  async function startMeetingFileTranscription(
    filePath: string,
    prompt: string | null,
    profileId: string | null,
  ): Promise<void> {
    if (transcribing) return;
    if (meetingReview && !window.confirm("Start a new meeting review and replace the current review if it completes?")) return;
    if (meetingResultId) {
      try {
        await setMeetingResultPending(false);
      } catch (error) {
        meetingError = meetingFailureText(error, "Could not preserve the current meeting result.");
        return;
      }
    }
    meetingResultId = null;
    const generation = ++meetingPollGeneration;
    // Keep the previous review and its unsaved drafts mounted until a NEW
    // document succeeds. Pending edits finish before the import starts below;
    // successful replacement still invalidates any stale action revision.
    transcribing = true;
    transcriptionProgress = 0;
    transcribeError = "";
    transcriptionResult = "";
    meetingError = "";
    meetingPollingFailed = false;
    meetingJobId = null;
    meetingJobStatus = "running";
    meetingPhase = "Starting";
    try {
      await waitForMeetingActions();
      if (generation !== meetingPollGeneration) return;
      const jobId = await beginMeetingFile(filePath, prompt, profileId);
      if (generation !== meetingPollGeneration) return;
      if (!jobId) throw new Error("Meeting import did not return a job ID.");
      meetingJobId = jobId;
      meetingResultId = jobId;
      meetingJobStatus = "running";
      void pollMeetingJob(jobId, generation);
    } catch (error) {
      if (generation !== meetingPollGeneration) return;
      transcribing = false;
      meetingJobId = null;
      meetingJobStatus = "failed";
      meetingPhase = "Failed";
      meetingError = meetingFailureText(error, "Could not start meeting import.");
    }
  }

  async function handleFileTranscription(filePath: string) {
    if (transcribing) return;
    const profileId = job.profileId;
    const prompt = job.prompt;
    if (job.diarize) {
      await startMeetingFileTranscription(filePath, prompt, profileId);
      return;
    }

    ++meetingPollGeneration;
    ++meetingDocumentRevision;
    meetingError = "";
    meetingJobStatus = null;
    transcribing = true;
    transcriptionProgress = 0;
    transcribeError = "";
    transcriptionResult = "";
    plainRunId = job.id;
    plainRequestStarted = false;
    cancellingPlain = false;
    plainStages = startStages();
    resultActionMessage = "";
    const startedAt = Date.now();
    // Elapsed time is feedback, never measured compute progress.
    const timer = setInterval(() => { transcribeElapsedSec = Math.floor((Date.now() - startedAt) / 1000); }, 500);
    let stopProgress: (() => void) | undefined;
    try {
      await waitForMeetingActions();
      // Register before invoking native work so even the first event is scoped.
      stopProgress = await listen("plain-transcription-progress", (event) => {
        plainStages = acceptRunProgress(plainStages, plainRunId, event.payload);
      });
      if (cancellingPlain) {
        plainStages = finishStages(plainStages, "cancelled");
        transcribeError = "Transcription was cancelled.";
        return;
      }
      plainRequestStarted = true;
      transcriptionResult = await transcribeFile(filePath, {
        prompt: prompt ?? undefined,
        diarize: false,
        profileId: profileId ?? undefined,
        runId: plainRunId,
      });
      if (cancellingPlain) resultActionMessage = "Finished before Stop took effect.";
      transcribeError = "";
      plainStages = finishStages(plainStages, "completed");
      if (transcriptionResult.trim()) void revealTranscriptionResult();
    } catch (error: any) {
      if (isMissingTranscribeFileError(error)) onMissingFile(filePath);
      plainStages = finishStages(plainStages, cancellingPlain ? "cancelled" : "failed");
      transcribeError = cancellingPlain ? "Transcription was cancelled." : meetingFailureText(error, "Transcription failed");
    } finally {
      stopProgress?.();
      clearInterval(timer);
      plainRunId = null;
      plainRequestStarted = false;
      cancellingPlain = false;
      transcribing = false;
      transcriptionProgress = 0;
    }
  }

  async function cancelPlainTranscription(): Promise<void> {
    if (!canCancelPlainTranscription({ transcribing, meetingJobStatus, cancellingPlain })) return;
    cancellingPlain = true;
    if (!plainRequestStarted || !plainRunId) return;
    const runId = plainRunId;
    try {
      await cancelFileTranscription(runId);
      if (plainRunId !== runId) return;
      // Backend success remains authoritative if Stop lost the race.
    } catch (error) {
      if (plainRunId !== runId) return;
      if (transcribing && !transcribeError) transcribeError = meetingFailureText(error, "Could not request cancellation. The transcription is still running.");
      cancellingPlain = false;
    }
  }

  async function copyTranscriptionResult(): Promise<void> {
    if (transcribing || !transcriptionResult) return;
    try {
      await copyTranscriptionText(transcriptionResult);
      await setUpdateResultPending(job.id, false);
      onFileRecoveryChange(null);
      resultActionMessage = "Copied to clipboard.";
    } catch (error) { resultActionMessage = meetingFailureText(error, "Could not copy."); }
  }

  async function saveTranscriptionResult(): Promise<void> {
    if (transcribing || !transcriptionResult) return;
    const { fileName, directory } = transcribeSaveDefaults(job.path);
    try {
      const saved = await saveTranscriptionText(transcriptionResult, fileName, directory);
      if (saved) {
        await setUpdateResultPending(job.id, false);
        onFileRecoveryChange(null);
      }
      resultActionMessage = saved ? "Saved." : "Save cancelled — nothing was written.";
    } catch (error) { resultActionMessage = meetingFailureText(error, "Could not save."); }
  }

  async function cancelMeetingImport(): Promise<void> {
    const jobId = meetingJobId;
    const generation = meetingPollGeneration;
    if (!jobId || meetingJobStatus === "cancelling") return;
    try {
      const accepted = await cancelMeetingJob(jobId);
      if (generation !== meetingPollGeneration || meetingJobId !== jobId) return;
      if (accepted) {
        meetingJobStatus = "cancelling";
        meetingPhase = "Cancelling";
        meetingError = "";
      } else {
        meetingError = "The meeting has already finished. Its final status is being retrieved.";
      }
    } catch (error) {
      if (generation !== meetingPollGeneration || meetingJobId !== jobId) return;
      meetingError = meetingFailureText(error, "Could not request cancellation. The meeting is still running.");
    }
  }

  function retryMeetingPolling(): void {
    if (!meetingJobId || meetingPollActive) return;
    meetingPollingFailed = false;
    meetingError = "";
    transcribing = true;
    void pollMeetingJob(meetingJobId, meetingPollGeneration);
  }

  async function renameMeetingReviewSpeaker(id: string, label: string): Promise<void> {
    await applyMeetingReviewOperations([{ kind: "rename_speaker", speaker_id: id, label }]);
  }

  async function mergeMeetingReviewSpeakers(fromId: string, intoId: string): Promise<void> {
    await applyMeetingReviewOperations([{ kind: "merge_speakers", from_id: fromId, into_id: intoId }]);
  }

  async function exportMeetingReview(format: MeetingExportFormat): Promise<boolean> {
    const saved = await enqueueMeetingAction((review) => saveMeetingReview(review, format));
    if (saved) {
      await setMeetingResultPending(false);
      onMeetingRecoveryChange(null);
    }
    return saved;
  }

  async function applyMeetingReviewOperations(operations: CorrectionOperation[]): Promise<void> {
    await enqueueMeetingAction(async (review, revision) => {
      const corrections = {
        schema_version: 1,
        source_sha256: review.original.source_sha256,
        original_revision: review.original_revision,
        expected_revision: review.revision,
        operations,
      };
      const state = await applyMeetingCorrections(review, corrections);
      if (revision === meetingDocumentRevision) acceptMeetingReview(state, meetingPollGeneration);
    });
  }

  async function undoMeetingReviewChanges(): Promise<void> {
    await enqueueMeetingAction(async (review, revision) => {
      const state = await undoMeetingReview(review, review.revision);
      if (revision === meetingDocumentRevision) acceptMeetingReview(state, meetingPollGeneration);
    });
  }

  async function resetMeetingReviewChanges(): Promise<void> {
    await enqueueMeetingAction(async (review, revision) => {
      const state = await resetMeetingReview(review, review.revision);
      if (revision === meetingDocumentRevision) acceptMeetingReview(state, meetingPollGeneration);
    });
  }

  async function saveCurrentMeetingReview(): Promise<boolean> {
    const saved = await enqueueMeetingAction((review) => saveMeetingReview(review, "json"));
    if (saved) {
      await setMeetingResultPending(false);
      onMeetingRecoveryChange(null);
    }
    return saved;
  }

  async function attachCurrentMeetingAudio(): Promise<MeetingAudioAttachment | null> {
    const review = meetingReview;
    const transcript = meetingTranscript;
    const generation = meetingPollGeneration;
    const resetKey = meetingReviewResetKey;
    if (!review || !transcript) return null;
    const attachment = await attachMeetingAudio(transcript.source_sha256);
    const stillCurrent =
      generation === meetingPollGeneration
      && resetKey === meetingReviewResetKey
      && meetingReview?.original_revision === review.original_revision
      && meetingTranscript?.source_sha256 === transcript.source_sha256;
    if (!stillCurrent) {
      if (attachment) await detachMeetingAudio(attachment.token).catch(() => undefined);
      return null;
    }
    return attachment;
  }

  function onMeetingReviewDraftDirtyChange(dirty: boolean): void {
    meetingReviewDraftDirty = dirty;
    if (dirty) void setMeetingResultPending(true);
  }

  function onMeetingReviewDraftSnapshotChange(snapshot: MeetingReviewDraftSnapshot): void {
    meetingRecoveryDraft = snapshot;
    if (!meetingReview || !meetingTranscript) return;
    const review = meetingReview;
    const transcript = meetingTranscript;
    untrack(() => onMeetingRecoveryChange({
      job_id: job.id,
      path: job.path,
      review: { review, transcript },
      editor_draft: snapshot.drafts,
      proposal: meetingProposal,
    }));
  }

  async function detachCurrentMeetingAudio(token: string): Promise<void> {
    await detachMeetingAudio(token);
  }

  async function openSavedMeetingReview(): Promise<void> {
    if (transcribing) return;
    if (meetingReview && !window.confirm("Open a saved review and replace the current review if it succeeds?")) return;
    const generation = ++meetingPollGeneration;
    meetingError = "";
    try {
      await waitForMeetingActions();
      const state = await openMeetingReview();
      if (generation !== meetingPollGeneration || !state) return;
      acceptMeetingReview(state, generation, true);
      if (meetingResultId) await setMeetingResultPending(false);
      meetingResultId = null;
    } catch (error) {
      if (generation === meetingPollGeneration) meetingError = meetingFailureText(error, "Could not open the meeting review.");
    }
  }

  async function planCurrentMeeting(mode: ReprocessingMode, threshold: number, saveCache: boolean): Promise<void> {
    if (otherBusy || transcribing || meetingReprocessingBusy || meetingReviewDraftDirty) return;
    meetingReprocessingBusy = true;
    try {
      await enqueueMeetingAction(async (review, revision) => {
        const generation = meetingPollGeneration;
        const selected = await planMeetingReprocessing(review, mode, threshold, saveCache,
          job.prompt, job.profileId);
        if (selected && revision === meetingDocumentRevision && generation === meetingPollGeneration) {
          meetingReprocessingPlan = selected;
        }
      });
    } finally { meetingReprocessingBusy = false; }
  }

  async function executeCurrentMeetingPlan(): Promise<void> {
    if (otherBusy || transcribing || meetingReprocessingBusy || meetingReviewDraftDirty || !meetingReprocessingPlan) return;
    await waitForMeetingActions();
    const selected = meetingReprocessingPlan;
    const review = meetingReview;
    if (!review || !selected || otherBusy || transcribing || meetingReprocessingBusy || meetingReviewDraftDirty) return;
    if (selected.plan.context.previous_revision !== review.revision) {
      throw new Error("The review changed. Prepare a new plan; your corrections have been kept.");
    }
    const generation = ++meetingPollGeneration;
    transcribing = true;
    meetingError = "";
    meetingPollingFailed = false;
    meetingJobStatus = "running";
    meetingPhase = "Preparing reprocessing";
    try {
      const id = await beginMeetingReprocessing(selected, review,
        job.prompt, job.profileId);
      if (!id) throw new Error("Reprocessing did not return a job ID.");
      meetingJobId = id;
      meetingResultId = id;
      void pollMeetingJob(id, generation);
    } catch (error) {
      transcribing = false;
      meetingJobId = null;
      meetingJobStatus = "failed";
      meetingError = meetingFailureText(error, "Reprocessing failed; the previous review is unchanged.");
      throw error;
    }
  }

  async function resolveCurrentMeetingProposal(index: number, operations: CorrectionOperation[]): Promise<void> {
    const state = meetingProposal;
    if (!state || transcribing || meetingReprocessingBusy) return;
    meetingReprocessingBusy = true;
    try {
      const next = await resolveMeetingProposal(state.proposal, index, operations);
      if (meetingProposal?.proposal.revision === state.proposal.revision) meetingProposal = next;
    } finally { meetingReprocessingBusy = false; }
  }

  async function acceptCurrentMeetingProposal(): Promise<void> {
    if (otherBusy || transcribing || meetingReprocessingBusy || meetingReviewDraftDirty) return;
    meetingReprocessingBusy = true;
    try {
      await enqueueMeetingAction(async (review, revision) => {
        const proposal = meetingProposal;
        const generation = meetingPollGeneration;
        if (!proposal || meetingReviewDraftDirty) return false;
        const state = await acceptMeetingProposal(proposal.proposal, review);
        if (revision !== meetingDocumentRevision || generation !== meetingPollGeneration
          || meetingReviewDraftDirty || proposal.proposal.revision !== meetingProposal?.proposal.revision) {
          throw new Error("The review changed before acceptance. It has not been replaced.");
        }
        acceptMeetingReview(state, generation, true);
      });
    } finally { meetingReprocessingBusy = false; }
  }

  async function saveCurrentMeetingProposal(): Promise<boolean> {
    const proposal = meetingProposal?.proposal ?? meetingReprocessingResult?.proposal;
    const saved = proposal ? await saveMeetingProposal(proposal) : false;
    if (saved) await setMeetingResultPending(false);
    return saved;
  }

  async function retryCurrentMeetingProposalPreview(): Promise<void> {
    const result = meetingReprocessingResult;
    if (!result || meetingProposal || transcribing || meetingReprocessingBusy || meetingReviewInit) return;
    const generation = meetingPollGeneration;
    meetingReprocessingBusy = true;
    meetingError = "";
    try {
      const state = await previewMeetingProposal(result.proposal);
      if (generation === meetingPollGeneration && meetingReprocessingResult === result) meetingProposal = state;
    } catch (error) {
      meetingError = meetingFailureText(error, "Could not preview the proposal. Save it to retry later; the previous review is unchanged.");
    } finally { meetingReprocessingBusy = false; }
  }

  async function openCurrentMeetingProposal(): Promise<void> {
    if (otherBusy || transcribing || meetingReprocessingBusy) return;
    meetingReprocessingBusy = true;
    const generation = meetingPollGeneration;
    try {
      const state = await openMeetingProposal();
      if (state && generation === meetingPollGeneration) {
        meetingProposal = state;
        meetingReprocessingResult = null;
        meetingReprocessingPlan = null;
      }
    } finally { meetingReprocessingBusy = false; }
  }

</script>
{#if !job.diarize && !openReview}
  <TranscriptionStages state={plainStages} cancelling={cancellingPlain} />
{/if}
{#if job.status === "queued"}
  <p role="status">Queued — waiting for the previous file.</p>
{:else if job.status === "completed" && !transcriptionResult && !meetingTranscript}
  <p role="status">Completed — no speech detected.</p>
{:else if job.status === "cancelled" && !meetingError}
  <p role="status">Cancelled.</p>
{/if}
          {#if transcribing}
            {#if meetingJobStatus !== null}
              <div class="spinner"></div>
              <div class="drop-zone-text">{meetingStageText()}</div>
              {#if meetingJobId && meetingPollingFailed}
                <button class="secondary" onclick={retryMeetingPolling}>Retry status check</button>
              {:else if meetingJobId}
                <button
                  class="secondary"
                  onclick={cancelMeetingImport}
                  disabled={meetingJobStatus === "cancelling"}
                >
                  {meetingJobStatus === "cancelling" ? "Cancelling…" : "Cancel meeting"}
                </button>
              {/if}
            {:else}
              <div class="drop-zone-text">Elapsed: {transcribeElapsedSec}s</div>
              <button class="secondary" onclick={() => void cancelPlainTranscription()} disabled={cancellingPlain}>
                {cancellingPlain ? "Cancelling…" : "Stop transcription"}
              </button>
            {/if}
          {/if}
        {#if transcribeError}
          <div class="transcribe-error">{transcribeError}</div>
        {/if}

        {#if meetingError && !meetingTranscript}
          <div class="transcribe-error">{meetingError}</div>
        {/if}

        {#if transcriptionResult}
          <div bind:this={transcribeResultSection}>
          <div class="result-label">Result</div>
          <textarea class="transcribe-result" readonly>{transcriptionResult}</textarea>
          <div class="result-actions">
            <button class="secondary" onclick={() => void copyTranscriptionResult()} disabled={transcribing}>Copy</button>
            <button class="secondary" bind:this={saveResultButton} onclick={() => void saveTranscriptionResult()} disabled={transcribing}
              title="Choose where to save — defaults to the audio file's folder">Save…</button>
            {#if resultActionMessage}<span role="status">{resultActionMessage}</span>{/if}
          </div>
          </div>
        {/if}

        {#if meetingReview && meetingTranscript}
            {#if meetingReprocessingResult && !meetingProposal}
              <div role="status">
                <p>The completed proposal is retained. Retry its preview or save it to open later.</p>
                <button class="btn btn-secondary" disabled={otherBusy || transcribing || meetingReprocessingBusy || meetingReviewInit !== null}
                  onclick={() => void retryCurrentMeetingProposalPreview()}>Retry proposal preview</button>
                <button class="btn btn-secondary" disabled={otherBusy || transcribing || meetingReprocessingBusy || meetingReviewInit !== null}
                  onclick={() => void saveCurrentMeetingProposal().catch((error) => { meetingError = meetingFailureText(error, "Could not save the proposal."); })}>Save retained proposal</button>
              </div>
            {/if}
            <MeetingReprocessing
              idPrefix={job.id + "-"}
              currentReviewRevision={meetingReview.revision}
              busy={otherBusy || transcribing || meetingReprocessingBusy || meetingReviewInit !== null}
              draftDirty={meetingReviewDraftDirty}
              selected={meetingReprocessingPlan}
              proposal={meetingProposal}
              result={meetingReprocessingResult}
              onPlan={planCurrentMeeting}
              onExecute={executeCurrentMeetingPlan}
              onResolve={resolveCurrentMeetingProposal}
              onAccept={acceptCurrentMeetingProposal}
              onSave={saveCurrentMeetingProposal}
              onOpen={openCurrentMeetingProposal}
              onDiscard={() => { meetingProposal = null; meetingReprocessingResult = null; meetingReprocessingPlan = null; }}
            />
            <MeetingReview
              {active} idPrefix={job.id + "-"}
              review={meetingReview}
              transcript={meetingTranscript}
              busy={transcribing || meetingReprocessingBusy || meetingReviewInit !== null}
              error={meetingError || null}
              onApply={applyMeetingReviewOperations}
              onUndo={undoMeetingReviewChanges}
              onReset={resetMeetingReviewChanges}
              onSave={saveCurrentMeetingReview}
              onExport={exportMeetingReview}
              onAttachAudio={attachCurrentMeetingAudio}
              onDetachAudio={detachCurrentMeetingAudio}
              onDraftDirtyChange={onMeetingReviewDraftDirtyChange}
              initialDraftSnapshot={meetingRecoveryDraft}
              onDraftSnapshotChange={onMeetingReviewDraftSnapshotChange}
              resetDraftKey={meetingReviewResetKey}
            />
        {/if}

<style>
  .transcribe-error {
    margin-top: 12px;
    padding: 10px 14px;
    background: color-mix(in srgb, var(--danger) 12%, var(--bg));
    border: 1px solid var(--danger);
    border-radius: var(--radius);
    color: var(--danger);
    font-size: 12px;
  }

  .result-label {
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--text-muted);
    margin-top: 14px;
    margin-bottom: 6px;
    font-weight: 600;
  }

  .transcribe-result {
    width: 100%;
    min-height: 100px;
    max-height: 180px;
    padding: 10px 12px;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    color: var(--text);
    font-family: inherit;
    font-size: 13px;
    line-height: 1.5;
    resize: vertical;
    outline: none;
  }

  .transcribe-result:focus {
    border-color: var(--accent);
  }


  .drop-zone-text { font-size: 13px; color: var(--text-muted); margin: 10px 0; }
  .result-actions { display: flex; align-items: center; gap: 8px; font-size: 12px; }
  .spinner { width: 24px; height: 24px; border: 3px solid var(--border); border-top-color: var(--accent); border-radius: 50%; animation: spin 0.8s linear infinite; }
  @keyframes spin { to { transform: rotate(360deg); } }
  button { margin: 6px 4px 6px 0; padding: 7px 12px; background: var(--bg-secondary); color: var(--text); border: 1px solid var(--border); border-radius: var(--radius); cursor: pointer; }
  button:disabled { opacity: 0.5; cursor: default; }
</style>
