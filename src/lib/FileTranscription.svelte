<script lang="ts">
  import { drainUpdateWork } from "./update-preparation";
  import { backendErrorRecovery, downloadFailureRecovery, isBackendError, toErrorMessage, type BackendErrorRecovery } from "./errors";
  import ErrorRecovery from "./ErrorRecovery.svelte";
  import MeetingReprocessing from "./MeetingReprocessing.svelte";
  import { onDestroy, tick, untrack } from "svelte";
  import { subscribeTauriEvent } from "./tauri-events.svelte";
  import { createPlainTranscriptionRun } from "./plain-transcription-run";
  import TranscriptionStages from "./TranscriptionStages.svelte";
  import MeetingProgress from "./MeetingProgress.svelte";
  import { initialStages, startStages, acceptRunProgress, finishStages } from "./transcribe-stages";
  import { canCancelPlainTranscription, transcribeSaveDefaults, isMissingTranscribeFileError } from "./transcribe-ui-state";
  import {
    transcribeFile, cancelFileTranscription, copyTranscriptionText, saveTranscriptionText,
    setUpdateResultPending,
  } from "./api";
  import MeetingReview from "./MeetingReview.svelte";
  import type { UpdateRecoveryFile, UpdateRecoveryMeeting } from "./update-recovery";
  import type { FileJob, FileJobStatus } from "./transcription-queue";
  import { MeetingImport } from "./file-transcription/meetingImport.svelte";
  let {
    job, otherBusy, active, openReview = false, initialRecoveryFile = null, initialRecoveryMeeting = null,
    onComplete, onBusyChange, onMissingFile, onRetry, retryRefusal, onDownloadModel, downloadBusy, updatePreparing, onAttentionChange,
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
    /** Run this job's file again as a new job ("Try again"). False when nothing was queued. */
    onRetry: (job: FileJob) => boolean;
    /** Why the last "Try again" was refused, shown beside the button; empty when it was not. */
    retryRefusal: string;
    /** Download the engine's model again; rejects when the download fails. */
    onDownloadModel: () => Promise<void>;
    /** A download is running, so another cannot start. */
    downloadBusy: boolean;
    /** An update is being prepared: nothing new starts (the panel is inert meanwhile). */
    updatePreparing: boolean;
    onAttentionChange: (id: string, needsRetry: boolean) => void;
    onFileRecoveryChange?: (entry: UpdateRecoveryFile | null) => void;
    onMeetingRecoveryChange?: (entry: UpdateRecoveryMeeting | null) => void;
  } = $props();
  let started = $state(false);
  let starting = $state(false);
  let hydratedFileRecoveryJobId = $state<string | null>(null);
  // The meeting import reads the props through getters, so it always sees
  // their current values, and shares the run state that stays here.
  const meeting = new MeetingImport({
    get job() { return job; },
    get otherBusy() { return otherBusy; },
    get initialRecoveryMeeting() { return initialRecoveryMeeting; },
    get onMeetingRecoveryChange() { return onMeetingRecoveryChange; },
    get transcribing() { return transcribing; },
    set transcribing(value) { transcribing = value; },
    get started() { return started; },
    set started(value) { started = value; },
    get transcribeError() { return transcribeError; },
    set transcribeError(value) { transcribeError = value; },
    get transcribeRecovery() { return transcribeRecovery; },
    set transcribeRecovery(value) { transcribeRecovery = value; },
    get transcriptionResult() { return transcriptionResult; },
    set transcriptionResult(value) { transcriptionResult = value; },
  });

  $effect(() => {
    if (job.status === "running" && !started) {
      started = true;
      starting = true;
      void (openReview ? meeting.openSavedMeetingReview() : handleFileTranscription(job.path))
        .finally(() => { starting = false; });
    }
  });

  $effect(() => {
    onBusyChange(job.id, starting || transcribing || meeting.meetingReprocessingBusy || meeting.meetingReviewInit !== null || meeting.meetingPollActive);
  });

  $effect(() => { onAttentionChange(job.id, meeting.meetingPollingFailed); });

  // A polling error is recoverable: hold the queue until a terminal snapshot
  // is retrieved. Await review initialization before releasing the next file.
  $effect(() => {
    if (!started || starting || job.status !== "running" || transcribing
      || meeting.meetingReviewInit !== null || meeting.meetingPollActive || meeting.meetingPollingFailed) return;
    onComplete(job.id, meeting.meetingJobStatus === "cancelled" || plainStages.status === "cancelled" ? "cancelled"
      : transcribeError || meeting.meetingError ? "failed"
      : openReview && !meeting.meetingReview ? "cancelled" : "completed");
  });

  let transcribing: boolean = $state(false);
  let plainStages = $state(initialStages());
  let cancellingPlain = $state(false);
  let plainTimer: ReturnType<typeof setInterval> | undefined;
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
  let transcriptionResult: string = $state("");
  let transcribeError: string = $state("");
  /** The way out of `transcribeError`, decided by the error's code (`backendErrorRecovery`). */
  let transcribeRecovery: BackendErrorRecovery | null = $state(null);
  const plainTranscriptionRun = createPlainTranscriptionRun({
    waitForMeetingActions: meeting.waitForMeetingActions,
    subscribeProgress: (handler) => subscribeTauriEvent("plain-transcription-progress", handler),
    transcribeFile,
    cancelFileTranscription,
    onStart: () => {
      ++meeting.meetingPollGeneration;
      ++meeting.meetingDocumentRevision;
      meeting.meetingError = "";
      meeting.meetingJobStatus = null;
      transcribing = true;

      transcribeError = "";
      transcribeRecovery = null;
      transcriptionResult = "";
      cancellingPlain = false;
      plainStages = startStages();
      resultActionMessage = "";
      const startedAt = Date.now();
      // Elapsed time is feedback, never measured compute progress.
      plainTimer = setInterval(() => { transcribeElapsedSec = Math.floor((Date.now() - startedAt) / 1000); }, 500);
    },
    onProgress: (payload) => {
      plainStages = acceptRunProgress(plainStages, payload.runId, payload);
    },
    onCancelledBeforeSubmission: () => {
      plainStages = finishStages(plainStages, "cancelled");
      transcribeError = "Transcription was cancelled.";
    },
    onResult: (text, cancellationRequested) => {
      transcriptionResult = text;
      if (cancellationRequested) resultActionMessage = "Finished before Stop took effect.";
      transcribeError = "";
      plainStages = finishStages(plainStages, "completed");
      if (transcriptionResult.trim()) void revealTranscriptionResult();
    },
    onError: (error, cancellationRequested, path) => {
      if (isMissingTranscribeFileError(error)) onMissingFile(path);
      // Stop was pressed, or the backend ended the run as cancelled by itself
      // (ADR 0006): a cancelled run, not a failure.
      const cancelled = cancellationRequested || isBackendError(error, "cancelled");
      plainStages = finishStages(plainStages, cancelled ? "cancelled" : "failed");
      transcribeError = cancelled ? "Transcription was cancelled." : toErrorMessage(error, "Transcription failed");
      transcribeRecovery = cancelled ? null : backendErrorRecovery(error);
    },
    onFinished: () => {
      if (plainTimer !== undefined) clearInterval(plainTimer);
      plainTimer = undefined;
      cancellingPlain = false;
      transcribing = false;

    },
    onCancelRequested: () => { cancellingPlain = true; },
    onCancelError: (error) => {
      if (transcribing && !transcribeError) transcribeError = toErrorMessage(error, "Could not request cancellation. The transcription is still running.");
      cancellingPlain = false;
    },
  });
  let modelDownloading = $state(false);

  // Native work can finish before its IPC result or terminal meeting poll
  // reaches this component. Drain those deliveries and pending edits before
  // Settings snapshots the recovery entries emitted by our effects.
  export async function prepareUpdateRecovery(): Promise<void> {
    await drainUpdateWork({
      busy: () => starting || transcribing || meeting.meetingPollActive
        || meeting.meetingReviewInit !== null || meeting.meetingReprocessingBusy,
      settle: async () => {
        await meeting.meetingActionQueue;
        await meeting.meetingReviewInit;
        await tick();
      },
      failure: () => meeting.meetingPollingFailed
        ? "Retry the meeting status check before installing the update."
        : meeting.meetingJobStatus === "completed" && meeting.meetingReviewInit === null
          && (meeting.meetingError || !meeting.meetingReview || !meeting.meetingTranscript
            || (meeting.meetingReprocessingResult !== null && meeting.meetingProposal === null))
          ? "Finish or retry the meeting review before installing the update."
          : null,
    });
  }

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

  $effect(() => { meeting.hydrateRecovery(); });

  $effect(() => {
    if (transcriptionResult.trim()) {
      untrack(() => onFileRecoveryChange({ job_id: job.id, path: job.path, text: transcriptionResult }));
    }
  });

  $effect(() => { meeting.reportRecovery(); });

  onDestroy(() => { meeting.destroy(); });

  async function handleFileTranscription(filePath: string) {
    if (transcribing) return;
    const profileId = job.profileId;
    const prompt = job.prompt;
    if (job.diarize) {
      await meeting.startMeetingFileTranscription(filePath, prompt, profileId);
      return;
    }

    return plainTranscriptionRun.start({
      path: filePath,
      prompt: prompt ?? undefined,
      profileId: profileId ?? undefined,
      runId: job.id,
    });
  }

  /** "Try again": the file runs again as a new job; this one stays as it failed. */
  function retryFile(): void {
    if (onRetry(job)) transcribeRecovery = null;
  }

  /** "Download the model again"; when it is there, the run can be repeated. */
  async function downloadModelAgain(): Promise<void> {
    if (modelDownloading || downloadBusy) return;
    modelDownloading = true;
    try {
      await onDownloadModel();
      transcribeRecovery = "retry";
    } catch (failure) {
      transcribeError = toErrorMessage(failure, "Speech engine download failed.");
      // The download again, only where it can pass; a refusal offers nothing.
      transcribeRecovery = downloadFailureRecovery(failure);
    } finally {
      modelDownloading = false;
    }
  }

  async function cancelPlainTranscription(): Promise<void> {
    if (!canCancelPlainTranscription({ transcribing, meetingJobStatus: meeting.meetingJobStatus, cancellingPlain })) return;
    await plainTranscriptionRun.cancel();
  }

  async function copyTranscriptionResult(): Promise<void> {
    if (transcribing || !transcriptionResult) return;
    try {
      await copyTranscriptionText(transcriptionResult);
      await setUpdateResultPending(job.id, false);
      onFileRecoveryChange(null);
      resultActionMessage = "Copied to clipboard.";
    } catch (error) { resultActionMessage = toErrorMessage(error, "Could not copy."); }
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
    } catch (error) { resultActionMessage = toErrorMessage(error, "Could not save."); }
  }

