// Update recovery for the Settings window (SAGA-379). An update restarts the
// app, so before it installs the updater sends `update-preparing`: the
// controller then stores the unsaved dictation, file results and meeting
// reviews in update-recovery.json and acknowledges the request. After the
// restart it reads that file back and reopens every entry as a draft, which
// leaves the file again once its result is copied, saved or discarded.
//
// Every write of the file goes through one queue (`#enqueueMutation`), so a
// cleanup, a snapshot and a discard never interleave, and each rereads the
// file before it changes it.
//
// The controller owns no view. It reads and writes the Dictate tab's editor
// and the Transcribe tab's queue through the two ports below, and reaches the
// backend through the injected `api`.
import { tick } from "svelte";
import type * as backend from "../api";
import { toErrorMessage } from "../errors";
import type { FileJob } from "../transcription-queue";
import { drainUpdateWork, UpdatePreparationTimeoutError } from "../update-preparation";
import {
  createUpdateRecoveryPayload,
  readPersistedUpdateRecoveryPayload,
  serializeUpdateRecoveryPayload,
  type UpdateRecoveryFile,
  type UpdateRecoveryMeeting,
} from "../update-recovery";

/** The backend commands update recovery uses: the wrappers of the same names in `api.ts`. */
export type UpdateRecoveryApi = Pick<
  typeof backend,
  | "loadUpdateRecovery"
  | "saveUpdateRecovery"
  | "clearUpdateRecovery"
  | "completeUpdatePreparation"
  | "getLastTranscription"
  | "getUpdateResultPending"
  | "acknowledgeUpdateResult"
>;

/** The dictation editor of the Dictate tab, as the controller reads and writes it. */
export interface DictationEditorPort {
  text: string;
  /** The text is a result the user has not copied or saved yet. */
  recoveryPending: boolean;
  /** The user typed in the editor after its result was shown. */
  edited: boolean;
  /** A dictation is being transcribed, or its model is loading. */
  readonly transcribing: boolean;
}

/** The file queue of the Transcribe tab. Restoring adds completed jobs to it; discarding removes them. */
export interface FileQueuePort {
  jobs: FileJob[];
  /** The jobs that open as a saved meeting review. */
  savedReviewIds: string[];
  selectedId: string | null;
}

/** A result panel with work of its own to finish before its recovery entry is final. */
export interface UpdateRecoveryParticipant {
  prepareUpdateRecovery(): Promise<void>;
}

export interface UpdateRecoveryOptions {
  api: UpdateRecoveryApi;
  dictation: DictationEditorPort;
  files: FileQueuePort;
  /**
   * Resolves once the view has rendered the state changed so far. Svelte's
   * `tick` unless a test replaces it. See `setResultPanel` for what the
   * snapshot needs it for.
   */
  settle?: () => Promise<void>;
}

/** What a Copy or Save of the dictation editor delivered, captured before the delivery starts. */
export interface DictationDelivery {
  text: string;
  lineage: number;
  wasEdited: boolean;
  nativeSourceText: string | null;
  recoveredText: string | null;
  persistedText: string | null;
  persistedRevision: number | null;
}

type RecoveryCleanup = {
  fileJobIds?: readonly string[];
  meetingJobIds?: readonly string[];
  dictationTexts?: readonly string[];
  dictationLineage?: number;
  dictationText?: string;
  dictationPersistenceRevision?: number;
};

export class UpdateRecoveryController {
  /** The updater asked for a snapshot and was neither answered with a failure nor aborted since. */
  preparing = $state(false);
  /** The recovery entry of each file result, as its panel last reported it. */
  fileEntries = $state<UpdateRecoveryFile[]>([]);
  /** The recovery entry of each meeting review, as its panel last reported it. */
  meetingEntries = $state<UpdateRecoveryMeeting[]>([]);
  /** Drafts restored after an update are still open. */
  draftsNotice = $state(false);
  /** Why the stored drafts could not be read, or could not be cleaned up after a delivery. */
  readError = $state("");
  /** Why "Discard recovered drafts" could not delete the stored drafts; they stay open. The next attempt replaces it. */
  discardError = $state("");

