<script lang="ts">
  import { drainUpdateWork } from "./update-preparation";
  import FileTranscription from "./FileTranscription.svelte";
  import {
    createFileJobs, nextQueuedFile, updateFileJob, fileJobName,
    type FileJob, type FileJobStatus,
  } from "./transcription-queue";
  import { onMount, tick } from "svelte";
  import { rememberTranscription, type RecentTranscription } from "./recent-transcriptions";
  import {
    createUpdateRecoveryPayload,
    parseUpdateRecoveryPayload,
    serializeUpdateRecoveryPayload,
    type UpdateRecoveryFile,
    type UpdateRecoveryMeeting,
  } from "./update-recovery";
  import {
    getSettings,
    getLastError,
    getLastTranscription,
    setLanguage,
    setHotkeyProfiles,
    setAutoPaste,
    setInitialPrompt,
    setProfileGlossary,
    setShowOverlay,
    setWhisperModel,
    setFileTranscriptionModel,
    setBeamSize,
    setTemperatureFallback,
    setVadEnabled,
    getBuildInfo,
    getModelInfo,
    getFileModelOptions,
    getEffectiveModelInfo,
    downloadModel,
    downloadPianissimoModel,
    getSupportedFormats,
    getPlatform,
    checkAccessibilityPermission,
    requestAccessibilityPermission,
    retryHotkeyRegistration,
    startRecording,
    stopAndTranscribe,
    copyTranscriptionText,
    saveTranscriptionText,
    setUpdateResultPending,
    saveUpdateRecovery,
    loadUpdateRecovery,
    clearUpdateRecovery,
    completeUpdatePreparation,
    hotkeyStatus,
    type Settings,
    type BuildInfo,
    type Language,
    type WhisperModel,
    type HotkeyStatus,
    type HotkeyProfile,
  } from "./api";
  import { listen } from "@tauri-apps/api/event";
  import { open } from "@tauri-apps/plugin-dialog";
  import { getCurrentWebview } from "@tauri-apps/api/webview";
  import {
    dictateButtonAction,
    retainTestRecordingOwnership,
    type BackendDictationState,
  } from "./dictation-ui-state";
  import {
    canUseBareHotkey,
    formatHotkeyDisplay as formatShortcutDisplay,
    supportedBareFunctionKeyRange,
    tauriKeyName,
  } from "./hotkey.js";
  import {
    allProfileShortcutValues,
    displayProfileShortcut,
    profileWithShortcut,
    suggestedProfileShortcuts,
  } from "./profile-shortcuts.js";

  let settings: Settings | null = $state(null);
  let buildInfo: BuildInfo | null = $state(null);
  let models: WhisperModel[] = $state([]);
  let fileModelOptions: WhisperModel[] = $state([]);
  let fileAutoModel: WhisperModel | null = $state(null);
  let fileModelError = $state("");
  let fileModelSaving = $state(false);
  type SettingsTab = "dictate" | "transcribe" | "settings";
  let activeTab: SettingsTab = $state("dictate");
  let downloading: string | null = $state(null);
  let downloadingName: string = $state("");
  let downloadProgress: number = $state(0);
  let profileModels: Record<string, WhisperModel> = $state({});
  let profileModelErrors: Record<string, string> = $state({});
  let profileModelRefresh = 0;

  let platform: string = $state("unknown");

  // Initial data-fetch + settings-mutation error states
  let initError: string = $state("");
  let settingsError: string = $state("");
  let blockedLanguageChange: { language: Language; source: string } | null = $state(null);
  let languageSaving = $state(false);
  let languageSelectEl: HTMLSelectElement | undefined = $state();
  let languageRecoveryTitle: HTMLHeadingElement | undefined = $state();
  // Contract-pinned against the Rust guard by test-language-switch-recovery.
  const defaultDictionaryLanguageBlock = "Profile 'default' has a personal dictionary";

  $effect(() => {
    if (blockedLanguageChange && languageRecoveryTitle) languageRecoveryTitle.focus();
  });

  async function restoreLanguageFocus(): Promise<void> {
    await tick();
    languageSelectEl?.focus();
  }

  function cancelLanguageChange(): void {
    blockedLanguageChange = null;
    void restoreLanguageFocus();
  }

  // Model selection state
  let selecting: boolean = $state(false);
  let modelError: string = $state("");

  let accessibilityGranted: boolean = $state(true); // assume true; checked on mount for macOS
  let accessibilityChecking: boolean = $state(false);
  let accessibilityRequested: boolean = $state(false);

  // Hotkey recorder state
  let recordingProfileId: string | null = $state(null);
  type ExplicitShortcutSlot = "push_to_talk_shortcut" | "toggle_shortcut";
  const shortcutControls: { slot: ExplicitShortcutSlot; label: string; helper: string }[] = [
    {
      slot: "push_to_talk_shortcut",
      label: "Hold to record",
      helper: "Hold the shortcut while speaking. Release to stop.",
    },
    {
      slot: "toggle_shortcut",
      label: "Press to start/stop",
      helper: "Press the shortcut to start recording. Press it again to stop.",
    },
  ];
  let recordingShortcutSlot: ExplicitShortcutSlot | null = $state(null);
  let hotkeyCaptureGeneration = 0;
  let draftProfileId: string | null = $state(null);
  let hotkeyError: string = $state("");
  let hotkeyRecorderEl: HTMLButtonElement | undefined = $state();

  // Hotkey registration health — is the saved hotkey actually bound right
  // now? (distinct from hotkeyError above, which is about validating a
  // shortcut the user is in the middle of entering.) Assume healthy until
  // proven otherwise so there's no flash of a warning before the initial
  // fetch resolves.
  let hotkeyStatusOk: boolean = $state(true);
  let hotkeyStatusError: string = $state("");

  $effect(() => {
    if (recordingProfileId && recordingShortcutSlot && hotkeyRecorderEl) hotkeyRecorderEl.focus();
  });

  // Dictate test state
  let testRecording: boolean = $state(false);
  let testTranscribing: boolean = $state(false);
  let backendDictationState: BackendDictationState = $state("idle");
  let testOwnsRecording: boolean = $state(false);
  let testResult: string = $state("");
  let testError: string = $state("");
  let testResultActionMessage: string = $state("");
  let testResultRecoveryPending: boolean = $state(false);
  let updatePreparing: boolean = $state(false);
  let liveDictationRevision = 0;
  let observedNativeDictation: string | null = null;
  let fileComponents: Record<string, { prepareUpdateRecovery(): Promise<void> } | undefined> = $state({});

  let fileRecoveryEntries = $state<UpdateRecoveryFile[]>([]);
  let meetingRecoveryEntries = $state<UpdateRecoveryMeeting[]>([]);
  let recoveredFileIds = $state<string[]>([]);
  let recoveredMeetingIds = $state<string[]>([]);
  let recoveredDictationText: string | null = $state(null);
  let recoveredDictationActive = $state(false);
  let recoveredDraftsNotice = $state(false);
  let recoveryWriteQueue: Promise<void> = Promise.resolve();
  let recoveryRestore: Promise<void> = Promise.resolve();

  function persistRemainingRecoveredDrafts(): void {
    const fileIds = new Set(recoveredFileIds);
    const meetingIds = new Set(recoveredMeetingIds);
    const remaining = createUpdateRecoveryPayload({
      dictation: recoveredDictationActive && testResult.trim() ? { text: testResult } : null,
      files: fileRecoveryEntries.filter((entry) => fileIds.has(entry.job_id)),
      meetings: meetingRecoveryEntries.filter((entry) => meetingIds.has(entry.job_id)),
    });
    recoveryWriteQueue = recoveryWriteQueue.catch(() => undefined).then(async () => {
      if (!remaining.dictation && remaining.files.length === 0 && remaining.meetings.length === 0) {
        await clearUpdateRecovery();
      } else {
        serializeUpdateRecoveryPayload(remaining);
        await saveUpdateRecovery(remaining);
      }
    });
    void recoveryWriteQueue.catch((error) => {
      settingsError = `Could not update recovered drafts: ${recoveryErrorText(error)}`;
    });
  }

  function refreshRecoveredDraftsNotice(): void {
    if (!recoveredDictationActive && recoveredFileIds.length === 0 && recoveredMeetingIds.length === 0) {
      recoveredDraftsNotice = false;
    }
  }

  function updateFileRecovery(entry: UpdateRecoveryFile): void {
    const existing = fileRecoveryEntries.some((current) => current.job_id === entry.job_id);
    fileRecoveryEntries = existing
      ? fileRecoveryEntries.map((current) => current.job_id === entry.job_id ? entry : current)
      : [...fileRecoveryEntries, entry];
  }

  function clearFileRecovery(jobId: string): void {
    fileRecoveryEntries = fileRecoveryEntries.filter((entry) => entry.job_id !== jobId);
    if (recoveredFileIds.includes(jobId)) {
      recoveredFileIds = recoveredFileIds.filter((id) => id !== jobId);
      refreshRecoveredDraftsNotice();
      persistRemainingRecoveredDrafts();
    }
  }

  function onFileRecoveryChange(entry: UpdateRecoveryFile | null, jobId?: string): void {
    if (entry === null) {
      if (jobId) clearFileRecovery(jobId);
      return;
    }
    updateFileRecovery(entry);
  }

  function updateMeetingRecovery(entry: UpdateRecoveryMeeting | null): void {
    if (entry === null) return;
    const existing = meetingRecoveryEntries.some((current) => current.job_id === entry.job_id);
    meetingRecoveryEntries = existing
      ? meetingRecoveryEntries.map((current) => current.job_id === entry.job_id ? entry : current)
      : [...meetingRecoveryEntries, entry];
  }

  function clearMeetingRecovery(jobId: string): void {
    meetingRecoveryEntries = meetingRecoveryEntries.filter((entry) => entry.job_id !== jobId);
    if (recoveredMeetingIds.includes(jobId)) {
      recoveredMeetingIds = recoveredMeetingIds.filter((id) => id !== jobId);
      refreshRecoveredDraftsNotice();
      persistRemainingRecoveredDrafts();
    }
  }

  function onMeetingRecoveryChange(entry: UpdateRecoveryMeeting | null, jobId?: string): void {
    if (entry === null) {
      if (jobId) clearMeetingRecovery(jobId);
      return;
    }
    updateMeetingRecovery(entry);
  }

  function recoveryErrorText(error: unknown): string {
    return typeof error === "string" ? error : error instanceof Error ? error.message : String(error);
  }

  async function restoreUpdateRecovery(): Promise<void> {
    const dictationRevision = liveDictationRevision;
    const existingFileIds = new Set(fileJobs.map((job) => job.id));
    const existingMeetingIds = new Set(savedReviewIds);
    try {
      const persisted = parseUpdateRecoveryPayload(await loadUpdateRecovery());
      if (!persisted) return;

      const recoveredDictation = persisted.dictation?.text.trim() ? persisted.dictation : null;
      if (recoveredDictation && !testResult.trim() && liveDictationRevision === dictationRevision) {
        testResult = recoveredDictation.text;
        testResultRecoveryPending = true;
        recoveredDictationText = recoveredDictation.text;
        recoveredDictationActive = true;
      }

      const filesToRestore = persisted.files.filter((entry) => !existingFileIds.has(entry.job_id));
      if (filesToRestore.length) {
        fileRecoveryEntries = [...fileRecoveryEntries, ...filesToRestore];
        recoveredFileIds = [...recoveredFileIds, ...filesToRestore.map((entry) => entry.job_id)];
        fileJobs = [
          ...fileJobs,
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

      const meetingsToRestore = persisted.meetings.filter((entry) => !existingMeetingIds.has(entry.job_id));
      if (meetingsToRestore.length) {
        meetingRecoveryEntries = [...meetingRecoveryEntries, ...meetingsToRestore];
        recoveredMeetingIds = [...recoveredMeetingIds, ...meetingsToRestore.map((entry) => entry.job_id)];
        fileJobs = [
          ...fileJobs,
          ...meetingsToRestore.map((entry) => ({
            id: entry.job_id,
            path: entry.path,
            diarize: false,
            prompt: null,
            profileId: null,
            status: "completed" as const,
          })),
        ];
        savedReviewIds = [...savedReviewIds, ...meetingsToRestore.map((entry) => entry.job_id)];
      }

      if (selectedFileId === null) {
        selectedFileId = filesToRestore[0]?.job_id ?? meetingsToRestore[0]?.job_id ?? null;
      }

      if (recoveredDictation || filesToRestore.length || meetingsToRestore.length) {
        recoveredDraftsNotice = true;
      }
    } catch (error) {
      console.warn("Could not restore update recovery drafts", error);
      throw error;
    }
  }

  async function discardRecoveredDrafts(): Promise<void> {
    try {
      await recoveryWriteQueue.catch(() => undefined);
      await clearUpdateRecovery();
    } catch (error) {
      settingsError = `Could not discard recovered drafts: ${recoveryErrorText(error)}`;
      return;
    }

    const recoveredFileIdSet = new Set(recoveredFileIds);
    const recoveredMeetingIdSet = new Set(recoveredMeetingIds);
    fileJobs = fileJobs.filter((job) => !recoveredFileIdSet.has(job.id) && !recoveredMeetingIdSet.has(job.id));
    savedReviewIds = savedReviewIds.filter((id) => !recoveredMeetingIdSet.has(id));
    fileRecoveryEntries = fileRecoveryEntries.filter((entry) => !recoveredFileIdSet.has(entry.job_id));
    meetingRecoveryEntries = meetingRecoveryEntries.filter((entry) => !recoveredMeetingIdSet.has(entry.job_id));
    if (recoveredDictationActive && testResult === recoveredDictationText) {
      testResult = "";
      testResultRecoveryPending = false;
    }
    recoveredFileIds = [];
    recoveredMeetingIds = [];
    recoveredDictationText = null;
    recoveredDictationActive = false;
    recoveredDraftsNotice = false;
    selectedFileId = fileJobs[0]?.id ?? null;
  }

  async function prepareForUpdate(nonce: string): Promise<void> {
    try {
      await recoveryRestore;
      await tick();
      await Promise.all(fileJobs.map(async (job) => {
        const component = fileComponents[job.id];
        if (!component) throw new Error("A transcription result is still opening. Retry the update.");
        await component.prepareUpdateRecovery();
      }));
      // The updater holds the native exclusive lease here. Its last result is
      // stable, but the result event/command response may still be in transit.
      const lastNativeDictation = await getLastTranscription();
      await drainUpdateWork({
        busy: () => testTranscribing || Boolean(lastNativeDictation?.trim()
          && observedNativeDictation !== lastNativeDictation),
        settle: tick,
      });
      await tick();
      await recoveryWriteQueue.catch(() => undefined);
      const payload = createUpdateRecoveryPayload({
        dictation: testResultRecoveryPending && testResult.trim() ? { text: testResult } : null,
        files: fileRecoveryEntries,
        meetings: meetingRecoveryEntries,
      });
      // Validate the exact serialized size before handing the payload to Rust.
      serializeUpdateRecoveryPayload(payload);
      await saveUpdateRecovery(payload);
      await completeUpdatePreparation(nonce, null);
    } catch (error) {
      const message = recoveryErrorText(error);
      updatePreparing = false;
      try {
        await completeUpdatePreparation(nonce, message);
      } catch (ackError) {
        console.warn("Could not acknowledge failed update preparation", ackError);
      }
    }
  }

  async function copyTestResult(): Promise<void> {
    if (!testResult.trim()) return;
    try {
      await copyTranscriptionText(testResult);
      await setUpdateResultPending("live-dictation", false);
      const wasRecovered = recoveredDictationActive;
      testResultRecoveryPending = false;
      recoveredDictationActive = false;
      refreshRecoveredDraftsNotice();
      if (wasRecovered) persistRemainingRecoveredDrafts();
      testResultActionMessage = "Copied to clipboard.";
    } catch (error) {
      testResultActionMessage = typeof error === "string" ? error : String(error);
    }
  }

  async function saveTestResult(): Promise<void> {
    if (!testResult.trim()) return;
    try {
      const saved = await saveTranscriptionText(testResult, "dictation.txt", null);
      if (saved) {
        await setUpdateResultPending("live-dictation", false);
        const wasRecovered = recoveredDictationActive;
        testResultRecoveryPending = false;
        recoveredDictationActive = false;
        refreshRecoveredDraftsNotice();
        if (wasRecovered) persistRemainingRecoveredDrafts();
      }
      testResultActionMessage = saved ? "Saved." : "Save cancelled — nothing was written.";
    } catch (error) {
      testResultActionMessage = typeof error === "string" ? error : String(error);
    }
  }

  onMount(() => {
    let disposed = false;
    let revision = 0;
    const stops: Array<() => void> = [];
    const remember = (stop: () => void) => disposed ? stop() : stops.push(stop);
    const errorListener = listen<string>("error", (event) => {
      revision++;
      testError = event.payload;
      requestTabChange("dictate");
    }).then(remember);
    const resultListener = listen<string>("transcription-result", (event) => {
      revision++;
      liveDictationRevision++;
      testResultRecoveryPending = true;
      recoveredDictationActive = false;
      observedNativeDictation = event.payload;
      testResult = event.payload;
      testError = "";
    }).then(remember);
    const stateListener = listen<string>("state-changed", (event) => {
      if (event.payload === "recording") {
        revision++;
        liveDictationRevision++;
        testError = "";
      }
    }).then(remember);
    // A failed background dictation may create this window after its event.
    // Recover the persisted-in-memory result without racing newer events.
    Promise.all([errorListener, resultListener, stateListener]).then(async () => {
      const initialRevision = revision;
      const [error, text] = await Promise.all([getLastError(), getLastTranscription()]);
      if (!disposed && revision === initialRevision) {
        if (!text?.trim() || text === testResult) observedNativeDictation = text;
        testError = error ?? "";
        if (text && !testResult.trim()) {
          observedNativeDictation = text;
          liveDictationRevision++;
          testResultRecoveryPending = true;
          recoveredDictationActive = false;
          testResult = text;
        }
      }
    }).catch((error) => {
      console.warn("Could not restore the last dictation result", error);
    });
    return () => { disposed = true; stops.forEach((stop) => stop()); };
  });

  // Transcribe tab state
  let supportedFormats: string[] = $state([]);
  let dragOver = $state(false);
  let transcribePrompt = $state("");
  let transcribeDiarize = $state(false);
  let transcribeProfileId: string | null = $state(null);
  let fileJobs = $state<FileJob[]>([]);
  let selectedFileId = $state<string | null>(null);
  let fileBusy = $state<Record<string, boolean>>({});
  let fileAttention = $state<Record<string, boolean>>({});
  let filesNeedingRetry = $derived(fileJobs.filter(job => fileAttention[job.id]));
  let savedReviewIds = $state<string[]>([]);
  let recentTranscriptions = $state<RecentTranscription[]>([]);
  let selectedRerunPath = $state("");
  let rerunError = $state("");
  let showDiarizeInfo = $state(false);
  let diarizeDialog: HTMLDialogElement | undefined = $state();
  let diarizeInfoButton: HTMLButtonElement | undefined = $state();
  $effect(() => {
    if (showDiarizeInfo && diarizeDialog && !diarizeDialog.open) diarizeDialog.showModal();
  });
  async function closeDiarizeInfo(): Promise<void> {
    showDiarizeInfo = false;
    await tick();
    diarizeInfoButton?.focus();
  }
  let transcribing = $derived(fileJobs.some(job => job.status === "queued" || job.status === "running")
    || Object.values(fileBusy).some(Boolean));

  $effect(() => {
    if (updatePreparing) return;
    const next = nextQueuedFile(fileJobs, Object.values(fileBusy).some(Boolean));
    if (next) {
      fileJobs = updateFileJob(fileJobs, next.id, "running");
    }
  });

  function handleFilesTranscription(paths: string[], openReview = false): void {
    if (updatePreparing) return;
    const jobs = createFileJobs(paths, {
      diarize: transcribeDiarize,
      prompt: transcribePrompt.trim() || null,
      profileId: selectedTranscribeProfile()?.id ?? null,
    }, () => crypto.randomUUID());
    if (!jobs.length) return;
    if (openReview) savedReviewIds = [...savedReviewIds, ...jobs.map(job => job.id)];
    else for (const job of jobs) {
      recentTranscriptions = rememberTranscription(recentTranscriptions, { path: job.path,
        profileId: job.profileId, prompt: job.prompt ?? "", diarize: job.diarize });
      selectedRerunPath = job.path;
    }
    const keepSelection = transcribing && selectedFileId !== null;
    fileJobs = [...fileJobs, ...jobs];
    if (!keepSelection) selectedFileId = jobs[0].id;
  }

  function completeFile(id: string, status: FileJobStatus): void {
    fileJobs = updateFileJob(fileJobs, id, status);
  }

  function retryLastTranscription(): void {
    const run = recentTranscriptions.find(run => run.path === selectedRerunPath);
    if (!run) return;
    if (run.profileId && !profileForId(run.profileId)) {
      rerunError = "The profile for this file is no longer available. Choose a profile and open the file again.";
      return;
    }
    rerunError = "";
    recentTranscriptions = rememberTranscription(recentTranscriptions, run);
    // A fresh queued job preserves every previous result and review. Only the
    // serial scheduler can start it; model/decoder settings apply at execution.
    const jobs = createFileJobs([run.path], { profileId: run.profileId,
      prompt: run.prompt || null, diarize: run.diarize }, () => crypto.randomUUID());
    fileJobs = [...fileJobs, ...jobs];
    selectedFileId = jobs[0].id;
  }

  function updateFileBusy(id: string, busy: boolean): void {
    if (fileBusy[id] !== busy) fileBusy = { ...fileBusy, [id]: busy };
  }

  function forgetMissingFile(path: string): void {
    recentTranscriptions = recentTranscriptions.filter(run => run.path !== path);
    if (selectedRerunPath === path) selectedRerunPath = recentTranscriptions[0]?.path ?? "";
  }

  function updateFileAttention(id: string, needsRetry: boolean): void {
    if (fileAttention[id] !== needsRetry) fileAttention = { ...fileAttention, [id]: needsRetry };
  }

  function showFileNeedingRetry(id: string): void {
    requestTabChange("transcribe", () => {
      selectedFileId = id;
      void tick().then(() => document.getElementById(`file-tab-${id}`)?.focus());
    });
  }

  function onResultTabKeydown(event: KeyboardEvent, index: number): void {
    let next = index;
    if (event.key === "ArrowRight") next = (index + 1) % fileJobs.length;
    else if (event.key === "ArrowLeft") next = (index + fileJobs.length - 1) % fileJobs.length;
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = fileJobs.length - 1;
    else return;
    event.preventDefault();
    selectedFileId = fileJobs[next].id;
    document.getElementById(`file-tab-${selectedFileId}`)?.focus();
  }

  // The global dictionary is retained as a decoder hint source. Explicit
  // language profiles are the only selectable sources for deterministic
  // glossary replacements.
  // Empty string is the UI-only global sentinel; profile IDs may legally be
  // "global", so that name cannot identify the global scope.
  let glossaryScopeId: string = $state("");
  let glossaryDraft: string = $state("");
  let glossaryDraftInitialized = false;
  let glossaryScopeGeneration = $state(0);
  let glossaryDraftGeneration = $state(0);
  let lastStoredGlossarySources: Record<string, string> = {};
  let glossaryEditBaseline: { scopeId: string; source: string; generation: number } | null = $state(null);
  let glossarySaving: boolean = $state(false);
  let glossarySaveInFlight: Promise<boolean> | null = null;
  type PendingGlossaryNavigation =
    | { kind: "scope"; scopeId: string }
    | { kind: "tab"; tab: SettingsTab; afterNavigate?: () => void };
  let pendingGlossaryNavigation: PendingGlossaryNavigation | null = $state(null);
  let glossaryDialogEl: HTMLDivElement | undefined = $state();
  let glossaryReturnFocusEl: HTMLElement | null = null;
  type RecoveredGlossaryDraft = { scopeId: string; draft: string; conflicted: boolean };
  type GlossarySaveRequest = {
    scopeId: string;
    generation: number;
    draftGeneration: number;
    value: string;
  };
  let recoveredGlossaryDrafts: RecoveredGlossaryDraft[] = $state([]);
  let glossaryConflictScopeId: string | null = $state(null);

  const dictionaryConflictPrefix = "Dictionary changed elsewhere:";

  // The dictionary editor is intentionally local state. Settings reloads may
  // update the saved source, but must never replace a draft that differs from
  // the source we last saw for this scope.
  function glossaryHasUnsavedChanges(): boolean {
    if (glossarySaving) return true;
    const baseline = glossaryEditBaseline;
    const draftChanged = baseline?.scopeId === glossaryScopeId
      && glossaryDraft !== baseline.source;
    const conflict = glossaryConflictScopeId === glossaryScopeId
      && settingsError.startsWith(dictionaryConflictPrefix);
    return Boolean(draftChanged || conflict);
  }

  $effect(() => {
    if (!pendingGlossaryNavigation || !glossaryDialogEl) return;
    queueMicrotask(() => {
      if (glossarySaving) {
        glossaryDialogEl?.focus();
      } else {
        glossaryDialogEl?.querySelector<HTMLButtonElement>("[data-dialog-stay]")?.focus();
      }
    });
  });

  function explicitProfiles(source: Settings | null = settings): HotkeyProfile[] {
    return source?.hotkey_profiles.filter((profile) => profile.language !== "auto") ?? [];
  }

  function profileForId(profileId: string | null, source: Settings | null = settings): HotkeyProfile | null {
    if (!profileId) return null;
    return explicitProfiles(source).find((profile) => profile.id === profileId) ?? null;
  }

  function glossarySourceForScope(scopeId: string, source: Settings | null = settings): string {
    if (!source || scopeId === "") return source?.initial_prompt ?? "";
    return source.profile_glossaries[scopeId] ?? "";
  }

  function isValidGlossaryScope(scopeId: string, source: Settings | null = settings): boolean {
    return scopeId === "" || profileForId(scopeId, source) !== null;
  }

  function glossaryScopeLabel(scopeId: string, source: Settings | null = settings): string {
    if (scopeId === "") return "Global hints";
    return profileForId(scopeId, source)?.name ?? scopeId;
  }

  function rememberGlossaryRecovery(scopeId: string, draft: string, conflicted = false): void {
    const existing = recoveredGlossaryDrafts.find(
      (recovery) => recovery.scopeId === scopeId && recovery.draft === draft,
    );
    if (existing) {
      if (conflicted && !existing.conflicted) {
        recoveredGlossaryDrafts = recoveredGlossaryDrafts.map((recovery) =>
          recovery === existing ? { ...recovery, conflicted: true } : recovery,
        );
      }
      return;
    }
    recoveredGlossaryDrafts = [...recoveredGlossaryDrafts, { scopeId, draft, conflicted }];
  }

  function removeGlossaryRecovery(scopeId: string, draft: string): void {
    recoveredGlossaryDrafts = recoveredGlossaryDrafts.filter(
      (recovery) => recovery.scopeId !== scopeId || recovery.draft !== draft,
    );
  }

  function isCurrentGlossaryRequest(request: GlossarySaveRequest): boolean {
    return request.generation === glossaryScopeGeneration
      && request.scopeId === glossaryScopeId
      && request.draftGeneration === glossaryDraftGeneration;
  }

  function selectedTranscribeProfile(): HotkeyProfile | null {
    return profileForId(transcribeProfileId);
  }

  function transcribeLanguage(): Language {
    return selectedTranscribeProfile()?.language ?? settings?.language ?? "auto";
  }

  $effect(() => {
    if (!settings) return;
    const language = transcribeLanguage();
    let stale = false;
    void Promise.all([getFileModelOptions(language), getEffectiveModelInfo(language)])
      .then(([options, automatic]) => {
        if (!stale) { fileModelOptions = Array.isArray(options) ? options : []; fileAutoModel = automatic; }
      })
      .catch((error) => {
        if (!stale) fileModelError = typeof error === "string" ? error : String(error);
      });
    return () => { stale = true; };
  });

  async function onFileModelChange(event: Event): Promise<void> {
    if (!settings || fileModelSaving) return;
    const id = (event.target as HTMLSelectElement).value;
    fileModelSaving = true;
    fileModelError = "";
    try {
      await setFileTranscriptionModel(id);
      settings = await getSettings();
      if (id === "pianissimo-sv") { transcribeDiarize = false; transcribePrompt = ""; }
    } catch (error) {
      fileModelError = typeof error === "string" ? error : String(error);
    } finally { fileModelSaving = false; }
  }

  async function downloadSelectedFileModel(): Promise<void> {
    const id = settings?.file_transcription_model;
    if (!id || id === "auto" || downloading !== null) return;
    downloading = id;
    downloadingName = fileModelOptions.find((model) => model.id === id)?.display_name ?? id;
    downloadProgress = 0;
    fileModelError = "";
    try {
      if (id === "pianissimo-sv") await downloadPianissimoModel();
      else await downloadModel(id);
      const options = await getFileModelOptions(transcribeLanguage());
      fileModelOptions = Array.isArray(options) ? options : [];
    } catch (error) {
      fileModelError = typeof error === "string" ? error : String(error);
    } finally { downloading = null; downloadProgress = 0; }
  }

  // Settings can be reloaded after hotkey/profile changes. Never leave a
  // removed or newly-Auto profile selected, and never show another scope's
  // text after that reconciliation.
  $effect(() => {
    const currentSettings = settings;
    const currentScope = glossaryScopeId;
    const currentTranscribeProfile = transcribeProfileId;
    if (!currentSettings) return;

    const currentStored = glossarySourceForScope(currentScope, currentSettings);
    const previousStored = lastStoredGlossarySources[currentScope];
    if (!glossaryDraftInitialized || glossaryDraft === previousStored) {
      glossaryDraft = currentStored;
      glossaryDraftInitialized = true;
    }
    lastStoredGlossarySources[currentScope] = currentStored;
    if (!isValidGlossaryScope(currentScope, currentSettings)) {
      const removedProfileHasDraft = currentScope !== ""
        && (
          glossaryEditBaseline?.scopeId === currentScope
          || (glossaryDraftInitialized && glossaryDraft !== (previousStored ?? currentStored))
        );
      if (removedProfileHasDraft) {
        rememberGlossaryRecovery(
          currentScope,
          glossaryDraft,
          glossaryConflictScopeId === currentScope && settingsError.startsWith(dictionaryConflictPrefix),
        );
      }
      glossaryScopeGeneration += 1;
      glossaryScopeId = "";
      glossaryDraft = currentSettings.initial_prompt;
      glossaryDraftGeneration += 1;
      glossaryEditBaseline = null;
      glossaryConflictScopeId = null;
    }
    if (currentTranscribeProfile && !profileForId(currentTranscribeProfile, currentSettings)) {
      transcribeProfileId = null;
    }
  });

  async function refreshProfileModels(profiles: HotkeyProfile[]) {
    const generation = ++profileModelRefresh;
    try {
      const entries = await Promise.all(
        profiles.map(async (profile) => [
          profile.id,
          await getEffectiveModelInfo(profile.language),
        ] as const),
      );
      if (generation === profileModelRefresh) {
        profileModels = Object.fromEntries(entries);
      }
    } catch (e: any) {
      if (generation === profileModelRefresh) {
        settingsError = typeof e === "string" ? e : e?.message || "Failed to check speech engines.";
      }
    }
  }

  onMount(() => {
    let disposed = false;
    let recoveryStop: (() => void) | null = null;
    let recoveryAbortStop: (() => void) | null = null;
    const recoveryListener = listen<unknown>("update-preparing", (event) => {
      const payload = event.payload;
      const nonce = typeof payload === "string"
        ? payload
        : payload && typeof payload === "object" && "nonce" in payload && typeof payload.nonce === "string"
          ? payload.nonce
          : null;
      if (nonce && !disposed) {
        updatePreparing = true;
        void prepareForUpdate(nonce);
      }
    });
    recoveryListener.then((stop) => {
      if (disposed) stop();
      else recoveryStop = stop;
    }).catch((error) => console.warn("Could not listen for update preparation", error));
    listen("update-aborted", () => { updatePreparing = false; }).then((stop) => {
      if (disposed) stop();
      else recoveryAbortStop = stop;
    }).catch((error) => console.warn("Could not listen for update abort", error));
    recoveryRestore = restoreUpdateRecovery();
    void recoveryRestore.catch(() => undefined);

    // Register listeners + drag-drop FIRST — they don't depend on the data
    // fetched below, so a rejected invoke in the fetch sequence must never
    // prevent them from wiring up (e.g. a stuck-at-0% download).
    listen("model-download-progress", (event: any) => {
      downloadProgress = event.payload.progress;
    });

    listen("model-ready", async () => {
      downloading = null;
      downloadProgress = 0;
      models = await getModelInfo();
      if (settings) await refreshProfileModels(settings.hotkey_profiles);
    });

    // Hotkey registration health can change at any time (settings-file
    // hot-reload, a failed re-register racing a Spotlight/Raycast combo
    // claim, etc.) — not just as a result of something this window did.
    listen("hotkey-registration-changed", (event: any) => {
      const status = event.payload as HotkeyStatus;
      hotkeyStatusOk = status.ok;
      hotkeyStatusError = status.error ?? "";
    });

    // Keep Dictate synchronized with hotkey-driven work. Previously these
    // states were ignored, so the button still offered "Start recording"
    // while the backend was loading/transcribing and the next click produced
    // a misleading busy error.
    listen("state-changed", async (event: any) => {
      const nextState = event.payload;
      if (nextState === "settings_reloaded") {
        settings = await getSettings();
        models = await getModelInfo();
        await refreshProfileModels(settings.hotkey_profiles);
        return;
      }

      if (!["idle", "recording", "loading_model", "transcribing"].includes(nextState)) return;
      const acceptedState = nextState as BackendDictationState;
      testOwnsRecording = retainTestRecordingOwnership(acceptedState, testOwnsRecording);
      backendDictationState = acceptedState;
      testRecording = nextState === "recording";
      testTranscribing = nextState === "loading_model" || nextState === "transcribing";
    });

    // Listen for tab navigation from tray menu
    listen("navigate_tab", (event: any) => {
      const t = event.payload;
      if (t === "dictate" || t === "transcribe" || t === "settings") {
        requestTabChange(t);
      }
    });

    // Listen for drag-and-drop events
    const webview = getCurrentWebview();
    webview.onDragDropEvent((event) => {
      if (event.payload.type === "over") {
        dragOver = true;
      } else if (event.payload.type === "drop") {
        dragOver = false;
        const paths = event.payload.paths;
        if (paths.length > 0 && !updatePreparing) {
          requestTabChange("transcribe", () => handleFilesTranscription(paths));
        }
      } else {
        dragOver = false;
      }
    });

    // Fetch initial data. A rejection here surfaces as a visible inline
    // error instead of leaving a half-initialized window.
    (async () => {
      initError = "";
      try {
        buildInfo = await getBuildInfo();
      } catch (error) {
        // Build identity is diagnostic only. Keep it independent from the
        // settings bootstrap so it remains visible when another query fails.
        console.warn("Failed to load build information", error);
      }
      try {
        settings = await getSettings();
        await refreshProfileModels(settings.hotkey_profiles);
        platform = await getPlatform();
        if (platform === "macos") {
          accessibilityGranted = await checkAccessibilityPermission();
        }
        models = await getModelInfo();
        supportedFormats = await getSupportedFormats();
        const status = await hotkeyStatus();
        hotkeyStatusOk = status.ok;
        hotkeyStatusError = status.error ?? "";
        if (
          platform === "macos" &&
          accessibilityGranted &&
          !status.ok &&
          configuredShortcutsUseBareHotkey()
        ) {
          await refreshHotkeyRegistration();
        }

        // Check URL params for initial tab
        const params = new URLSearchParams(window.location.search);
        const tab = params.get("tab");
        if (tab === "dictate" || tab === "transcribe" || tab === "settings") {
          requestTabChange(tab);
        }
      } catch (e: any) {
        initError = typeof e === "string" ? e : e?.message || "Failed to load settings.";
      }
    })();
    return () => {
      disposed = true;
      recoveryStop?.();
      recoveryAbortStop?.();
    };
  });

  /**
   * Shared wrapper for the common "mutate then refresh settings" pattern.
   * On failure: records a visible error and forces bound controls back to
   * last-known-good by reassigning `settings` to a fresh object (one-way
   * bindings re-render from state, so a native control that already shows
   * the rejected value snaps back). Never re-throws.
   */
  async function applySetting(
    mutate: () => Promise<void>,
    errorSink?: { value: string },
    reportError = true,
  ): Promise<boolean> {
    if (reportError) settingsError = "";
    try {
      await mutate();
      settings = await getSettings();
      await refreshProfileModels(settings.hotkey_profiles);
      return true;
    } catch (e: any) {
      const message = typeof e === "string" ? e : e?.message || "Failed to save setting.";
      if (reportError) settingsError = message;
      if (errorSink) errorSink.value = message;
      if (settings) settings = { ...settings };
      return false;
    }
  }

  async function onLanguageChange(e: Event) {
    const select = e.currentTarget as HTMLSelectElement;
    const value = select.value as Language;
    if (!settings) return;
    // Native selects change before the async handler. A rejected value must
    // never remain visible merely because Svelte's saved value did not change.
    select.value = settings.language;
    if (languageSaving || glossarySaving) return;
    blockedLanguageChange = null;
    languageSaving = true;
    try {
      const ok = await applySetting(() => setLanguage(value));
      if (!ok && settingsError.includes(defaultDictionaryLanguageBlock)) {
        // Show the actual blocking dictionary, not whichever editor scope is
        // selected. Freeze the preview for the existing compare-and-swap API.
        const current = await getSettings();
        settings = current;
        const source = current.profile_glossaries.default ?? "";
        if (source.trim()) blockedLanguageChange = { language: value, source };
        else settingsError = "The blocking dictionary was cleared elsewhere. Select the language again to retry.";
      }
      if (ok) models = await getModelInfo();
    } catch (error: any) {
      settingsError += `${settingsError ? " " : ""}Could not refresh language settings; reopen Settings to confirm the saved language: ${typeof error === "string" ? error : error?.message || "Unknown error"}`;
    } finally {
      select.value = settings.language;
      languageSaving = false;
      if (!blockedLanguageChange) void restoreLanguageFocus();
    }
  }

  async function clearDictionaryAndSwitchLanguage(): Promise<void> {
    if (!settings || !blockedLanguageChange || languageSaving || glossarySaving) return;
    const request = blockedLanguageChange;
    const defaultDraft = glossaryScopeId === "default" && glossaryHasUnsavedChanges()
      ? glossaryDraft : null;
    languageSaving = true;
    settingsError = "";
    let cleared = false;
    try {
      await setProfileGlossary("default", "", request.source);
      cleared = true;
      // Clear the confirmation immediately: a failed retry must not offer to
      // erase a dictionary that may have been populated again in the meantime.
      blockedLanguageChange = null;
      if (defaultDraft !== null) rememberGlossaryRecovery("default", defaultDraft);
      settings = { ...settings, profile_glossaries: { ...settings.profile_glossaries, default: "" } };
      if (glossaryScopeId === "default") discardGlossaryChanges();
      await setLanguage(request.language);
    } catch (error: any) {
      const message = typeof error === "string" ? error : error?.message || "Unknown error";
      if (cleared) rememberGlossaryRecovery("default", request.source);
      settingsError = cleared
        ? `The default dictionary was cleared, but the language change failed: ${message}`
        : `The dictionary could not be cleared; the language was not changed: ${message}`;
      blockedLanguageChange = null;
    } finally {
      try {
        settings = await getSettings();
        models = await getModelInfo();
        await refreshProfileModels(settings.hotkey_profiles);
      } catch (error: any) {
        settingsError += `${settingsError ? " " : ""}Could not refresh settings; reopen Settings to confirm the saved language: ${typeof error === "string" ? error : error?.message || "Unknown error"}`;
      }
      languageSaving = false;
      void restoreLanguageFocus();
    }
  }

  async function onAutoPasteToggle() {
    if (!settings) return;
    const enabling = !settings.auto_paste;
    if (!enabling || platform !== "macos") {
      await applySetting(() => setAutoPaste(enabling));
      return;
    }

    // Keep the preference off until TCC actually reports approval. This is an
    // explicit user action, so it is the one place where prompting is allowed.
    accessibilityChecking = true;
    accessibilityRequested = true;
    settingsError = "";
    try {
      accessibilityGranted = await checkAccessibilityPermission();
      if (!accessibilityGranted) {
        await requestAccessibilityPermission();
        accessibilityGranted = await waitForAccessibilityPermission();
      }

      if (accessibilityGranted) {
        await applySetting(() => setAutoPaste(true));
      } else {
        await applySetting(() => setAutoPaste(false));
        settingsError = "Accessibility permission was not granted. Auto-paste remains off.";
      }
    } catch (e: any) {
      await applySetting(() => setAutoPaste(false));
      settingsError = typeof e === "string" ? e : e?.message || "Failed to check Accessibility permission.";
    } finally {
      accessibilityChecking = false;
    }
  }

  async function waitForAccessibilityPermission(): Promise<boolean> {
    // System Settings can take a while to update TCC. Recheck for at most one
    // minute; no unbounded timer survives after this explicit attempt.
    for (let attempt = 0; attempt < 60; attempt += 1) {
      await new Promise((resolve) => setTimeout(resolve, 1000));
      if (await checkAccessibilityPermission()) {
        await refreshHotkeyRegistration();
        return true;
      }
    }
    return false;
  }

  function configuredShortcutsUseBareHotkey(): boolean {
    // Only bare F13–F24 registrations depend on the macOS Accessibility
    // grant, so only those benefit from an automatic retry on Settings
    // open. Retrying other failures (e.g. shortcut-in-use) would just churn
    // unregister/register without helping. Gated on shortcut content, not
    // on backend error text, to avoid fragile string coupling.
    if (!settings) return false;
    const shortcuts = [
      settings.hotkey,
      ...allProfileShortcutValues(settings.hotkey_profiles, settings.hotkey_mode),
    ];
    return shortcuts.some((shortcut) => canUseBareHotkey(shortcut, platform));
  }

  async function refreshHotkeyRegistration(): Promise<void> {    try {
      await retryHotkeyRegistration();
      settings = await getSettings();
      await refreshProfileModels(settings.hotkey_profiles);
    } catch (error) {
      // The registration-health response below contains the backend's full
      // diagnostic and keeps the failure visible in Settings.
      console.warn("Failed to retry hotkey registration", error);
    }
    const status = await hotkeyStatus();
    hotkeyStatusOk = status.ok;
    hotkeyStatusError = status.error ?? "";
  }

  async function onShowOverlayToggle() {
    if (!settings) return;
    const next = !settings.show_overlay;
    await applySetting(() => setShowOverlay(next));
  }

  async function refreshDictionaryAfterConflict(primaryError: string, request: GlossarySaveRequest) {
    try {
      settings = await getSettings();
    } catch (error) {
      console.warn("Could not refresh the dictionary after a concurrent change", error);
    }
    // A stale request still refreshes the source of truth, but cannot replace
    // a newer scope's error or draft. The recovery item carries its context.
    if (isCurrentGlossaryRequest(request)) {
      settingsError = primaryError;
      glossaryConflictScopeId = request.scopeId;
    } else {
      rememberGlossaryRecovery(request.scopeId, request.value, true);
    }
  }

  async function saveGlossary(): Promise<boolean> {
    if (languageSaving) return false;
    if (glossarySaveInFlight) return glossarySaveInFlight;

    const request: GlossarySaveRequest = {
      scopeId: glossaryScopeId,
      generation: glossaryScopeGeneration,
      draftGeneration: glossaryDraftGeneration,
      value: glossaryDraft,
    };
    const scopeId = request.scopeId;
    const draftGeneration = request.draftGeneration;
    const value = request.value;
    const editBaseline = glossaryEditBaseline;
    const expectedSource = editBaseline?.scopeId === scopeId
      && editBaseline.generation <= draftGeneration
      ? editBaseline.source
      : lastStoredGlossarySources[scopeId] ?? glossarySourceForScope(scopeId);

    if (!settings || !isValidGlossaryScope(scopeId)) return false;
    if (!glossaryHasUnsavedChanges()) {
      if (glossaryEditBaseline?.scopeId === scopeId) glossaryEditBaseline = null;
      return true;
    }

    const saveError = { value: "" };
    const operation = (async (): Promise<boolean> => {
      glossarySaving = true;
      settingsError = "";
      try {
        const saved = await applySetting(() => scopeId === ""
          ? setInitialPrompt(value, expectedSource)
          : setProfileGlossary(scopeId, value, expectedSource), saveError, false);
        const conflict = saveError.value.startsWith(dictionaryConflictPrefix);
        let requestIsCurrent = isCurrentGlossaryRequest(request);
        if (conflict) {
          await refreshDictionaryAfterConflict(saveError.value, request);
          requestIsCurrent = isCurrentGlossaryRequest(request);
        } else if (!saved && !requestIsCurrent) {
          rememberGlossaryRecovery(scopeId, value);
        }

        if (requestIsCurrent) {
          settingsError = saved ? "" : saveError.value;
          if (saved) glossaryConflictScopeId = null;
        }
        if (saved) removeGlossaryRecovery(scopeId, value);

        // If our own save won the CAS race while the user kept typing in the
        // same edit lineage, advance only that lineage's baseline to our
        // value. A stale request never clears a newer draft.
        if (
          saved
          && editBaseline
          && glossaryEditBaseline === editBaseline
          && editBaseline.scopeId === scopeId
        ) {
          if (draftGeneration === glossaryDraftGeneration) {
            glossaryEditBaseline = null;
          } else {
            glossaryEditBaseline = { ...editBaseline, source: value };
          }
        }

        // A scope removal/reload while the invoke was pending owns the
        // textarea now; never navigate based on a stale request.
        return saved && requestIsCurrent;
      } finally {
        glossarySaving = false;
      }
    })();
    glossarySaveInFlight = operation;
    try {
      return await operation;
    } finally {
      if (glossarySaveInFlight === operation) glossarySaveInFlight = null;
    }
  }

  function onGlossaryInput(e: Event) {
    if (!glossaryEditBaseline || glossaryEditBaseline.scopeId !== glossaryScopeId) {
      glossaryEditBaseline = {
        scopeId: glossaryScopeId,
        source: lastStoredGlossarySources[glossaryScopeId] ?? glossarySourceForScope(glossaryScopeId),
        generation: glossaryDraftGeneration + 1,
      };
    }
    glossaryDraftGeneration += 1;
    glossaryDraft = (e.target as HTMLTextAreaElement).value;
  }

  function discardGlossaryChanges(): void {
    if (!settings || glossarySaving) return;
    const currentSource = glossarySourceForScope(glossaryScopeId, settings);
    glossaryDraftGeneration += 1;
    glossaryDraft = currentSource;
    glossaryDraftInitialized = true;
    lastStoredGlossarySources[glossaryScopeId] = currentSource;
    glossaryEditBaseline = null;
    if (glossaryConflictScopeId === glossaryScopeId) {
      glossaryConflictScopeId = null;
      if (settingsError.startsWith(dictionaryConflictPrefix)) settingsError = "";
    }
  }

  function commitGlossaryScopeChange(nextScope: string): void {
    if (!settings || !isValidGlossaryScope(nextScope)) return;
    glossaryScopeGeneration += 1;
    glossaryDraftGeneration += 1;
    glossaryEditBaseline = null;
    glossaryConflictScopeId = null;
    settingsError = settingsError.startsWith(dictionaryConflictPrefix) ? "" : settingsError;
    glossaryScopeId = nextScope;
    glossaryDraft = glossarySourceForScope(nextScope, settings);
    glossaryDraftInitialized = true;
    lastStoredGlossarySources[nextScope] = glossaryDraft;
  }

  function onGlossaryScopeChange(e: Event) {
    const nextScope = (e.target as HTMLSelectElement).value;
    if (!settings || !isValidGlossaryScope(nextScope)) return;
    if (nextScope === glossaryScopeId) return;
    if (glossaryHasUnsavedChanges()) {
      // The browser changes a select's displayed value before onchange fires.
      // Restore the current scope until the user makes an explicit decision.
      (e.currentTarget as HTMLSelectElement).value = glossaryScopeId;
      promptGlossaryNavigation({ kind: "scope", scopeId: nextScope }, e.currentTarget as HTMLElement);
      return;
    }
    commitGlossaryScopeChange(nextScope);
  }

  function promptGlossaryNavigation(
    pending: PendingGlossaryNavigation,
    returnFocusEl?: HTMLElement,
  ): void {
    const activeElement = returnFocusEl
      ?? (typeof document === "undefined" ? null : document.activeElement);
    glossaryReturnFocusEl = activeElement instanceof HTMLElement ? activeElement : null;
    pendingGlossaryNavigation = pending;
  }

  function restoreGlossaryNavigationFocus(): void {
    const returnFocusEl = glossaryReturnFocusEl;
    glossaryReturnFocusEl = null;
    queueMicrotask(() => {
      if (returnFocusEl?.isConnected && !returnFocusEl.matches(":disabled")) {
        returnFocusEl.focus();
      }
    });
  }

  function onGlossaryDialogKeydown(event: KeyboardEvent): void {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      if (!glossarySaving) stayOnGlossaryDraft();
      return;
    }
    if (event.key !== "Tab") return;

    const dialog = glossaryDialogEl;
    if (!dialog) return;
    const focusable = Array.from(dialog.querySelectorAll<HTMLElement>(
      "button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex='-1'])",
    )).filter((element) => !element.matches(":disabled"));
    event.preventDefault();
    if (focusable.length === 0) {
      dialog.focus();
      return;
    }

    const activeElement = typeof document === "undefined" ? null : document.activeElement;
    const activeIndex = activeElement ? focusable.indexOf(activeElement as HTMLElement) : -1;
    const nextIndex = activeIndex < 0
      ? (event.shiftKey ? focusable.length - 1 : 0)
      : (activeIndex + (event.shiftKey ? -1 : 1) + focusable.length) % focusable.length;
    focusable[nextIndex]?.focus();
  }

  function finishPendingGlossaryNavigation(pending: PendingGlossaryNavigation): void {
    pendingGlossaryNavigation = null;
    restoreGlossaryNavigationFocus();
    if (pending.kind === "scope") {
      commitGlossaryScopeChange(pending.scopeId);
    } else {
      activeTab = pending.tab;
        pending.afterNavigate?.();
    }
  }

  async function saveAndFinishGlossaryNavigation(): Promise<void> {
    const pending = pendingGlossaryNavigation;
    if (!pending || glossarySaving || languageSaving) return;
    if (await saveGlossary() && pendingGlossaryNavigation === pending) {
      finishPendingGlossaryNavigation(pending);
    }
  }

  function discardAndFinishGlossaryNavigation(): void {
    const pending = pendingGlossaryNavigation;
    if (!pending || glossarySaving || languageSaving) return;
    discardGlossaryChanges();
    finishPendingGlossaryNavigation(pending);
  }

  function stayOnGlossaryDraft(): void {
    pendingGlossaryNavigation = null;
    restoreGlossaryNavigationFocus();
  }

  function requestTabChange(nextTab: SettingsTab, afterNavigate?: () => void): void {
    if (languageSaving) return;
    if (nextTab === activeTab) {
      afterNavigate?.();
      return;
    }
    if (glossarySaving) return;
    if (glossaryHasUnsavedChanges()) {
      promptGlossaryNavigation({ kind: "tab", tab: nextTab, afterNavigate });
      return;
    }
    activeTab = nextTab;
    afterNavigate?.();
  }

  async function onBeamSizeChange(e: Event) {
    const value = Number((e.target as HTMLSelectElement).value);
    await applySetting(() => setBeamSize(value));
  }

  async function onTemperatureFallbackToggle() {
    if (!settings) return;
    const next = !settings.temperature_fallback;
    await applySetting(() => setTemperatureFallback(next));
  }

  async function onVadToggle() {
    if (!settings) return;
    const next = !settings.vad_enabled;
    await applySetting(() => setVadEnabled(next));
  }

  async function selectModel(model: WhisperModel) {
    if (selecting) return;
    selecting = true;
    modelError = "";
    try {
      // The backend verifies Ready models and replaces only artifacts whose
      // bytes provably fail the immutable integrity manifest.
      downloading = model.id;
      downloadingName = model.display_name;
      downloadProgress = 0;
      await downloadModel(model.id);
      // model_ready event will refresh the list
      await setWhisperModel(model.id);
      settings = await getSettings();
      models = await getModelInfo();
    } catch (e: any) {
      modelError = typeof e === "string" ? e : e?.message || "Model selection failed.";
    } finally {
      downloading = null;
      downloadProgress = 0;
      selecting = false;
    }
  }

  async function downloadProfileModel(profile: HotkeyProfile) {
    const model = profileModels[profile.id];
    if (!model || model.downloaded || downloading !== null) return;
    downloading = model.id;
    downloadingName = model.display_name;
    downloadProgress = 0;
    profileModelErrors = { ...profileModelErrors, [profile.id]: "" };
    try {
      await downloadModel(model.id);
      await refreshProfileModels(settings?.hotkey_profiles ?? []);
      models = await getModelInfo();
    } catch (e: any) {
      profileModelErrors = {
        ...profileModelErrors,
        [profile.id]: typeof e === "string" ? e : e?.message || "Speech engine download failed.",
      };
    } finally {
      downloading = null;
      downloadProgress = 0;
    }
  }

  async function onTestRecord() {
    if (updatePreparing) return;
    const action = dictateButtonAction(backendDictationState, testOwnsRecording);
    if (action === "blocked") return;

    if (action === "stop") {
      // Stop and transcribe
      testOwnsRecording = false;
      testRecording = false;
      testTranscribing = true;
      backendDictationState = "transcribing";
      testError = "";
      try {
        const text = await stopAndTranscribe();
        observedNativeDictation = text;
        testResultRecoveryPending = true;
        testResult = testResult ? testResult + " " + text : text;
      } catch (e: any) {
        testError = typeof e === "string" ? e : e.message || "Transcription failed";
      } finally {
        testTranscribing = false;
        backendDictationState = "idle";
      }
    } else {
      // Start recording
      testError = "";
      try {
        await startRecording();
        testOwnsRecording = true;
        testRecording = true;
        backendDictationState = "recording";
      } catch (e: any) {
        testOwnsRecording = false;
        backendDictationState = "idle";
        testError = typeof e === "string" ? e : e.message || "Failed to start recording";
      }
    }
  }

  function onTranscribeProfileChange(e: Event) {
    const nextProfileId = (e.target as HTMLSelectElement).value;
    transcribeProfileId = profileForId(nextProfileId)?.id ?? null;
  }

  async function onPickFile() {
    if (updatePreparing) return;
    const exts = supportedFormats.length > 0 ? supportedFormats : ["wav", "mp3", "m4a", "mp4", "ogg", "flac"];
    const file = await open({
      multiple: true,
      filters: [
        {
          name: "Audio/Video",
          extensions: exts,
        },
      ],
    });
    if (file && !updatePreparing) {
      handleFilesTranscription(file);
    }
  }

  /** Platform-correct modifier display names */
  function modifierNames(): { ctrl: string; alt: string; meta: string } {
    const mac = platform === "macos";
    return {
      ctrl: mac ? "Control" : "Ctrl",
      alt: mac ? "Option" : "Alt",
      meta: mac ? "Cmd" : "Win",
    };
  }

  /** Format a shortcut string for display (e.g. "Control+Shift+Space" → "Ctrl + Shift + Space") */
  function formatHotkeyDisplay(shortcut: string): string {
    return formatShortcutDisplay(shortcut, platform);
  }

  function beginHotkeyCapture(profileId: string, slot: ExplicitShortcutSlot) {
    hotkeyCaptureGeneration += 1;
    recordingProfileId = profileId;
    recordingShortcutSlot = slot;
    hotkeyError = "";
  }

  async function onHotkeyKeydown(e: KeyboardEvent, profileId: string, slot: ExplicitShortcutSlot) {
    const captureGeneration = hotkeyCaptureGeneration;
    e.preventDefault();
    e.stopPropagation();

    // Escape cancels recording
    if (e.key === "Escape") {
      hotkeyCaptureGeneration += 1;
      recordingProfileId = null;
      recordingShortcutSlot = null;
      hotkeyError = "";
      return;
    }

    // Ignore bare modifier presses — wait for a non-modifier key
    if (["Control", "Shift", "Alt", "Meta"].includes(e.key)) return;

    const keyName = tauriKeyName(e.key, e.code, platform);
    if (!keyName) {
      hotkeyError = e.code === "IntlBackslash"
        ? "The ISO section key is supported only on macOS and Windows. Choose another key."
        : `"${e.key}" is not a supported key. Use A–Z, 0–9, F1–F24, Space, Arrow keys, or Tab/Enter/Delete.`;
      return;
    }

    // Ordinary keys require a modifier. Extended function keys are reserved
    // for programmable buttons and may be used directly through the native
    // macOS monitor or the Windows global-shortcut backend.
    const hasModifier = e.ctrlKey || e.altKey || e.metaKey || e.shiftKey;
    if (platform === "macos" && hasModifier && /^F2[1-4]$/.test(keyName)) {
      hotkeyError = `${keyName} is supported without modifiers on macOS, but its modified forms cannot be registered reliably.`;
      return;
    }
    if (!hasModifier && !canUseBareHotkey(keyName, platform)) {
      const m = modifierNames();
      const bareRange = supportedBareFunctionKeyRange(platform);
      hotkeyError = `Shortcut must include a modifier (${m.ctrl}, ${m.alt}, ${m.meta}, or Shift).${bareRange ? ` ${bareRange} may be used alone.` : ""}`;
      return;
    }

    if (platform === "macos" && !hasModifier && /^F(?:1[3-9]|2[0-4])$/.test(keyName)) {
      accessibilityChecking = true;
      try {
        accessibilityGranted = await checkAccessibilityPermission();
        if (!accessibilityGranted) {
          await requestAccessibilityPermission();
          accessibilityGranted = await waitForAccessibilityPermission();
        }
      } catch (error: any) {
        hotkeyError = typeof error === "string" ? error : error?.message || "Failed to check Accessibility permission.";
        return;
      } finally {
        accessibilityChecking = false;
      }
      if (!accessibilityGranted) {
        hotkeyError = "F13–F24 requires Accessibility permission on macOS. Permission was not granted.";
        return;
      }
      if (captureGeneration !== hotkeyCaptureGeneration) return;
    }

    // Build Tauri-format shortcut string (order: Control, Alt, Super, Shift, Key)
    // Note: muda crate uses "Super" (not "Meta") for Cmd on macOS / Win key on Windows
    const parts: string[] = [];
    if (e.ctrlKey) parts.push("Control");
    if (e.altKey) parts.push("Alt");
    if (e.metaKey) parts.push("Super");
    if (e.shiftKey) parts.push("Shift");
    parts.push(keyName);

    const shortcut = parts.join("+");
    hotkeyError = "";

    const currentSettings = settings;
    if (!currentSettings) return;
    const profiles = currentSettings.hotkey_profiles.map((profile) =>
      profile.id === profileId
        ? profileWithShortcut(profile, slot, shortcut, currentSettings.hotkey_mode)
        : profile
    );
    setHotkeyProfiles(profiles)
      .then(async () => {
        recordingProfileId = null;
        recordingShortcutSlot = null;
        draftProfileId = null;
        settings = await getSettings();
        await refreshProfileModels(settings.hotkey_profiles);
      })
      .catch((err: any) => {
        hotkeyError = typeof err === "string" ? err : err.message || "Failed to set hotkey";
      });
  }

  async function clearProfileShortcut(profileId: string, slot: ExplicitShortcutSlot) {
    const currentSettings = settings;
    if (!currentSettings) return;
    const profile = currentSettings.hotkey_profiles.find((candidate) => candidate.id === profileId);
    if (!profile || !profile.push_to_talk_shortcut || !profile.toggle_shortcut) return;
    const profiles = currentSettings.hotkey_profiles.map((candidate) =>
      candidate.id === profileId ? profileWithShortcut(candidate, slot, null, currentSettings.hotkey_mode) : candidate
    );
    if (profileId === draftProfileId) {
      settings = { ...currentSettings, hotkey_profiles: profiles };
      await refreshProfileModels(profiles);
      return;
    }
    const saved = await applySetting(() => setHotkeyProfiles(profiles));
    if (!saved && settingsError) hotkeyError = settingsError;
  }

  async function updateProfile(profileId: string, changes: Partial<HotkeyProfile>) {
    if (!settings) return;
    const profiles = settings.hotkey_profiles.map((profile) =>
      profile.id === profileId ? { ...profile, ...changes } : profile
    );
    if (profileId === draftProfileId) {
      settings = { ...settings, hotkey_profiles: profiles };
      await refreshProfileModels(profiles);
      return;
    }
    await applySetting(() => setHotkeyProfiles(profiles));
  }

  function addProfile() {
    if (!settings) return;
    let suffix = settings.hotkey_profiles.length + 1;
    while (
      settings.hotkey_profiles.some((profile) => profile.id === `profile-${suffix}`)
      || Object.hasOwn(settings.profile_glossaries, `profile-${suffix}`)
    ) suffix += 1;
    const suggested = suggestedProfileShortcuts(
      settings.hotkey_profiles,
      settings.hotkey,
      platform,
    );
    const profile: HotkeyProfile = {
      id: `profile-${suffix}`,
      name: `Profile ${suffix}`,
      ...suggested,
      language: settings.language === "sv" ? "en" : "sv",
    };
    settings = { ...settings, hotkey_profiles: [...settings.hotkey_profiles, profile] };
    draftProfileId = profile.id;
    beginHotkeyCapture(profile.id, "push_to_talk_shortcut");
    void refreshProfileModels(settings.hotkey_profiles);
  }

  async function removeProfile(profileId: string) {
    if (!settings || settings.hotkey_profiles.length <= 1) return;
    if (profileId === draftProfileId) {
      settings = { ...settings, hotkey_profiles: settings.hotkey_profiles.filter((profile) => profile.id !== profileId) };
      draftProfileId = null;
      recordingProfileId = null;
      recordingShortcutSlot = null;
      return;
    }
    await applySetting(() =>
      setHotkeyProfiles(settings!.hotkey_profiles.filter((profile) => profile.id !== profileId)),
    );
  }

  function languageLabel(lang: Language): string {
    switch (lang) {
      case "sv": return "Swedish";
      case "no": return "Norwegian";
      case "fi": return "Finnish";
      case "en": return "English";
      default: return "Auto-detect";
    }
  }

</script>

<div class="settings-window">
  <header class="window-header">
    <h1 class="window-title">Sagascript</h1>
    <div class="build-info" aria-label="Build information">
      {#if buildInfo}
        Version {buildInfo.version} · Build {buildInfo.git_hash} · {buildInfo.build_date}
      {:else}
        Version information unavailable
      {/if}
    </div>
  </header>

  <div class="tabs">
    <button class="tab" class:active={activeTab === "dictate"} onclick={() => requestTabChange("dictate")} disabled={languageSaving}>
      Dictate
    </button>
    <button class="tab" class:active={activeTab === "transcribe"} onclick={() => requestTabChange("transcribe")} disabled={languageSaving}>
      Transcribe
    </button>
    <button class="tab" class:active={activeTab === "settings"} onclick={() => requestTabChange("settings")} disabled={languageSaving}>
      Settings
    </button>
  </div>

  {#if settings}
    <div class="content">
      {#if initError}
        <div class="transcribe-error">{initError}</div>
      {/if}
      {#if settingsError}
        <div class="transcribe-error" role={pendingGlossaryNavigation ? undefined : "alert"}>{settingsError}</div>
      {/if}
      {#if recoveredDraftsNotice}
        <section class="recovery-notice" role="status" aria-label="Recovered drafts">
          <strong>Recovered drafts</strong>
          <span>Unsaved dictation and transcription results were restored after the update.</span>
          <button class="secondary" type="button" disabled={updatePreparing}
            onclick={() => void discardRecoveredDrafts()}>Discard recovered drafts</button>
        </section>
      {/if}
      <p class="queue-summary" class:queue-empty={fileJobs.length === 0} role="status" aria-label="File transcription status" aria-atomic="true">
        {#if fileJobs.length}
          {fileJobs.filter(job => job.status === "completed" && !fileBusy[job.id]).length} completed ·
          {fileJobs.filter(job => job.status === "failed").length} failed ·
          {fileJobs.filter(job => job.status === "cancelled").length} cancelled ·
          {fileJobs.filter(job => job.status === "queued").length} queued ·
          {fileJobs.filter(job => job.status === "running" || fileBusy[job.id]).length} running ·
          {filesNeedingRetry.length} needs retry
        {/if}
      </p>
      {#if filesNeedingRetry.length}
        <div class="queue-attention">
          {#each filesNeedingRetry as job (job.id)}
            <div>
              <span>{fileJobName(job.path)} needs a status check.{#if fileJobs.some(file => file.status === "queued")} The queue is paused.{/if}</span>
              <button class="secondary" onclick={() => showFileNeedingRetry(job.id)}>Show {fileJobName(job.path)}</button>
            </div>
          {/each}
        </div>
      {/if}
      {#if activeTab === "dictate"}
        <div class="field profile-field">
          <div class="profile-heading">
            <span class="field-label">Dictation shortcuts</span>
            <button class="link-btn" onclick={addProfile}>+ Add language</button>
          </div>
          {#each settings.hotkey_profiles as profile (profile.id)}
            <div class="profile-card">
              <div class="profile-row">
                <input
                  type="text"
                  class="profile-name"
                  aria-label="Profile name"
                  value={profile.name}
                  onblur={(event) => updateProfile(profile.id, { name: (event.target as HTMLInputElement).value })}
                />
                <select
                  aria-label={`${profile.name} language`}
                  value={profile.language}
                  onchange={(event) => updateProfile(profile.id, { language: (event.target as HTMLSelectElement).value as Language })}
                >
                  <option value="en">English</option>
                  <option value="sv">Swedish</option>
                  <option value="no">Norwegian</option>
                  <option value="fi">Finnish</option>
                  <option value="auto">Auto-detect</option>
                </select>
              </div>
              <div class="shortcut-grid">
                {#each shortcutControls as shortcutControl}
                  <div class="shortcut-control">
                    <span class="shortcut-label">{shortcutControl.label}</span>
                    {#if recordingProfileId === profile.id && recordingShortcutSlot === shortcutControl.slot}
                      <button
                        class="hotkey-recorder recording"
                        bind:this={hotkeyRecorderEl}
                        aria-label={`${profile.name} ${shortcutControl.label} shortcut`}
                        onkeydown={(event) => onHotkeyKeydown(event, profile.id, shortcutControl.slot)}
                        onblur={() => { recordingProfileId = null; recordingShortcutSlot = null; hotkeyError = ""; }}
                      >Press shortcut...</button>
                    {:else}
                      <button
                        class="hotkey-recorder"
                        aria-label={`${profile.name} ${shortcutControl.label} shortcut`}
                        onclick={() => beginHotkeyCapture(profile.id, shortcutControl.slot)}
                      >{formatHotkeyDisplay(displayProfileShortcut(profile, shortcutControl.slot, settings.hotkey_mode) || "Not set")}</button>
                    {/if}
                    <div class="shortcut-helper">{shortcutControl.helper}</div>
                    {#if profile.push_to_talk_shortcut && profile.toggle_shortcut}
                      <button
                        type="button"
                        class="shortcut-clear"
                        aria-label={`Clear ${shortcutControl.label} shortcut for ${profile.name}`}
                        onclick={() => clearProfileShortcut(profile.id, shortcutControl.slot)}
                      >Clear</button>
                    {/if}
                  </div>
                {/each}
              </div>
              {#if settings.hotkey_profiles.length > 1}
                <button class="profile-remove" aria-label={`Remove ${profile.name}`} onclick={() => removeProfile(profile.id)}>Remove</button>
              {/if}
              {#if profileModels[profile.id]}
                <div class="profile-engine" class:missing={!profileModels[profile.id].downloaded}>
                  <span>
                    {profileModels[profile.id].downloaded
                      ? "Speech engine ready"
                      : `Speech engine required · ${profileModels[profile.id].size_mb} MB`}
                  </span>
                  {#if !profileModels[profile.id].downloaded}
                    <button
                      class="link-btn profile-engine-action"
                      onclick={() => downloadProfileModel(profile)}
                      disabled={downloading !== null}
                    >
                      {downloading === profileModels[profile.id].id
                        ? `Downloading ${Math.round(downloadProgress)}%`
                        : "Download speech engine"}
                    </button>
                  {/if}
                </div>
                {#if profileModelErrors[profile.id]}
                  <div class="hotkey-error">{profileModelErrors[profile.id]}</div>
                {/if}
              {/if}
            </div>
          {/each}
          {#if hotkeyError}
            <div class="hotkey-error">{hotkeyError}</div>
          {:else if !hotkeyStatusOk}
            <div class="hotkey-error">
              ⚠ Not registered{hotkeyStatusError ? `: ${hotkeyStatusError}` : ""}{#if platform === "macos"} — check Accessibility permission or whether another app uses this shortcut.{:else} — this shortcut may already be in use by another app. Try a different combination.{/if}
            </div>
          {/if}
          <div class="hotkey-hint">
            Each shortcut above is independent and either or both may be configured. Use a modifier ({modifierNames().meta}, {modifierNames().ctrl}, {modifierNames().alt}, Shift) + key{#if supportedBareFunctionKeyRange(platform)}, or {supportedBareFunctionKeyRange(platform)} by itself{/if}.{#if platform === "macos"}{" "}Bare F13–F24 requires Accessibility permission: macOS sends keyboard events to Sagascript, which immediately ignores everything except bare F13–F24 and never stores or sends them.{/if}
          </div>
        </div>

        <div class="field-row">
          <span class="field-label">Auto-paste transcription</span>
          <button
            type="button"
            class="toggle"
            class:active={settings.auto_paste}
            onclick={onAutoPasteToggle}
            role="switch"
            aria-checked={settings.auto_paste}
            aria-label="Auto-paste transcription"
            disabled={accessibilityChecking}
          ></button>
        </div>
        <div class="hotkey-hint">Automatically paste dictated text into the active app when transcription finishes.</div>
        {#if platform === "macos" && !accessibilityGranted && (settings.auto_paste || accessibilityRequested)}
          <div class="hotkey-error">Requires Accessibility permission. Auto-paste remains off until approved. <button class="link-btn" onclick={onAutoPasteToggle} disabled={accessibilityChecking}>{accessibilityChecking ? "Checking…" : "Open System Settings"}</button></div>
        {/if}

        <div class="test-section">
          <div class="test-section-label">Try it out</div>
          <button
            class="test-record-btn"
            class:recording={testRecording}
            class:transcribing={testTranscribing}
            onclick={onTestRecord}
            disabled={updatePreparing || dictateButtonAction(backendDictationState, testOwnsRecording) === "blocked" || downloading !== null}
          >
            {#if testTranscribing}
              <div class="spinner small"></div>
              {backendDictationState === "loading_model" ? "Preparing speech engine..." : "Transcribing..."}
            {:else if downloading !== null}
              <div class="spinner small"></div>
              Downloading speech engine...
            {:else if testRecording}
              <div class="recording-dot"></div>
              {testOwnsRecording ? "Stop recording" : "Recording via hotkey..."}
            {:else}
              Start recording
            {/if}
          </button>
          {#if testError}
            <div class="transcribe-error">{testError}</div>
          {/if}
          <textarea
            class="test-result"
            bind:value={testResult}
            oninput={() => { testResultRecoveryPending = true; }}
            disabled={updatePreparing}
            placeholder="Click here and use your hotkey, or press the button above"
          ></textarea>
          {#if testResult.trim()}
            <div class="result-actions">
              <button class="secondary" onclick={() => void copyTestResult()} disabled={updatePreparing}>Copy result</button>
              <button class="secondary" onclick={() => void saveTestResult()} disabled={updatePreparing}>Save result…</button>
              {#if testResultActionMessage}<span role="status">{testResultActionMessage}</span>{/if}
            </div>
          {/if}
        </div>

      {/if}
      <section hidden={activeTab !== "transcribe"} aria-label="File transcription">
        <div class="transcribe-settings-row">
        <button class="active-config-bar" onclick={() => requestTabChange("settings")}>
          <div class="active-config-row">
            <span class="active-config-label">Language</span>
            <span class="active-config-value">{languageLabel(transcribeLanguage())}</span>
          </div>
          <span class="active-config-link">Settings</span>
        </button>
          <div class="field profile-field">
            <label for="transcribe-profile">Profile (optional)</label>
            <select id="transcribe-profile" value={transcribeProfileId ?? ""} onchange={onTranscribeProfileChange} disabled={transcribing}>
              <option value="">No profile (use selected language)</option>
              {#each explicitProfiles() as profile (profile.id)}
                <option value={profile.id}>{profile.name} · {languageLabel(profile.language)}</option>
              {/each}
            </select>
          </div>
        </div>

        <div class="transcribe-options">
          <div class="field profile-field">
            <label for="file-model">File transcription model</label>
            <select id="file-model" value={settings.file_transcription_model}
              onchange={(event) => void onFileModelChange(event)}
              disabled={transcribing || fileModelSaving || downloading !== null}>
              <option value="auto">Auto — current dictation model ({fileAutoModel?.display_name ?? "loading…"})</option>
              {#if settings.file_transcription_model !== "auto" && !fileModelOptions.some((model) => model.id === settings?.file_transcription_model)}
                <option value={settings.file_transcription_model}>Current choice is incompatible with {languageLabel(transcribeLanguage())}</option>
              {/if}
              {#each fileModelOptions as model (model.id)}
                <option value={model.id}>{model.display_name} · {model.size_mb} MB{model.downloaded ? " · ready" : " · download needed"}</option>
              {/each}
            </select>
            <div class="hotkey-hint">Effective model: {settings.file_transcription_model === "auto"
              ? fileAutoModel?.display_name ?? "loading…"
              : fileModelOptions.find((model) => model.id === settings?.file_transcription_model)?.display_name ?? "incompatible with this language"}.
              This choice affects files only; dictation shortcuts keep their own model.</div>
            {#if settings.file_transcription_model !== "auto" && fileModelOptions.some((model) => model.id === settings?.file_transcription_model && !model.downloaded)}
              <button class="secondary" onclick={() => void downloadSelectedFileModel()}
                disabled={downloading !== null || transcribing}>
                {downloading === settings.file_transcription_model ? `Downloading ${downloadingName}… ${downloadProgress}%` : "Download selected model"}
              </button>
            {/if}
            {#if settings.file_transcription_model === "pianissimo-sv"}
              <div class="hotkey-hint">The macOS 13+ app includes Pianissimo's local runtime. Download the corrected 714 MB Q8 model once. Peak memory was about 0.9 GB in our test.</div>
            {/if}
            {#if fileModelError}<div class="transcribe-error" role="alert">{fileModelError}</div>{/if}
          </div>
          {#if selectedTranscribeProfile()}
            <div class="hotkey-hint">This profile fixes the file language and uses its personal dictionary.</div>
          {/if}
          <div class="diarize-row"><label class="diarize-option">
            <input type="checkbox" bind:checked={transcribeDiarize} disabled={transcribing || settings.file_transcription_model === "pianissimo-sv"} />
            Speaker diarization
          </label>
          <button class="info-dot" bind:this={diarizeInfoButton} aria-label="What is speaker diarization?"
            aria-expanded={showDiarizeInfo} onclick={() => { showDiarizeInfo = !showDiarizeInfo; }}>?</button></div>
          {#if showDiarizeInfo}
            <dialog bind:this={diarizeDialog} class="popover-card" aria-label="What is speaker diarization?"
              oncancel={closeDiarizeInfo} onclose={closeDiarizeInfo}>
              <h3>Speaker diarization</h3>
              <p>Detects <strong>who speaks when</strong> and labels each part of the transcript — [Speaker 1], [Speaker 2], …</p>
              <p>Slower than plain transcription, and needs the diarization models (downloaded once).</p>
              <p>Leave it off for a plain transcript.</p>
              <button class="secondary" onclick={closeDiarizeInfo}>Close</button>
            </dialog>
          {/if}
          <textarea class="prompt-input" aria-label="Extra context for this file"
            placeholder="Extra context, e.g. names to listen for: Astrid, Grimnir (optional)"
            bind:value={transcribePrompt} rows="2" disabled={transcribing || settings.file_transcription_model === "pianissimo-sv"}></textarea>
        </div>

        <div
          class="drop-zone"
          class:drag-over={dragOver}
          class:transcribing={transcribing}
        >
          <div class="drop-zone-icon">&#x1F4C1;</div>
          <div class="drop-zone-text">Drop audio or video files here</div>
          <button class="primary open-file-btn" onclick={onPickFile} disabled={updatePreparing}>Open Files...</button>
          {#if recentTranscriptions.length}
            <div class="rerun-controls">
              <button class="secondary rerun-highlight" onclick={retryLastTranscription}
                title="Queue a new run with current model settings and this file's saved profile/context">Re-run</button>
              <select aria-label="Recent files to re-run (last 5, this session)" bind:value={selectedRerunPath} title={selectedRerunPath}>
                {#each recentTranscriptions as run (run.path)}
                  <option value={run.path}>{recentTranscriptions.filter(item => fileJobName(item.path) === fileJobName(run.path)).length > 1 ? run.path : fileJobName(run.path)}</option>
                {/each}
              </select>
            </div>
          {/if}
          <button class="secondary" onclick={() => handleFilesTranscription(["Saved review"], true)} disabled={transcribing || updatePreparing}>
            Open saved meeting...
          </button>
        </div>
        {#if rerunError}<div class="transcribe-error">{rerunError}</div>{/if}

        <div class="formats-hint">
          Supported: {supportedFormats.map(f => f.toUpperCase()).join(", ") || "WAV, MP3, M4A, AAC, MP4, MOV, OGG, WEBM, FLAC"}
        </div>

        {#if fileJobs.length}
          <div class="result-tabs" role="tablist" aria-label="Transcription results">
            {#each fileJobs as job, index (job.id)}
              <button class="result-tab" class:active={selectedFileId === job.id}
                id={`file-tab-${job.id}`} role="tab" aria-selected={selectedFileId === job.id}
                aria-controls={`file-panel-${job.id}`} tabindex={selectedFileId === job.id ? 0 : -1}
                title={job.path} onclick={() => { selectedFileId = job.id; }}
                onkeydown={(event) => onResultTabKeydown(event, index)}>
                <span class="result-filename">{fileJobName(job.path)}</span>
                <span class="result-status">{fileAttention[job.id] ? "needs retry" : job.status}</span>
              </button>
            {/each}
          </div>
        {/if}
        {#each fileJobs as job (job.id)}
          <div id={`file-panel-${job.id}`} role="tabpanel" aria-labelledby={`file-tab-${job.id}`}
            inert={updatePreparing}
            tabindex="0" hidden={selectedFileId !== job.id}>
            <FileTranscription {job} bind:this={fileComponents[job.id]}
              otherBusy={updatePreparing || fileJobs.some(other => other.id !== job.id && other.status === "running")
                || Object.entries(fileBusy).some(([id, busy]) => id !== job.id && busy)}
              openReview={savedReviewIds.includes(job.id)} onComplete={completeFile} onBusyChange={updateFileBusy}
              onMissingFile={forgetMissingFile}
              onAttentionChange={updateFileAttention}
              initialRecoveryFile={fileRecoveryEntries.find((entry) => entry.job_id === job.id) ?? null}
              initialRecoveryMeeting={meetingRecoveryEntries.find((entry) => entry.job_id === job.id) ?? null}
              onFileRecoveryChange={(entry) => onFileRecoveryChange(entry, job.id)}
              onMeetingRecoveryChange={(entry) => onMeetingRecoveryChange(entry, job.id)}
              active={activeTab === "transcribe" && selectedFileId === job.id} />
          </div>
        {/each}
      </section>

      {#if activeTab === "settings"}
        <div class="field">
          <label for="language">Language</label>
          <select id="language" bind:this={languageSelectEl} value={settings.language} onchange={onLanguageChange} disabled={languageSaving || glossarySaving}>
            <option value="en">English</option>
            <option value="sv">Swedish</option>
            <option value="no">Norwegian</option>
            <option value="fi">Finnish</option>
            <option value="auto">Auto-detect</option>
          </select>
          {#if blockedLanguageChange}
            <section class="language-recovery" aria-labelledby="language-recovery-title">
              <h3 id="language-recovery-title" bind:this={languageRecoveryTitle} tabindex="-1">Switch to {languageLabel(blockedLanguageChange.language)}?</h3>
              <p>The <strong>default</strong> profile has the saved dictionary below. Its words belong to its current language, even when another dictionary is selected in the editor.</p>
              <textarea class="initial-prompt-input" rows="3" aria-label="Saved default profile dictionary" value={blockedLanguageChange.source} readonly></textarea>
              <p>Copy these words before continuing if you want to keep them. Clearing cannot be undone. Unsaved editor drafts will be preserved.</p>
              <p>To keep this dictionary, cancel and use or add a language profile in the Dictate tab.</p>
              <div class="dictionary-actions">
                <button type="button" class="secondary" onclick={cancelLanguageChange} disabled={languageSaving}>Cancel</button>
                <button type="button" class="danger" onclick={() => void clearDictionaryAndSwitchLanguage()} disabled={languageSaving || glossarySaving}>
                  {languageSaving ? "Switching…" : `Clear default dictionary and switch to ${languageLabel(blockedLanguageChange.language)}`}
                </button>
              </div>
            </section>
          {/if}
        </div>

        <div class="field-row">
          <span class="field-label">Show recording overlay</span>
          <button
            type="button"
            class="toggle"
            class:active={settings.show_overlay}
            onclick={onShowOverlayToggle}
            role="switch"
            aria-checked={settings.show_overlay}
            aria-label="Show recording overlay"
          ></button>
        </div>

        <div class="field">
          <div class="dictionary-heading">
            <label for="initial-prompt">Personal dictionary</label>
            {#if glossaryHasUnsavedChanges()}
              <span class="unsaved-indicator" role="status">Unsaved changes</span>
            {/if}
          </div>
          <select id="dictionary-scope" value={glossaryScopeId} onchange={onGlossaryScopeChange} disabled={glossarySaving || languageSaving}>
            <option value="">Global hints</option>
            {#each explicitProfiles() as profile (profile.id)}
              <option value={profile.id}>{profile.name} · {languageLabel(profile.language)}</option>
            {/each}
          </select>
          <textarea
            id="initial-prompt"
            class="initial-prompt-input"
            rows="5"
            value={glossaryDraft}
            oninput={onGlossaryInput}
            disabled={glossarySaving || languageSaving}
            placeholder="OpenRouter = open router | open vrouter&#10;merge = merch&#10;Cloudflare = cloud flare"
          ></textarea>
          <div class="dictionary-actions">
            <button
              type="button"
              class="primary"
              onclick={() => void saveGlossary()}
              disabled={!glossaryHasUnsavedChanges() || glossarySaving || languageSaving}
            >{glossarySaving ? "Saving…" : "Save changes"}</button>
            <button
              type="button"
              class="secondary"
              onclick={discardGlossaryChanges}
              disabled={!glossaryHasUnsavedChanges() || glossarySaving || languageSaving}
            >Discard changes</button>
          </div>
          {#if glossaryScopeId === ""}
            <div class="hotkey-hint glossary-migration">
              Global entries are hint-only and remain stored. To enable deterministic alias replacements, copy an entry into the explicit-language profile that should use it.
            </div>
          {:else}
            <div class="hotkey-hint glossary-migration">
              This explicit-language profile supplies deterministic aliases for its language. Save changes explicitly; switching scope never moves entries to another dictionary.
            </div>
          {/if}
          {#if glossaryConflictScopeId === glossaryScopeId && settingsError.startsWith(dictionaryConflictPrefix)}
            <div class="hotkey-hint glossary-migration">
              This dictionary changed elsewhere. Your draft is preserved; copy it if needed, then switch scopes and reselect this scope to reload the saved value. If it still shows the old text, close and reopen Settings.
            </div>
          {/if}
          {#if recoveredGlossaryDrafts.length > 0}
            <div class="hotkey-hint glossary-migration">
              Unsaved drafts are preserved below for manual recovery. They are never saved or copied automatically into another dictionary.
            </div>
            {#each recoveredGlossaryDrafts as recovery (recovery.scopeId + "\u0000" + recovery.draft)}
              <div class="glossary-recovery">
                <div class="hotkey-hint">
                  <strong>Unsaved draft</strong> for <code>{glossaryScopeLabel(recovery.scopeId)}</code>
                  {#if recovery.conflicted} — the saved dictionary changed elsewhere.{/if}
                </div>
                <textarea
                  class="initial-prompt-input"
                  rows="3"
                  aria-label={`Unsaved draft for ${glossaryScopeLabel(recovery.scopeId)}`}
                  value={recovery.draft}
                  readonly
                ></textarea>
              </div>
            {/each}
          {/if}
          <div class="hotkey-hint">
            One preferred spelling per line. Add exact mishearings after <code>=</code>, separated by <code>|</code>.
            Plain terms still guide Whisper. Save explicitly to use changes for live dictation and batch jobs.
          </div>
        </div>

        <details class="advanced-section">
          <summary>Advanced</summary>
          <div class="advanced-content">
            <p class="advanced-intro">
              Sagascript automatically chooses the recommended local model for each language.
              Change these controls only when you have a specific quality or performance need.
            </p>

            <div class="model-section-label">
              Manual model choice · {languageLabel(settings.language)}
            </div>

            <div class="model-picker">
              {#each models as model}
                <button
                  class="model-card"
                  class:active={model.active}
                  class:downloading={downloading === model.id}
                  onclick={() => selectModel(model)}
                  disabled={downloading !== null || selecting}
                >
                  <div class="model-card-header">
                    <span class="model-card-name">{model.display_name}</span>
                    {#if model.active}
                      <span class="model-badge active-badge">Active</span>
                    {:else if model.downloaded}
                      <span class="model-badge ready-badge">Ready</span>
                    {:else}
                      <span class="model-badge download-badge">Download · {model.size_mb} MB</span>
                    {/if}
                  </div>
                  <div class="model-card-desc">{model.description}</div>
                  {#if downloading === model.id}
                    <div class="progress-bar">
                      <div class="progress-fill" style="width: {downloadProgress}%"></div>
                    </div>
                  {/if}
                </button>
              {/each}
            </div>

            {#if modelError}
              <div class="transcribe-error">{modelError}</div>
            {/if}

            <div class="model-hint">
              Larger models are more accurate but take longer to transcribe.
              {#if models.some(m => !m.downloaded && !m.active)}
                Models are downloaded once and stored locally.
              {/if}
            </div>

            <div class="field advanced-field">
              <label for="beam-size">Decoding mode</label>
              <select id="beam-size" value={settings.beam_size} onchange={onBeamSizeChange}>
                <option value={0}>Greedy (fast)</option>
                <option value={5}>Beam search (accurate)</option>
              </select>
            </div>

            <div class="field-row">
              <span class="field-label">Temperature fallback</span>
              <button
                type="button"
                class="toggle"
                class:active={settings.temperature_fallback}
                onclick={onTemperatureFallbackToggle}
                role="switch"
                aria-checked={settings.temperature_fallback}
                aria-label="Temperature fallback"
              ></button>
            </div>
            <div class="hotkey-hint">Re-decode hard segments; off is faster but less robust.</div>

            <div class="field-row advanced-toggle">
              <span class="field-label">Voice activity detection</span>
              <button
                type="button"
                class="toggle"
                class:active={settings.vad_enabled}
                onclick={onVadToggle}
                role="switch"
                aria-checked={settings.vad_enabled}
                aria-label="Voice activity detection"
              ></button>
            </div>
            <div class="hotkey-hint">Skip silence; downloads a small model on first enable.</div>
          </div>
        </details>

      {/if}
    </div>
  {:else}
    <div class="loading">
      {#if initError}
        <div class="transcribe-error">{initError}</div>
      {:else}
        Loading settings...
      {/if}
    </div>
  {/if}

  {#if downloading}
    <div class="download-status-bar">
      <div class="download-status-info">
        <span class="download-status-label">Downloading {downloadingName}...</span>
        <span class="download-status-pct">{Math.round(downloadProgress)}%</span>
      </div>
      <div class="download-status-track">
        <div class="download-status-fill" style="width: {downloadProgress}%"></div>
      </div>
    </div>
  {/if}

  {#if pendingGlossaryNavigation}
    <div class="dialog-backdrop">
      <div
        class="unsaved-dialog"
        bind:this={glossaryDialogEl}
        role="dialog"
        aria-modal="true"
        aria-labelledby="unsaved-dialog-title"
        aria-describedby="unsaved-dialog-description"
        tabindex="-1"
        onkeydown={onGlossaryDialogKeydown}
      >
        <h2 id="unsaved-dialog-title">Unsaved dictionary changes</h2>
        <p id="unsaved-dialog-description">
          {pendingGlossaryNavigation.kind === "scope"
            ? `Save changes before switching to ${glossaryScopeLabel(pendingGlossaryNavigation.scopeId)}?`
            : "Save changes before leaving Settings?"}
        </p>
        {#if settingsError}
          <div class="hotkey-error" role="alert">{settingsError}</div>
        {/if}
        <div class="dialog-actions">
          <button
            type="button"
            class="primary"
            onclick={() => void saveAndFinishGlossaryNavigation()}
            disabled={glossarySaving || languageSaving}
          >{glossarySaving ? "Saving…" : "Save"}</button>
          <button
            type="button"
            class="danger"
            onclick={discardAndFinishGlossaryNavigation}
            disabled={glossarySaving || languageSaving}
          >Discard</button>
          <button type="button" class="secondary" data-dialog-stay onclick={stayOnGlossaryDraft} disabled={glossarySaving}>Stay</button>
        </div>
      </div>
    </div>
  {/if}
</div>

<style>
  .settings-window {
    display: flex;
    flex-direction: column;
    height: 100vh;
    min-width: 0;
  }

  .tabs {
    display: flex;
    padding: 12px 20px 0;
    gap: 4px;
    border-bottom: 1px solid var(--border);
  }

  .window-header {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 16px;
    flex-wrap: wrap;
    padding: 16px 20px 12px;
    border-bottom: 1px solid var(--border);
    min-width: 0;
  }

  .window-title {
    min-width: 0;
    font-size: 18px;
    line-height: 1.2;
    color: var(--text);
  }

  .build-info {
    min-width: 0;
    flex: 1 1 auto;
    max-width: 100%;
    overflow-wrap: anywhere;
    color: var(--text-muted);
    font-size: 11px;
    font-variant-numeric: tabular-nums;
    text-align: right;
  }

  .tab {
    padding: 8px 16px;
    background: transparent;
    color: var(--text-muted);
    border-radius: 6px 6px 0 0;
    font-size: 13px;
    border-bottom: 2px solid transparent;
    margin-bottom: -1px;
  }

  .tab.active {
    color: var(--accent);
    border-bottom-color: var(--accent);
  }

  .tab:hover:not(.active) {
    color: var(--text);
  }

  .content {
    padding: 20px;
    flex: 1;
    overflow-y: auto;
    min-width: 0;
    max-width: 100%;
  }

  .window-header > *,
  .tabs > *,
  .content > * {
    min-width: 0;
    max-width: 100%;
  }

  .active-config-bar {
    display: flex;
    align-items: center;
    gap: 16px;
    width: 100%;
    padding: 8px 12px;
    margin-bottom: 16px;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    cursor: pointer;
    transition: border-color 0.15s;
    min-width: 0;
  }

  .active-config-bar:hover {
    border-color: var(--text-muted);
  }

  .active-config-row {
    display: flex;
    flex-direction: column;
    gap: 1px;
    text-align: left;
    min-width: 0;
  }

  .active-config-label {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--text-muted);
    font-weight: 600;
  }

  .active-config-value {
    font-size: 13px;
    color: var(--text);
    overflow-wrap: anywhere;
  }

  .active-config-link {
    font-size: 11px;
    color: var(--accent);
    font-weight: 500;
    margin-left: auto;
    flex-shrink: 0;
  }

  .hotkey-recorder {
    background: var(--bg-secondary);
    border: 2px solid var(--border);
    border-radius: var(--radius);
    padding: 8px 12px;
    font-family: monospace;
    font-size: 13px;
    color: var(--accent);
    cursor: pointer;
    text-align: left;
    width: 100%;
    transition: border-color 0.15s;
  }

  .hotkey-recorder:hover {
    border-color: var(--text-muted);
  }

  .hotkey-recorder.recording {
    border-color: var(--accent);
    animation: pulse-border 1.2s ease-in-out infinite;
    color: var(--text-muted);
  }

  .profile-heading,
  .profile-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .profile-heading {
    justify-content: space-between;
    margin-bottom: 8px;
  }

  .profile-card {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 10px;
    margin-bottom: 8px;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }

  .profile-name {
    min-width: 0;
    width: auto;
    flex: 1 1 52%;
    font-weight: 600;
  }

  .profile-row > select {
    min-width: 0;
    width: auto;
    flex: 1 1 48%;
  }

  .shortcut-grid {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 8px;
    align-items: end;
  }

  .shortcut-control {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .shortcut-label {
    color: var(--text-muted);
    font-size: 11px;
    font-weight: 600;
  }

  .shortcut-helper {
    color: var(--text-muted);
    font-size: 11px;
    line-height: 1.35;
  }

  .shortcut-clear {
    align-self: flex-start;
    background: transparent;
    border: none;
    cursor: pointer;
    color: var(--text-muted);
    font-size: 11px;
    padding: 0;
  }

  .shortcut-clear:hover {
    color: var(--text);
  }

  @media (max-width: 520px) {
    .shortcut-grid {
      grid-template-columns: 1fr;
    }
  }

  .profile-engine {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding-top: 3px;
    color: var(--text-muted);
    font-size: 11px;
  }

  .profile-engine.missing {
    color: var(--danger);
  }

  .profile-engine .link-btn:disabled {
    cursor: wait;
    opacity: 0.7;
  }

  .profile-engine-action {
    display: inline-block;
    flex: 0 0 152px;
    width: 152px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    text-align: right;
  }

  .profile-remove {
    border: none;
    background: none;
    color: var(--danger);
    cursor: pointer;
    font-size: 11px;
  }

  @keyframes pulse-border {
    0%, 100% { border-color: var(--accent); }
    50% { border-color: var(--border); }
  }

  .hotkey-error {
    margin-top: 4px;
    font-size: 11px;
    color: var(--danger);
  }

  .link-btn {
    background: none;
    border: none;
    color: var(--accent);
    font-size: inherit;
    padding: 0;
    cursor: pointer;
    text-decoration: underline;
  }

  .hotkey-hint {
    margin-top: 4px;
    font-size: 11px;
    color: var(--text-secondary, #888);
  }

  /* Model picker */

  .advanced-section {
    margin: 22px 0 18px;
    border-top: 1px solid var(--border);
    border-bottom: 1px solid var(--border);
  }

  .advanced-section summary {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 13px 0;
    color: var(--text);
    font-weight: 600;
    cursor: pointer;
    list-style: none;
  }

  .advanced-section summary::-webkit-details-marker {
    display: none;
  }

  .advanced-section summary::after {
    content: "+";
    color: var(--text-muted);
    font-size: 18px;
    font-weight: 400;
  }

  .advanced-section[open] summary::after {
    content: "−";
  }

  .advanced-content {
    padding: 2px 0 18px;
  }

  .advanced-intro {
    margin-bottom: 16px;
    color: var(--text-muted);
    font-size: 12px;
    line-height: 1.55;
  }

  .advanced-field {
    margin-top: 20px;
  }

  .advanced-toggle {
    margin-top: 16px;
  }

  .model-section-label {
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--text-muted);
    margin-bottom: 10px;
    font-weight: 600;
  }

  .model-picker {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .model-card {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 14px 16px;
    background: var(--bg-secondary);
    border: 2px solid var(--border);
    border-radius: 10px;
    cursor: pointer;
    text-align: left;
    transition: border-color 0.15s, background 0.15s;
    width: 100%;
  }

  .model-card:hover:not(:disabled) {
    border-color: var(--text-muted);
  }

  .model-card.active {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 8%, var(--bg-secondary));
  }

  .model-card:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .model-card-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .model-card-name {
    font-size: 15px;
    font-weight: 600;
    color: var(--text);
  }

  .model-card-desc {
    font-size: 12px;
    color: var(--text-muted);
  }

  .model-badge {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 10px;
    font-weight: 500;
  }

  .active-badge {
    background: color-mix(in srgb, var(--accent) 20%, transparent);
    color: var(--accent);
  }

  .ready-badge {
    background: color-mix(in srgb, var(--success, #34c759) 15%, transparent);
    color: var(--success, #34c759);
  }

  .download-badge {
    background: var(--bg);
    color: var(--text-muted);
    border: 1px solid var(--border);
  }

  .progress-bar {
    height: 4px;
    background: var(--border);
    border-radius: 2px;
    margin-top: 6px;
    overflow: hidden;
  }

  .progress-fill {
    height: 100%;
    background: var(--accent);
    border-radius: 2px;
    transition: width 0.2s;
  }

  .model-hint {
    font-size: 12px;
    color: var(--text-muted);
    line-height: 1.5;
    margin-top: 12px;
  }

  /* Download status bar (bottom of window, visible on all tabs) */

  .download-status-bar {
    padding: 10px 20px 14px;
    border-top: 1px solid var(--border);
    background: var(--bg-secondary);
    flex-shrink: 0;
  }

  .download-status-info {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 6px;
  }

  .download-status-label {
    font-size: 12px;
    font-weight: 500;
    color: var(--text);
  }

  .download-status-pct {
    font-size: 12px;
    font-weight: 600;
    color: var(--accent);
    font-variant-numeric: tabular-nums;
  }

  .download-status-track {
    height: 6px;
    background: var(--border);
    border-radius: 3px;
    overflow: hidden;
  }

  .download-status-fill {
    height: 100%;
    background: var(--accent);
    border-radius: 3px;
    transition: width 0.2s;
  }

  .initial-prompt-input {
    width: 100%;
    padding: 8px 10px;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    color: var(--text);
    font-family: inherit;
    font-size: 13px;
    line-height: 1.5;
    resize: vertical;
    outline: none;
    user-select: text;
    -webkit-user-select: text;
  }

  .initial-prompt-input:focus {
    border-color: var(--accent);
  }

  .dictionary-heading {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 8px;
  }

  .unsaved-indicator {
    color: var(--accent);
    font-size: 11px;
    font-weight: 600;
  }

  .dictionary-actions,
  .dialog-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    margin-top: 8px;
  }

  .dictionary-actions button:disabled,
  .dialog-actions button:disabled {
    cursor: default;
    opacity: 0.6;
  }

  .dialog-backdrop {
    position: fixed;
    inset: 0;
    z-index: 20;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 20px;
    background: color-mix(in srgb, var(--bg) 78%, transparent);
  }

  .unsaved-dialog {
    width: min(100%, 420px);
    padding: 20px;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    box-shadow: 0 12px 36px rgba(0, 0, 0, 0.35);
  }

  .unsaved-dialog h2 {
    margin-bottom: 8px;
    color: var(--text);
    font-size: 15px;
  }

  .unsaved-dialog p {
    color: var(--text-muted);
    font-size: 13px;
  }

  .loading {
    padding: 40px 20px;
    text-align: center;
    color: var(--text-muted);
  }

  /* Transcribe tab */
  .transcribe-settings-row { display: flex; gap: 8px; align-items: flex-start; margin-bottom: 8px; }
  .transcribe-settings-row .active-config-bar { flex: 1 1 0; width: auto; margin-bottom: 0; min-width: 0; }
  .transcribe-settings-row .profile-field { flex: 1.2 1 0; margin-bottom: 0; min-width: 0; }
  .rerun-controls { display: flex; align-items: center; gap: 8px; width: 100%; min-width: 0; }
  .rerun-controls button { flex: 0 0 auto; }
  .rerun-controls select { flex: 1; min-width: 0; width: 0; }
  button.secondary.rerun-highlight { border-color: #eab308; color: #eab308; }
  button.secondary.rerun-highlight:hover:not(:disabled) { background: color-mix(in srgb, #eab308 12%, transparent); }
  .diarize-row { display: flex; align-items: center; gap: 6px; }
  .info-dot { width: 18px; height: 18px; padding: 0; border: 1px solid var(--text-muted); border-radius: 50%; background: transparent; color: var(--text-muted); }
  .popover-card { margin: auto; width: 280px; max-width: calc(100vw - 48px); padding: 14px 16px; background: var(--bg-secondary); border: 1px solid var(--border); border-radius: 12px; color: var(--text); font-size: 12px; line-height: 1.55; }
  .popover-card::backdrop { background: rgba(0, 0, 0, 0.45); }
  .popover-card p { margin: 8px 0; }

  .drop-zone {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    padding: 12px 16px;
    border: 2px dashed var(--border);
    border-radius: 12px;
    text-align: center;
    transition: border-color 0.2s, background 0.2s;
  }

  .drop-zone.drag-over {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 6%, var(--bg));
  }

  .drop-zone.transcribing {
    border-style: solid;
    border-color: var(--accent);
  }


  .drop-zone-icon {
    font-size: 28px;
    line-height: 1;
  }

  .drop-zone-text {
    font-size: 13px;
    color: var(--text-muted);
  }

  .open-file-btn {
    margin-top: 4px;
  }

  .spinner {
    width: 28px;
    height: 28px;
    border: 3px solid var(--border);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin {
    to { transform: rotate(360deg); }
  }

  .formats-hint {
    font-size: 11px;
    color: var(--text-muted);
    margin-top: 10px;
    text-align: center;
  }

  .transcribe-options {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin-top: 0;
    margin-bottom: 8px;
  }

  .diarize-option {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
    color: var(--text);
    cursor: pointer;
    user-select: none;
  }

  .diarize-option input[type="checkbox"] {
    width: 15px;
    height: 15px;
    accent-color: var(--accent);
    cursor: pointer;
    flex-shrink: 0;
  }

  .prompt-input {
    width: 100%;
    padding: 8px 10px;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    color: var(--text);
    font-family: inherit;
    font-size: 12px;
    line-height: 1.5;
    resize: vertical;
    outline: none;
    box-sizing: border-box;
  }

  .prompt-input::placeholder {
    color: var(--text-muted);
  }

  .prompt-input:focus {
    border-color: var(--accent);
  }

  .transcribe-error {
    margin-top: 12px;
    padding: 10px 14px;
    background: color-mix(in srgb, var(--danger) 12%, var(--bg));
    border: 1px solid var(--danger);
    border-radius: var(--radius);
    color: var(--danger);
    font-size: 12px;
  }

  .recovery-notice {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 12px;
    padding: 10px 12px;
    border: 1px solid var(--accent);
    border-radius: var(--radius);
    font-size: 13px;
  }

  .recovery-notice span { flex: 1; color: var(--text-muted); }
  .recovery-notice button { flex-shrink: 0; }

  .queue-summary {
    font-size: 12px;
    color: var(--text-muted);
    margin-bottom: 12px;
    line-height: 1.6;
  }

  .queue-summary.queue-empty { margin: 0; }

  .queue-attention {
    border: 1px solid var(--accent);
    border-radius: var(--radius);
    padding: 10px 12px;
    margin-bottom: 12px;
    font-size: 13px;
  }

  .queue-attention > div { display: flex; align-items: center; gap: 12px; justify-content: space-between; }
  .queue-attention button { flex-shrink: 0; }

  .result-tabs {
    display: flex;
    gap: 6px;
    overflow-x: auto;
    margin: 18px 0 12px;
    padding-bottom: 4px;
  }
  .language-recovery {
    margin-top: 12px;
    padding: 14px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }

  .language-recovery h3 {
    margin: 0 0 8px;
    font-size: 14px;
  }

  .language-recovery p {
    margin: 8px 0;
    font-size: 12px;
    line-height: 1.5;
  }

  .result-tab {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 4px;
    min-width: 110px;
    max-width: 210px;
    padding: 10px 12px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg-secondary);
    color: var(--text);
    cursor: pointer;
  }
  .result-tab.active {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 10%, var(--bg));
  }
  .result-filename {
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .result-status {
    font-size: 11px;
    color: var(--text-muted);
    text-transform: capitalize;
  }
  [hidden] {
    display: none !important;
  }

  /* Test dictation section */

  .test-section {
    margin-top: 20px;
    padding-top: 16px;
    border-top: 1px solid var(--border);
  }

  .test-section-label {
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--text-muted);
    margin-bottom: 10px;
    font-weight: 600;
  }

  .test-record-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    width: 100%;
    padding: 10px 16px;
    background: var(--bg-secondary);
    border: 2px solid var(--border);
    border-radius: var(--radius);
    color: var(--text);
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    transition: border-color 0.15s, background 0.15s;
  }

  .test-record-btn:hover:not(:disabled) {
    border-color: var(--text-muted);
  }

  .test-record-btn.recording {
    border-color: var(--danger);
    background: color-mix(in srgb, var(--danger) 8%, var(--bg-secondary));
    color: var(--danger);
  }

  .test-record-btn:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .recording-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--danger);
    animation: pulse-dot 1s ease-in-out infinite;
  }

  @keyframes pulse-dot {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.4; }
  }

  .spinner.small {
    width: 14px;
    height: 14px;
    border-width: 2px;
  }

  .test-result {
    width: 100%;
    min-height: 60px;
    max-height: 120px;
    margin-top: 10px;
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

  .test-result:focus {
    border-color: var(--accent);
  }

  .result-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 8px;
    font-size: 12px;
  }
</style>