</script>
{#if !job.diarize && !openReview}
  <TranscriptionStages state={plainStages} cancelling={cancellingPlain} />
{/if}
{#if job.status === "queued"}
  <p role="status">Queued — waiting for the previous file.</p>
{:else if job.status === "completed" && !transcriptionResult && !meeting.meetingTranscript}
  <p role="status">Completed — no speech detected.</p>
{:else if job.status === "cancelled" && !meeting.meetingError}
  <p role="status">Cancelled.</p>
{/if}
          {#if meeting.meetingJobStatus !== null && (transcribing || meeting.meetingTimingAvailable)}
            <MeetingProgress status={meeting.meetingJobStatus} phase={meeting.meetingPhase}
              elapsedMs={meeting.meetingElapsedMs} phaseElapsedMs={meeting.meetingPhaseElapsedMs}
              lastCheckedAgoMs={meeting.meetingLastCheckedAgoMs} pollingFailed={meeting.meetingPollingFailed} />
          {/if}
          {#if transcribing}
            {#if meeting.meetingJobStatus !== null}
              {#if meeting.meetingJobId && meeting.meetingPollingFailed}
                <button class="secondary" onclick={meeting.retryMeetingPolling}>Retry status check</button>
              {:else if meeting.meetingJobId}
                <button
                  class="secondary"
                  onclick={meeting.cancelMeetingImport}
                  disabled={meeting.meetingJobStatus === "cancelling"}
                >
                  {meeting.meetingJobStatus === "cancelling" ? "Cancelling…" : "Cancel meeting"}
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
          <div class="transcribe-error" role="alert">{transcribeError}</div>
          <ErrorRecovery recovery={transcribeRecovery} busy={modelDownloading || downloadBusy || updatePreparing}
            onRetry={retryFile} onDownloadModel={() => void downloadModelAgain()} />
          {#if retryRefusal}<div class="transcribe-error" role="alert">{retryRefusal}</div>{/if}
        {/if}

        {#if meeting.meetingError && !meeting.meetingTranscript}
          <div class="transcribe-error">{meeting.meetingError}</div>
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

        {#if meeting.meetingReview && meeting.meetingTranscript}
            {#if meeting.meetingReprocessingResult && !meeting.meetingProposal}
              <div role="status">
                <p>The completed proposal is retained. Retry its preview or save it to open later.</p>
                <button class="btn btn-secondary" disabled={otherBusy || transcribing || meeting.meetingReprocessingBusy || meeting.meetingReviewInit !== null}
                  onclick={() => void meeting.retryCurrentMeetingProposalPreview()}>Retry proposal preview</button>
                <button class="btn btn-secondary" disabled={otherBusy || transcribing || meeting.meetingReprocessingBusy || meeting.meetingReviewInit !== null}
                  onclick={() => void meeting.saveCurrentMeetingProposal().catch((error) => { meeting.meetingError = toErrorMessage(error, "Could not save the proposal."); })}>Save retained proposal</button>
              </div>
            {/if}
            <MeetingReprocessing
              idPrefix={job.id + "-"}
              currentReviewRevision={meeting.meetingReview.revision}
              busy={otherBusy || transcribing || meeting.meetingReprocessingBusy || meeting.meetingReviewInit !== null}
              draftDirty={meeting.meetingReviewDraftDirty}
              selected={meeting.meetingReprocessingPlan}
              proposal={meeting.meetingProposal}
              result={meeting.meetingReprocessingResult}
              onPlan={meeting.planCurrentMeeting}
              onExecute={meeting.executeCurrentMeetingPlan}
              onResolve={meeting.resolveCurrentMeetingProposal}
              onAccept={meeting.acceptCurrentMeetingProposal}
              onSave={meeting.saveCurrentMeetingProposal}
              onOpen={meeting.openCurrentMeetingProposal}
              onDiscard={() => { meeting.meetingProposal = null; meeting.meetingReprocessingResult = null; meeting.meetingReprocessingPlan = null; }}
            />
            <MeetingReview
              {active} idPrefix={job.id + "-"}
              review={meeting.meetingReview}
              transcript={meeting.meetingTranscript}
              busy={transcribing || meeting.meetingReprocessingBusy || meeting.meetingReviewInit !== null}
              error={meeting.meetingError || null}
              onApply={meeting.applyMeetingReviewOperations}
              onUndo={meeting.undoMeetingReviewChanges}
              onReset={meeting.resetMeetingReviewChanges}
              onSave={meeting.saveCurrentMeetingReview}
              onExport={meeting.exportMeetingReview}
              onAttachAudio={meeting.attachCurrentMeetingAudio}
              onDetachAudio={meeting.detachCurrentMeetingAudio}
              onDraftDirtyChange={meeting.onMeetingReviewDraftDirtyChange}
              initialDraftSnapshot={meeting.meetingRecoveryDraft}
              onDraftSnapshotChange={meeting.onMeetingReviewDraftSnapshotChange}
              resetDraftKey={meeting.meetingReviewResetKey}
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
  button { margin: 6px 4px 6px 0; padding: 7px 12px; background: var(--bg-secondary); color: var(--text); border: 1px solid var(--border); border-radius: var(--radius); cursor: pointer; }
  button:disabled { opacity: 0.5; cursor: default; }
</style>