  readonly #api: UpdateRecoveryApi;
  readonly #dictation: DictationEditorPort;
  readonly #files: FileQueuePort;
  readonly #settle: () => Promise<void>;

  #activeNonce: string | null = null;
  #liveDictationRevision = 0;
  #observedNativeDictation: string | null = null;
  #nativeEditorOrigin: { text: string; lineage: number } | null = null;
  readonly #resultPanels: Record<string, UpdateRecoveryParticipant | undefined> = {};

  #recoveredFileIds: string[] = [];
  #recoveredMeetingIds: string[] = [];
  #recoveredDictationText: string | null = null;
  #recoveredDictationActive = false;
  #writeQueue: Promise<void> = Promise.resolve();
  #restored: Promise<void> = Promise.resolve();
  #cleanupWarning = false;
  #editorLineage = 0;
  #persistenceRevision = 0;
  #lastPersistedDictation: { text: string; lineage: number; revision: number } | null = null;

  constructor(options: UpdateRecoveryOptions) {
    this.#api = options.api;
    this.#dictation = options.dictation;
    this.#files = options.files;
    this.#settle = options.settle ?? tick;
  }

  /** The startup restore: resolves when the stored drafts are open, rejects when they could not be read. */
  get restored(): Promise<void> {
    return this.#restored;
  }

  /** Read the stored drafts and reopen them. Call once, when the window mounts. */
  restore(): void {
    this.#restored = this.#restore();
    void this.#restored.catch(() => undefined);
  }

  /** The window is closing: a preparation still running stores and acknowledges nothing more. */
  dispose(): void {
    this.#activeNonce = null;
  }

  /**
   * The panel of a file job, for the snapshot to ask it to finish its work
   * (`prepareUpdateRecovery`). This registry replaces the record the window
   * used to fill through `bind:this` and read directly.
   *
   * What remains of that dependency: a panel can only register once the view
   * has mounted it, which is after the job was added to the queue. The
   * snapshot therefore awaits `settle` before it reads the registry, and
   * refuses the update ("still opening") for a job whose panel is not there
   * by then. Settings feeds the registry from `bind:this` on each
   * FileTranscription, so with Svelte's `tick` as `settle` the order is the
   * same as before.
   */
  resultPanel(jobId: string): UpdateRecoveryParticipant | undefined {
    return this.#resultPanels[jobId];
  }

  setResultPanel(jobId: string, panel: UpdateRecoveryParticipant | null | undefined): void {
    this.#resultPanels[jobId] = panel ?? undefined;
  }

  /** `update-preparing`: snapshot what is open for the update `nonce` names, then acknowledge it. */
  beginPreparation(nonce: string): void {
    this.#activeNonce = nonce;
    this.preparing = true;
    void this.#prepare(nonce);
  }

  /** `update-aborted`: the update is off, and so is its snapshot. */
  abortPreparation(): void {
    this.#activeNonce = null;
    this.preparing = false;
  }

  /** A dictation started recording: it outdates a restore still reading the stored drafts. */
  dictationStarted(): void {
    this.#liveDictationRevision++;
  }

  /** `transcription-result`: a new native dictation replaces whatever the editor showed. */
  showNativeResult(text: string): void {
    this.#liveDictationRevision++;
    this.#editorLineage++;
    this.#dictation.recoveryPending = true;
    this.#dictation.edited = false;
    this.#recoveredDictationActive = false;
    this.#observedNativeDictation = text;
    this.#nativeEditorOrigin = { text, lineage: this.#editorLineage };
    this.#dictation.text = text;
  }

