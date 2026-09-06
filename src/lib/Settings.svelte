<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import {
    getSettings,
    getLastError,
    getLastTranscription,
    setLanguage,
    setHotkeyMode,
    setPresenterConfig,
    setHotkeyProfiles,
    setAutoPaste,
    setInitialPrompt,
    setProfileGlossary,
    setShowOverlay,
    setWhisperModel,
    setBeamSize,
    setTemperatureFallback,
    setVadEnabled,
    getBuildInfo,
    getModelInfo,
    getEffectiveModelInfo,
    downloadModel,
    transcribeFile,
    beginMeetingFile,
    getMeetingJob,
    cancelMeetingJob,
    renameMeetingSpeaker,
    mergeMeetingSpeakers,
    saveMeetingExport,
    getSupportedFormats,
    getPlatform,
    checkAccessibilityPermission,
    requestAccessibilityPermission,
    retryHotkeyRegistration,
    startRecording,
    stopAndTranscribe,
    hotkeyStatus,
    type Settings,
    type BuildInfo,
    type Language,
    type HotkeyMode,
    type WhisperModel,
    type HotkeyStatus,
    type HotkeyProfile,
    type MeetingJobStatus,
    type MeetingJobSnapshot,
    type PresenterConfig,
  } from "./api";
  import MeetingReview from "./MeetingReview.svelte";
  import type { MeetingExportFormat, MeetingTranscript } from "./meeting-types";
  import { pollMeetingJob as pollMeetingJobClient } from "./meeting-job-client";
  import PresenterSettings from "./PresenterSettings.svelte";
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
    supportedBareFunctionKeyRange,
    tauriKeyName,
  } from "./hotkey.js";

  let settings: Settings | null = $state(null);
  let buildInfo: BuildInfo | null = $state(null);
  let models: WhisperModel[] = $state([]);
  let activeTab: "dictate" | "transcribe" | "settings" = $state("dictate");
  let downloading: string | null = $state(null);
  let downloadingName: string = $state("");
  let downloadProgress: number = $state(0);
  let profileModels: Record<string, WhisperModel> = $state({});
  let profileModelErrors: Record<string, string> = $state({});
  let profileModelRefresh = 0;

  let platform: string = $state("macos");

  // Initial data-fetch + settings-mutation error states
  let initError: string = $state("");
  let settingsError: string = $state("");

  // Model selection state
  let selecting: boolean = $state(false);
  let modelError: string = $state("");

  let accessibilityGranted: boolean = $state(true); // assume true; checked on mount for macOS
  let accessibilityChecking: boolean = $state(false);
  let accessibilityRequested: boolean = $state(false);

  // Hotkey recorder state
  let recordingProfileId: string | null = $state(null);
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
    if (recordingProfileId && hotkeyRecorderEl) hotkeyRecorderEl.focus();
  });

  // Dictate test state
  let testRecording: boolean = $state(false);
  let testTranscribing: boolean = $state(false);
  let backendDictationState: BackendDictationState = $state("idle");
  let testOwnsRecording: boolean = $state(false);
  let testResult: string = $state("");
  let testError: string = $state("");

  type PresenterStatus =
    | "listening"
    | "transcribing"
    | "verifying_insertion"
    | "inserted"
    | "submitting"
    | "sent"
    | "cancelled"
    | "draft"
    | "failed"
    | "no_speech"
    | "submit_uncertain";

  const presenterStatusLabels: Record<PresenterStatus, string> = {
    listening: "Presenter listening",
    transcribing: "Presenter transcribing",
    verifying_insertion: "Verifying insertion…",
    inserted: "Recognized text inserted",
    submitting: "Sending submit key…",
    sent: "Submit key sent; delivery not confirmed",
    cancelled: "Presenter cancelled",
    draft: "Not sent — copy recognized text from Dictate",
    failed: "Presenter failed",
    no_speech: "No speech detected",
    submit_uncertain: "Submit may have been sent — check destination before retrying",
  };

  let presenterStatus: PresenterStatus | null = $state(null);

  function isPresenterStatus(value: string): value is PresenterStatus {
    return Object.prototype.hasOwnProperty.call(presenterStatusLabels, value);
  }

  onMount(() => {
    let disposed = false;
    let revision = 0;
    const stops: Array<() => void> = [];
    const remember = (stop: () => void) => disposed ? stop() : stops.push(stop);
    const errorListener = listen<string>("error", (event) => {
      revision++;
      testError = event.payload;
      activeTab = "dictate";
    }).then(remember);
    const resultListener = listen<string>("transcription-result", (event) => {
      revision++;
      testResult = event.payload;
      testError = "";
    }).then(remember);
    const stateListener = listen<string>("state-changed", (event) => {
      if (event.payload === "recording") {
        revision++;
        testError = "";
      }
    }).then(remember);
    const presenterStatusListener = listen("presenter-status", (event) => {
      if (typeof event.payload !== "string" || !isPresenterStatus(event.payload)) return;
      presenterStatus = event.payload;
    }).then(remember);
    // A failed background dictation may create this window after its event.
    // Recover the persisted-in-memory result without racing newer events.
    Promise.all([errorListener, resultListener, stateListener, presenterStatusListener]).then(async () => {
      const initialRevision = revision;
      const [error, text] = await Promise.all([getLastError(), getLastTranscription()]);
      if (!disposed && revision === initialRevision) {
        testError = error ?? "";
        testResult = text ?? "";
      }
    }).catch((error) => {
      console.warn("Could not restore the last dictation result", error);
    });
    return () => { disposed = true; stops.forEach((stop) => stop()); };
  });

  // Transcribe tab state
  let supportedFormats: string[] = $state([]);
  let transcribing: boolean = $state(false);
  let transcriptionProgress: number = $state(0);
  let transcriptionResult: string = $state("");
  let transcribeError: string = $state("");
  let dragOver: boolean = $state(false);
  let transcribePrompt: string = $state('');
  let transcribeDiarize: boolean = $state(false);
  let transcribeProfileId: string | null = $state(null);
  let meetingTranscript: MeetingTranscript | null = $state(null);
  let meetingJobId: string | null = $state(null);
  let meetingJobStatus: MeetingJobStatus | null = $state(null);
  let meetingPhase: string = $state("");
  let meetingError: string = $state("");
  let meetingPollingFailed: boolean = $state(false);
  let meetingPollGeneration = 0;
  let meetingPollActive = false;
  let meetingDocumentRevision = $state(0);
  let meetingActionQueue: Promise<void> = Promise.resolve();

  onDestroy(() => {
    meetingPollGeneration += 1;
  });

  // The global dictionary is retained as a decoder hint source. Explicit
  // language profiles are the only selectable sources for deterministic
  // glossary replacements.
  // Empty string is the UI-only global sentinel; profile IDs may legally be
  // "global", so that name cannot identify the global scope.
  let glossaryScopeId: string = $state("");
  let glossaryDraft: string = $state("");
  let glossaryDraftInitialized = false;
  let glossaryScopeGeneration = 0;
  let glossaryDraftGeneration = 0;
  let lastStoredGlossarySources: Record<string, string> = {};
  let glossaryEditBaseline: { scopeId: string; source: string; generation: number } | null = null;
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
    // Register listeners + drag-drop FIRST — they don't depend on the data
    // fetched below, so a rejected invoke in the fetch sequence must never
    // prevent them from wiring up (e.g. a stuck-at-0% download).
    listen("model-download-progress", (event: any) => {
      downloadProgress = event.payload.progress;
    });

    listen("transcription-progress", (event: any) => {
      transcriptionProgress = event.payload;
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
        activeTab = t;
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
        if (paths.length > 0) {
          activeTab = "transcribe";
          handleFileTranscription(paths[0]);
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
          activeTab = tab;
        }
      } catch (e: any) {
        initError = typeof e === "string" ? e : e?.message || "Failed to load settings.";
      }
    })();
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
    const value = (e.target as HTMLSelectElement).value as Language;
    const ok = await applySetting(() => setLanguage(value));
    if (ok) {
      models = await getModelInfo();
    }
  }

  async function onHotkeyModeChange(e: Event) {
    const value = (e.target as HTMLSelectElement).value as HotkeyMode;
    await applySetting(() => setHotkeyMode(value));
  }

  async function onPresenterSave(config: PresenterConfig): Promise<string | null> {
    const ok = await applySetting(() => setPresenterConfig(config));
    return ok ? null : settingsError || "Failed to save presenter settings.";
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
      ...settings.hotkey_profiles.map((profile) => profile.shortcut),
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

  async function onInitialPromptBlur(e: Event) {
    const request: GlossarySaveRequest = {
      scopeId: glossaryScopeId,
      generation: glossaryScopeGeneration,
      draftGeneration: glossaryDraftGeneration,
      value: (e.target as HTMLTextAreaElement).value,
    };
    const scopeId = request.scopeId;
    const draftGeneration = request.draftGeneration;
    const value = (e.target as HTMLTextAreaElement).value;
    const editBaseline = glossaryEditBaseline;
    const expectedSource = editBaseline?.scopeId === scopeId
      && editBaseline.generation <= draftGeneration
      ? editBaseline.source
      : lastStoredGlossarySources[scopeId] ?? glossarySourceForScope(scopeId);
    glossaryDraft = value;
    if (!settings || !isValidGlossaryScope(scopeId)) return;
    if (!editBaseline && value === (lastStoredGlossarySources[scopeId] ?? glossarySourceForScope(scopeId))) return;

    const saveError = { value: "" };
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
    // same edit lineage, advance only that lineage's baseline to our value.
    // A reselected scope has a different baseline object and is never
    // silently advanced from a fresh settings read.
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

    // A selector change while the invoke was pending owns the textarea now;
    // never overwrite its newer scope with this request's result.
    if (!requestIsCurrent) return;
    if (saved) {
      if (glossaryEditBaseline === editBaseline) glossaryEditBaseline = null;
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

  function onGlossaryScopeChange(e: Event) {
    const nextScope = (e.target as HTMLSelectElement).value;
    if (!settings || !isValidGlossaryScope(nextScope)) return;
    const previousScope = glossaryScopeId;
    const previousConflict = glossaryConflictScopeId === previousScope
      && settingsError.startsWith(dictionaryConflictPrefix);
    if (
      glossaryEditBaseline?.scopeId === previousScope
      || previousConflict
    ) {
      rememberGlossaryRecovery(previousScope, glossaryDraft, previousConflict);
    }
    glossaryScopeGeneration += 1;
    glossaryDraftGeneration += 1;
    glossaryEditBaseline = null;
    if (previousConflict) {
      settingsError = "";
      glossaryConflictScopeId = null;
    }
    glossaryScopeId = nextScope;
    glossaryDraft = glossarySourceForScope(nextScope, settings);
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

  function enqueueMeetingAction(action: (transcript: MeetingTranscript, revision: number) => Promise<void>): Promise<void> {
    const queued = meetingActionQueue.then(async () => {
      const transcript = meetingTranscript;
      if (!transcript) return;
      await action(transcript, meetingDocumentRevision);
    });
    meetingActionQueue = queued.catch(() => undefined);
    return queued;
  }

  async function pollMeetingJob(jobId: string, generation: number): Promise<void> {
    if (meetingPollActive) return;
    meetingPollActive = true;
    try {
      await pollMeetingJobClient({
        jobId,
        get: getMeetingJob,
        isCurrent: () => generation === meetingPollGeneration,
        onFailure: () => {
          meetingPollingFailed = true;
          meetingError = "Could not check meeting progress. Retry the status check to continue.";
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
            if (snapshot.status === "completed" && snapshot.transcript) {
              meetingTranscript = snapshot.transcript;
              meetingDocumentRevision += 1;
              meetingError = "";
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
    const generation = ++meetingPollGeneration;
    ++meetingDocumentRevision;
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
    const profileId = selectedTranscribeProfile()?.id ?? null;
    const prompt = transcribePrompt.trim() || null;
    if (transcribeDiarize) {
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
    try {
      await waitForMeetingActions();
      transcriptionResult = await transcribeFile(filePath, {
        prompt: prompt ?? undefined,
        diarize: false,
        profileId: profileId ?? undefined,
      });
    } catch (error: any) {
      transcribeError = typeof error === "string" ? error : error.message || "Transcription failed";
    } finally {
      transcribing = false;
      transcriptionProgress = 0;
    }
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
        meetingError = "Cancellation was not accepted. The meeting is still running; try again.";
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
    await enqueueMeetingAction(async (transcript, revision) => {
      const updated = await renameMeetingSpeaker(transcript, id, label);
      if (revision === meetingDocumentRevision) {
        meetingTranscript = updated;
        meetingError = "";
      }
    });
  }

  async function mergeMeetingReviewSpeakers(fromId: string, intoId: string): Promise<void> {
    await enqueueMeetingAction(async (transcript, revision) => {
      const updated = await mergeMeetingSpeakers(transcript, fromId, intoId);
      if (revision === meetingDocumentRevision) {
        meetingTranscript = updated;
        meetingError = "";
      }
    });
  }

  async function exportMeetingReview(format: MeetingExportFormat): Promise<void> {
    await enqueueMeetingAction(async (transcript) => {
      await saveMeetingExport(transcript, format);
    });
  }

  function onTranscribeProfileChange(e: Event) {
    const nextProfileId = (e.target as HTMLSelectElement).value;
    transcribeProfileId = profileForId(nextProfileId)?.id ?? null;
  }

  async function onPickFile() {
    const exts = supportedFormats.length > 0 ? supportedFormats : ["wav", "mp3", "m4a", "mp4", "ogg", "flac"];
    const file = await open({
      multiple: false,
      filters: [
        {
          name: "Audio/Video",
          extensions: exts,
        },
      ],
    });
    if (file) {
      await handleFileTranscription(file);
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
    const m = modifierNames();
    return shortcut
      .replace(/Control/g, m.ctrl)
      .replace(/Alt/g, m.alt)
      .replace(/Super/g, m.meta)
      .split("+")
      .join(" + ");
  }

  function beginHotkeyCapture(profileId: string) {
    hotkeyCaptureGeneration += 1;
    recordingProfileId = profileId;
    hotkeyError = "";
  }

  async function onHotkeyKeydown(e: KeyboardEvent, profileId: string) {
    const captureGeneration = hotkeyCaptureGeneration;
    e.preventDefault();
    e.stopPropagation();

    // Escape cancels recording
    if (e.key === "Escape") {
      hotkeyCaptureGeneration += 1;
      recordingProfileId = null;
      hotkeyError = "";
      return;
    }

    // Ignore bare modifier presses — wait for a non-modifier key
    if (["Control", "Shift", "Alt", "Meta"].includes(e.key)) return;

    const keyName = tauriKeyName(e.key);
    if (!keyName) {
      hotkeyError = `"${e.key}" is not a supported key. Use A–Z, 0–9, F1–F24, Space, Arrow keys, or Tab/Enter/Delete.`;
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

    if (!settings) return;
    const profiles = settings.hotkey_profiles.map((profile) =>
      profile.id === profileId ? { ...profile, shortcut } : profile
    );
    setHotkeyProfiles(profiles)
      .then(async () => {
        recordingProfileId = null;
        draftProfileId = null;
        settings = await getSettings();
      })
      .catch((err: any) => {
        hotkeyError = typeof err === "string" ? err : err.message || "Failed to set hotkey";
      });
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
    const profile: HotkeyProfile = {
      id: `profile-${suffix}`,
      name: `Profile ${suffix}`,
      shortcut: "Control+Option+Shift+F12",
      language: settings.language === "sv" ? "en" : "sv",
    };
    settings = { ...settings, hotkey_profiles: [...settings.hotkey_profiles, profile] };
    draftProfileId = profile.id;
    beginHotkeyCapture(profile.id);
    void refreshProfileModels(settings.hotkey_profiles);
  }

  async function removeProfile(profileId: string) {
    if (!settings || settings.hotkey_profiles.length <= 1) return;
    if (profileId === draftProfileId) {
      settings = { ...settings, hotkey_profiles: settings.hotkey_profiles.filter((profile) => profile.id !== profileId) };
      draftProfileId = null;
      recordingProfileId = null;
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
    <button class="tab" class:active={activeTab === "dictate"} onclick={() => (activeTab = "dictate")}>
      Dictate
    </button>
    <button class="tab" class:active={activeTab === "transcribe"} onclick={() => (activeTab = "transcribe")}>
      Transcribe
    </button>
    <button class="tab" class:active={activeTab === "settings"} onclick={() => (activeTab = "settings")}>
      Settings
    </button>
  </div>

  {#if settings}
    <div class="content">
      {#if initError}
        <div class="transcribe-error">{initError}</div>
      {/if}
      {#if settingsError}
        <div class="transcribe-error">{settingsError}</div>
      {/if}
      {#if activeTab === "dictate"}
        <div class="field profile-field">
          <div class="profile-heading">
            <span class="field-label">{settings.hotkey_mode === "presenter" ? "Presenter start shortcuts" : "Dictation shortcuts"}</span>
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
                  <option value="auto">Auto-detect</option>
                </select>
              </div>
              <div class="profile-row">
                {#if recordingProfileId === profile.id}
                  <button
                    class="hotkey-recorder recording"
                    bind:this={hotkeyRecorderEl}
                    onkeydown={(event) => onHotkeyKeydown(event, profile.id)}
                    onblur={() => { recordingProfileId = null; hotkeyError = ""; }}
                  >Press shortcut...</button>
                {:else}
                  <button
                    class="hotkey-recorder"
                    onclick={() => beginHotkeyCapture(profile.id)}
                  >{formatHotkeyDisplay(profile.shortcut)}</button>
                {/if}
                {#if settings.hotkey_profiles.length > 1}
                  <button class="profile-remove" aria-label={`Remove ${profile.name}`} onclick={() => removeProfile(profile.id)}>Remove</button>
                {/if}
              </div>
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
            Each shortcut selects its language. Use a modifier ({modifierNames().meta}, {modifierNames().ctrl}, {modifierNames().alt}, Shift) + key{#if supportedBareFunctionKeyRange(platform)}, or {supportedBareFunctionKeyRange(platform)} by itself{/if}.{#if platform === "macos"}{" "}Bare F13–F24 requires Accessibility permission: macOS sends keyboard events to Sagascript, which immediately ignores everything except bare F13–F24 and never stores or sends them.{/if}
          </div>
        </div>

        <div class="field">
          <label for="hotkey-mode">Shortcut behavior</label>
          <select id="hotkey-mode" value={settings.hotkey_mode} onchange={onHotkeyModeChange}>
            <option value="push">Push-to-talk</option>
            <option value="toggle">Toggle</option>
            <option value="presenter">Presenter</option>
          </select>
        </div>

        {#if settings.hotkey_mode === "presenter"}
          <PresenterSettings
            config={settings.presenter}
            profileShortcuts={settings.hotkey_profiles.map((profile) => profile.shortcut)}
            {platform}
            onSave={onPresenterSave}
          />
        {/if}

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
            disabled={dictateButtonAction(backendDictationState, testOwnsRecording) === "blocked" || downloading !== null}
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
          {#if presenterStatus}
            <div class="presenter-status" role="status" aria-live="polite">
              {presenterStatusLabels[presenterStatus]}
            </div>
          {/if}
          <textarea
            class="test-result"
            bind:value={testResult}
            placeholder="Click here and use your hotkey, or press the button above"
          ></textarea>
        </div>

      {:else if activeTab === "transcribe"}
        <button class="active-config-bar" onclick={() => (activeTab = "settings")}>
          <div class="active-config-row">
            <span class="active-config-label">Language</span>
            <span class="active-config-value">{languageLabel(transcribeLanguage())}</span>
          </div>
          <span class="active-config-link">Settings</span>
        </button>

        <div
          class="drop-zone"
          class:drag-over={dragOver}
          class:transcribing={transcribing}
        >
          {#if transcribing}
            <div class="spinner"></div>
            {#if meetingJobStatus !== null}
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
              <div class="drop-zone-text">Transcribing... {transcriptionProgress}%</div>
              <div class="progress-bar transcription-progress">
                <div class="progress-fill" style="width: {transcriptionProgress}%"></div>
              </div>
            {/if}
          {:else}
            <div class="drop-zone-icon">&#x1F4C1;</div>
            <div class="drop-zone-text">Drop an audio or video file here</div>
            <button class="primary open-file-btn" onclick={onPickFile}>
              Open File...
            </button>
          {/if}
        </div>

        <div class="formats-hint">
          Supported: {supportedFormats.map(f => f.toUpperCase()).join(", ") || "WAV, MP3, M4A, AAC, MP4, MOV, OGG, WEBM, FLAC"}
        </div>

        <div class="transcribe-options">
          <div class="field">
            <label for="transcribe-profile">Profile (optional)</label>
            <select id="transcribe-profile" value={transcribeProfileId ?? ""} onchange={onTranscribeProfileChange} disabled={transcribing}>
              <option value="">No profile (use selected language)</option>
              {#each explicitProfiles() as profile (profile.id)}
                <option value={profile.id}>{profile.name} · {languageLabel(profile.language)}</option>
              {/each}
            </select>
          </div>
          {#if selectedTranscribeProfile()}
            <div class="hotkey-hint">This profile fixes the file language and uses its personal dictionary.</div>
          {:else}
            <div class="hotkey-hint">No profile keeps the selected language and global hint context.</div>
          {/if}
          <label class="diarize-option">
            <input type="checkbox" bind:checked={transcribeDiarize} disabled={transcribing} />
            Speaker diarization
          </label>
          <textarea
            class="prompt-input"
            aria-label="Extra context for this file"
            placeholder="Extra context for this file only (optional)"
            bind:value={transcribePrompt}
            rows="2"
            disabled={transcribing}
          ></textarea>
          <div class="hotkey-hint">Temporary hint-only context for this import. A selected profile supplies its dictionary; no profile uses global hints.</div>
        </div>

        {#if transcribeError}
          <div class="transcribe-error">{transcribeError}</div>
        {/if}

        {#if meetingError && !meetingTranscript}
          <div class="transcribe-error">{meetingError}</div>
        {/if}

        {#if transcriptionResult}
          <div class="result-label">Result</div>
          <textarea class="transcribe-result" readonly>{transcriptionResult}</textarea>
        {/if}

        {#if meetingTranscript}
          {#key meetingDocumentRevision}
            <MeetingReview
              transcript={meetingTranscript}
              busy={transcribing}
              error={meetingError || null}
              onRename={renameMeetingReviewSpeaker}
              onMerge={mergeMeetingReviewSpeakers}
              onExport={exportMeetingReview}
            />
          {/key}
        {/if}

      {:else if activeTab === "settings"}
        <div class="field">
          <label for="language">Language</label>
          <select id="language" value={settings.language} onchange={onLanguageChange}>
            <option value="en">English</option>
            <option value="sv">Swedish</option>
            <option value="no">Norwegian</option>
            <option value="auto">Auto-detect</option>
          </select>
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
          <label for="initial-prompt">Personal dictionary</label>
          <select id="dictionary-scope" value={glossaryScopeId} onchange={onGlossaryScopeChange}>
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
            onblur={onInitialPromptBlur}
            oninput={onGlossaryInput}
            placeholder="OpenRouter = open router | open vrouter&#10;merge = merch&#10;Cloudflare = cloud flare"
          ></textarea>
          {#if glossaryScopeId === ""}
            <div class="hotkey-hint glossary-migration">
              Global entries are hint-only and remain stored. To enable deterministic alias replacements, copy an entry into the explicit-language profile that should use it.
            </div>
          {:else}
            <div class="hotkey-hint glossary-migration">
              This explicit-language profile supplies deterministic aliases for its language. Leaving this field saves this profile only; switching scope never moves entries to another dictionary.
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
            Plain terms still guide Whisper. Saved automatically when you leave the field and used for live dictation and batch jobs.
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

  .profile-row .hotkey-recorder {
    flex: 1;
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
  }

  .initial-prompt-input:focus {
    border-color: var(--accent);
  }

  .loading {
    padding: 40px 20px;
    text-align: center;
    color: var(--text-muted);
  }

  /* Transcribe tab */

  .drop-zone {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    padding: 32px 20px;
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

  .transcription-progress {
    width: 80%;
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
    margin-top: 14px;
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

  .presenter-status {
    margin-top: 8px;
    color: var(--text-muted);
    font-size: 12px;
    line-height: 1.4;
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
</style>