  /**
   * The last native dictation, read once the window listens for results:
   * native delivery has settled, even when a different recovered draft is
   * already in the editor. Otherwise the snapshot waits forever for an event
   * that preceded the listener. An empty editor shows the result.
   */
  nativeResultSettled(text: string | null): void {
    this.#observedNativeDictation = text;
    if (text && !this.#dictation.text.trim()) {
      this.#liveDictationRevision++;
      this.#editorLineage++;
      this.#nativeEditorOrigin = { text, lineage: this.#editorLineage };
      this.#dictation.recoveryPending = true;
      this.#recoveredDictationActive = false;
      this.#dictation.text = text;
    }
  }

  /** The test recording of the Dictate tab was transcribed: its text joins what the editor holds. */
  appendTestRecording(text: string): void {
    this.#observedNativeDictation = text;
    this.#nativeEditorOrigin = { text, lineage: this.#editorLineage };
    this.#dictation.recoveryPending = true;
    this.#dictation.text = this.#dictation.text ? this.#dictation.text + " " + text : text;
  }

  /** A file result panel reported its recovery entry, or (`null`) that its result was delivered. */
  onFileRecoveryChange(entry: UpdateRecoveryFile | null, jobId?: string): void {
    if (entry === null) {
      if (jobId) this.#clearFileRecovery(jobId);
      return;
    }
    this.#updateFileRecovery(entry);
  }

  /** A meeting review panel reported its recovery entry, or (`null`) that its review was delivered. */
  onMeetingRecoveryChange(entry: UpdateRecoveryMeeting | null, jobId?: string): void {
    if (entry === null) {
      if (jobId) this.#clearMeetingRecovery(jobId);
      return;
    }
    this.#updateMeetingRecovery(entry);
  }

  /** Delete the stored drafts and close the ones restored from them. */
  async discardDrafts(): Promise<void> {
    this.discardError = "";
    try {
      await this.#enqueueMutation(async () => {
        await this.#api.clearUpdateRecovery();
      });
      this.#cleanupWarning = false;
      this.readError = "";
    } catch (error) {
      this.discardError = `Could not discard recovered drafts: ${toErrorMessage(error)}`;
      return;
    }

    const recoveredFileIds = this.#recoveredFileIds;
    const recoveredMeetingIds = this.#recoveredMeetingIds;
    this.#files.jobs = this.#files.jobs.filter((job) => !recoveredFileIds.includes(job.id) && !recoveredMeetingIds.includes(job.id));
    this.#files.savedReviewIds = this.#files.savedReviewIds.filter((id) => !recoveredMeetingIds.includes(id));
    this.fileEntries = this.fileEntries.filter((entry) => !recoveredFileIds.includes(entry.job_id));
    this.meetingEntries = this.meetingEntries.filter((entry) => !recoveredMeetingIds.includes(entry.job_id));
    if (this.#recoveredDictationActive && this.#dictation.text === this.#recoveredDictationText) {
      this.#dictation.text = "";
      this.#dictation.recoveryPending = false;
      this.#dictation.edited = false;
    }
    this.#recoveredFileIds = [];
    this.#recoveredMeetingIds = [];
    this.#recoveredDictationText = null;
    this.#recoveredDictationActive = false;
    this.#lastPersistedDictation = null;
    this.draftsNotice = false;
    this.#files.selectedId = this.#files.jobs[0]?.id ?? null;
  }

  /** What Copy or Save is about to deliver from the editor. Capture it before the delivery's first await. */
  captureDelivery(text: string): DictationDelivery {
    const lineage = this.#editorLineage;
    return {
      text,
      lineage,
      wasEdited: this.#dictation.edited,
      nativeSourceText: this.#nativeEditorOrigin?.lineage === lineage ? this.#nativeEditorOrigin.text : null,
      recoveredText: this.#recoveredDictationText,
      persistedText: this.#lastPersistedDictation?.lineage === lineage ? this.#lastPersistedDictation.text : null,
      persistedRevision: this.#lastPersistedDictation?.lineage === lineage ? this.#lastPersistedDictation.revision : null,
    };
  }

  /**
   * The editor's text was copied or saved: acknowledge the native result it
   * came from and remove its stored draft. Resolves true when another unsaved
   * native dictation was waiting and is now shown instead.
   */
  async markDelivered(delivery: DictationDelivery): Promise<boolean> {
    const { text, lineage, wasEdited, nativeSourceText, recoveredText, persistedText, persistedRevision } = delivery;
    const nativePending = await this.#api.getUpdateResultPending("live-dictation");
    const nativeText = nativePending ? await this.#api.getLastTranscription() : null;
    const matchesNative =
      lineage === this.#editorLineage &&
      nativePending &&
      (nativeText === text || (wasEdited && nativeSourceText !== null && nativeSourceText === nativeText));
    const deliveredNative = matchesNative && nativeText !== null ? await this.#api.acknowledgeUpdateResult(nativeText) : false;
    const cleanupTexts = [persistedText, recoveredText, text].filter((value): value is string => value !== null);
    await this.#clearDeliveredDraft(cleanupTexts, lineage, text, persistedRevision);
    if (this.#dictation.text !== text || this.#editorLineage !== lineage) return false; // a newer result arrived during Copy/Save
    this.#dictation.recoveryPending = false;
    this.#dictation.edited = false;
    this.#recoveredDictationActive = false;
    this.#refreshDraftsNotice();
    const currentNative =
      nativePending && !deliveredNative && (await this.#api.getUpdateResultPending("live-dictation"))
        ? await this.#api.getLastTranscription()
        : null;
    if (currentNative?.trim()) {
      // The copied recovered draft was not the latest native dictation. Show
      // that still-unsaved result next, keeping its update guard intact.
      this.#editorLineage++;
      this.#dictation.text = currentNative;
      this.#dictation.recoveryPending = true;
      this.#nativeEditorOrigin = { text: currentNative, lineage: this.#editorLineage };
      this.#observedNativeDictation = currentNative;
      return true;
    }
    return false;
  }

  #clearCleanupWarning(): void {
    if (!this.#cleanupWarning) return;
    this.#cleanupWarning = false;
    this.readError = "";
  }

  #cleanupError(error: unknown): string {
    const detail = toErrorMessage(error).replace(/[.]$/, "");
    return `Update recovery draft was retained after delivery. ${detail}. Back up or repair update-recovery.json in Sagascript's app data folder, then restart Sagascript before retrying the update.`;
  }

  /** Run `mutation` after every earlier one has settled, whether it succeeded or failed. */
  #enqueueMutation<T>(mutation: () => Promise<T>): Promise<T> {
    const operation = this.#writeQueue.catch(() => undefined).then(mutation);
    this.#writeQueue = operation.then(
      () => undefined,
      () => undefined,
    );
    return operation;
  }

  #enqueueCleanup(removals: RecoveryCleanup): Promise<void> {
    const fileJobIds = removals.fileJobIds ?? [];
    const meetingJobIds = removals.meetingJobIds ?? [];
    const dictationTexts = removals.dictationTexts ?? [];
    return this.#enqueueMutation(async () => {
      const previous = readPersistedUpdateRecoveryPayload(await this.#api.loadUpdateRecovery());
      if (!previous) {
        this.#clearCleanupWarning();
        return;
      }
      const dictationMatches = Boolean(
        previous.dictation &&
        dictationTexts.includes(previous.dictation.text) &&
        (removals.dictationLineage === undefined ||
          (this.#lastPersistedDictation?.lineage === removals.dictationLineage &&
            this.#lastPersistedDictation.text === previous.dictation.text &&
            (previous.dictation.text === removals.dictationText ||
              this.#lastPersistedDictation.revision === removals.dictationPersistenceRevision))),
      );
      const remaining = {
        ...previous,
        dictation: dictationMatches ? null : previous.dictation,
        files: previous.files.filter((entry) => !fileJobIds.includes(entry.job_id)),
        meetings: previous.meetings.filter((entry) => !meetingJobIds.includes(entry.job_id)),
      };
      const changed =
        dictationMatches || remaining.files.length !== previous.files.length || remaining.meetings.length !== previous.meetings.length;
      if (!changed) {
        this.#clearCleanupWarning();
        return;
      }
      if (!remaining.dictation && remaining.files.length === 0 && remaining.meetings.length === 0) {
        await this.#api.clearUpdateRecovery();
      } else {
        serializeUpdateRecoveryPayload(remaining);
        await this.#api.saveUpdateRecovery(remaining);
      }
      if (dictationMatches) this.#lastPersistedDictation = null;
      this.#clearCleanupWarning();
    });
  }

  #persistRemainingDrafts(removals: RecoveryCleanup): void {
    void this.#enqueueCleanup(removals).catch((error) => {
      this.#cleanupWarning = true;
      this.readError = this.#cleanupError(error);
    });
  }

  #refreshDraftsNotice(): void {
    if (!this.#recoveredDictationActive && this.#recoveredFileIds.length === 0 && this.#recoveredMeetingIds.length === 0) {
      this.draftsNotice = false;
      // The button that failed left with the notice: every recovered draft was delivered or closed.
      this.discardError = "";
    }
  }

  #updateFileRecovery(entry: UpdateRecoveryFile): void {
    const existing = this.fileEntries.some((current) => current.job_id === entry.job_id);
    this.fileEntries = existing
      ? this.fileEntries.map((current) => (current.job_id === entry.job_id ? entry : current))
      : [...this.fileEntries, entry];
  }

  #clearFileRecovery(jobId: string): void {
    this.fileEntries = this.fileEntries.filter((entry) => entry.job_id !== jobId);
    if (this.#recoveredFileIds.includes(jobId)) {
      this.#recoveredFileIds = this.#recoveredFileIds.filter((id) => id !== jobId);
      this.#refreshDraftsNotice();
      this.#persistRemainingDrafts({ fileJobIds: [jobId] });
    }
  }

  #updateMeetingRecovery(entry: UpdateRecoveryMeeting): void {
    const existing = this.meetingEntries.some((current) => current.job_id === entry.job_id);
    this.meetingEntries = existing
      ? this.meetingEntries.map((current) => (current.job_id === entry.job_id ? entry : current))
      : [...this.meetingEntries, entry];
  }

  #clearMeetingRecovery(jobId: string): void {
    this.meetingEntries = this.meetingEntries.filter((entry) => entry.job_id !== jobId);
    if (this.#recoveredMeetingIds.includes(jobId)) {
      this.#recoveredMeetingIds = this.#recoveredMeetingIds.filter((id) => id !== jobId);
      this.#refreshDraftsNotice();
      this.#persistRemainingDrafts({ meetingJobIds: [jobId] });
    }
  }

  async #restore(): Promise<void> {
    const dictationRevision = this.#liveDictationRevision;
    const existingFileIds = this.#files.jobs.map((job) => job.id);
    const existingMeetingIds = [...this.#files.savedReviewIds];
    try {
      const persisted = readPersistedUpdateRecoveryPayload(await this.#api.loadUpdateRecovery());
      this.readError = "";
      if (!persisted) return;

      const recoveredDictation = persisted.dictation?.text.trim() ? persisted.dictation : null;
      if (recoveredDictation && !this.#dictation.text.trim() && this.#liveDictationRevision === dictationRevision) {
        this.#editorLineage++;
        this.#dictation.text = recoveredDictation.text;
        this.#dictation.recoveryPending = true;
        this.#dictation.edited = false;
        this.#recoveredDictationText = recoveredDictation.text;
        this.#recoveredDictationActive = true;
        this.#nativeEditorOrigin = null;
        this.#persistenceRevision++;
        this.#lastPersistedDictation = {
          text: recoveredDictation.text,
          lineage: this.#editorLineage,
          revision: this.#persistenceRevision,
        };
      }

      const filesToRestore = persisted.files.filter((entry) => !existingFileIds.includes(entry.job_id));
      if (filesToRestore.length) {
        this.fileEntries = [...this.fileEntries, ...filesToRestore];
        this.#recoveredFileIds = [...this.#recoveredFileIds, ...filesToRestore.map((entry) => entry.job_id)];
        this.#files.jobs = [
          ...this.#files.jobs,
          ...filesToRestore.map((entry) => ({
            id: entry.job_id,
            path: entry.path,
            diarize: false,
            prompt: null,
            profileId: null,
            status: "completed" as const,
          })),
        ];
      }

      const meetingsToRestore = persisted.meetings.filter((entry) => !existingMeetingIds.includes(entry.job_id));
      if (meetingsToRestore.length) {
        this.meetingEntries = [...this.meetingEntries, ...meetingsToRestore];
        this.#recoveredMeetingIds = [...this.#recoveredMeetingIds, ...meetingsToRestore.map((entry) => entry.job_id)];
        this.#files.jobs = [
          ...this.#files.jobs,
          ...meetingsToRestore.map((entry) => ({
            id: entry.job_id,
            path: entry.path,
            diarize: false,
            prompt: null,
            profileId: null,
            status: "completed" as const,
          })),
        ];
        this.#files.savedReviewIds = [...this.#files.savedReviewIds, ...meetingsToRestore.map((entry) => entry.job_id)];
      }

      if (this.#files.selectedId === null) {
        this.#files.selectedId = filesToRestore[0]?.job_id ?? meetingsToRestore[0]?.job_id ?? null;
      }

      if (recoveredDictation || filesToRestore.length || meetingsToRestore.length) {
        this.draftsNotice = true;
      }
    } catch (error) {
      console.warn("Could not restore update recovery drafts", error);
      this.readError = `Update blocked: unreadable recovery drafts were retained. ${toErrorMessage(error).replace(/[.]$/, "")}. Back up or repair update-recovery.json in Sagascript's app data folder, then restart Sagascript before retrying.`;
      throw error;
    }
  }

  #isActive(nonce: string): boolean {
    return this.#activeNonce === nonce;
  }

  async #prepare(nonce: string): Promise<void> {
    try {
      await this.#restored;
      if (!this.#isActive(nonce)) return;
      // Every job's panel has to be registered before it is asked: see `setResultPanel`.
      await this.#settle();
      if (!this.#isActive(nonce)) return;
      await Promise.all(
        this.#files.jobs.map(async (job) => {
          const panel = this.#resultPanels[job.id];
          if (!panel) throw new Error("A transcription result is still opening. Retry the update.");
          await panel.prepareUpdateRecovery();
        }),
      );
      if (!this.#isActive(nonce)) return;
      // The updater holds the native exclusive lease here. Capture the stable
      // native snapshot before draining any result event that is still in
      // transit, but defer the conflict decision until after the drain.
      const stableNativeDictation = await this.#api.getLastTranscription();
      if (!this.#isActive(nonce)) return;
      const initialNativeResultPending = await this.#api.getUpdateResultPending("live-dictation");
      if (!this.#isActive(nonce)) return;
      try {
        await drainUpdateWork({
          busy: () =>
            this.#dictation.transcribing ||
            Boolean(initialNativeResultPending && stableNativeDictation?.trim() && this.#observedNativeDictation !== stableNativeDictation),
          failure: () => (this.#isActive(nonce) ? null : "Update preparation expired."),
          settle: this.#settle,
        });
      } catch (error) {
        // A completed native result whose event was missed needs the actual
        // conflict decision below, not advice to wait for already-finished work.
        // Rendering failures, cancellation and ongoing transcription still fail.
        const missedNativeResult =
          error instanceof UpdatePreparationTimeoutError &&
          !this.#dictation.transcribing &&
          initialNativeResultPending &&
          stableNativeDictation?.trim() &&
          this.#observedNativeDictation !== stableNativeDictation;
        if (!missedNativeResult) throw error;
      }
      if (!this.#isActive(nonce)) return;
      await this.#settle();
      if (!this.#isActive(nonce)) return;
      await this.#enqueueMutation(async () => {
        if (!this.#isActive(nonce)) return;
        // Refresh the pending flag and capture the editor only after earlier
        // recovery mutations finish. The exclusive lease keeps native text stable.
        const nativeResultPending = await this.#api.getUpdateResultPending("live-dictation");
        if (!this.#isActive(nonce)) return;
        const payloadLineage = this.#editorLineage;
        const nativeEditorOwnsResult =
          this.#nativeEditorOrigin?.lineage === payloadLineage && this.#nativeEditorOrigin.text === stableNativeDictation;
        const editedNativeResult = this.#dictation.edited && !this.#recoveredDictationActive && nativeEditorOwnsResult;
        if (
          nativeResultPending &&
          stableNativeDictation?.trim() &&
          ((!this.#dictation.text.trim() && !nativeEditorOwnsResult) ||
            (this.#dictation.text.trim() && this.#dictation.text !== stableNativeDictation && !editedNativeResult))
        ) {
          throw new Error("A different unsaved dictation is already open. Copy or save both results before retrying the update.");
        }
        const payload = createUpdateRecoveryPayload({
          dictation:
            this.#dictation.recoveryPending &&
            (nativeResultPending || this.#recoveredDictationActive || this.#dictation.edited) &&
            this.#dictation.text.trim()
              ? { text: this.#dictation.text }
              : null,
          files: this.fileEntries,
          meetings: this.meetingEntries,
        });
        // A result event can arrive before recovery is hydrated (or replace an
        // older recovered draft). Replace persisted bytes only when they are
        // the session's last successful save in this same editor lineage.
        const previous = readPersistedUpdateRecoveryPayload(await this.#api.loadUpdateRecovery());
        if (!this.#isActive(nonce)) return;
        const ownsPersistedDictation = Boolean(
          previous?.dictation?.text.trim() &&
          this.#lastPersistedDictation?.text === previous.dictation.text &&
          this.#lastPersistedDictation.lineage === payloadLineage,
        );
        if (previous?.dictation?.text.trim() && previous.dictation.text !== payload.dictation?.text && !ownsPersistedDictation) {
          throw new Error(
            "A different unsaved dictation is already in update recovery. Copy or save both results before retrying the update.",
          );
        }
        // Validate the exact serialized size before handing the payload to Rust.
        serializeUpdateRecoveryPayload(payload);
        await this.#api.saveUpdateRecovery(payload);
        if (payload.dictation) {
          this.#persistenceRevision++;
          this.#lastPersistedDictation = {
            text: payload.dictation.text,
            lineage: payloadLineage,
            revision: this.#persistenceRevision,
          };
        } else {
          this.#lastPersistedDictation = null;
        }
        if (!this.#isActive(nonce)) return;
        await this.#api.completeUpdatePreparation(nonce, null);
      });
    } catch (error) {
      if (!this.#isActive(nonce)) return;
      const message = toErrorMessage(error);
      this.preparing = false;
      try {
        await this.#enqueueMutation(async () => {
          if (!this.#isActive(nonce)) return;
          await this.#api.completeUpdatePreparation(nonce, message);
        });
      } catch (ackError) {
        console.warn("Could not acknowledge failed update preparation", ackError);
      }
    }
  }

  async #clearDeliveredDraft(
    texts: readonly string[],
    lineage: number,
    deliveredText: string,
    persistedRevision: number | null,
  ): Promise<void> {
    try {
      await this.#enqueueCleanup({
        dictationTexts: texts,
        dictationLineage: lineage,
        dictationText: deliveredText,
        dictationPersistenceRevision: persistedRevision ?? undefined,
      });
    } catch (error) {
      // Delivery already succeeded. Preserve the unreadable snapshot and
      // surface a concrete warning without turning Copy/Save into a failure.
      console.warn("Could not clear delivered update recovery draft", error);
      this.#cleanupWarning = true;
      this.readError = this.#cleanupError(error);
    }
  }
}
