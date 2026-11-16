<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { toErrorMessage } from "./errors";
  import { hotkeyKeyLabel } from "./hotkey";
  import { downloadModelById } from "./models.svelte";
  import { waitForAccessibility } from "./permissions";
  import { loadPlatform } from "./platform";
  import { awaitDownloadCompletion, completedDownloadState } from "./onboarding-download-state";
  import { useTauriEvent } from "./tauri-events.svelte";
  import DownloadStep from "./onboarding/DownloadStep.svelte";
  import LanguageStep from "./onboarding/LanguageStep.svelte";
  import MicrophoneStep from "./onboarding/MicrophoneStep.svelte";
  import ReadyStep from "./onboarding/ReadyStep.svelte";
  import type { OnboardingLanguage } from "./onboarding/types";
  import {
    getSettings,
    setLanguage,
    getEffectiveModelInfo,
    microphoneStatus,
    requestMicrophoneAccess,
    openMicrophoneSettings,
    checkAccessibilityPermission,
    requestAccessibilityPermission,
    retryHotkeyRegistration,
    openAccessibilitySettings,
    setAutoPaste,
    setOnboardingCompleted,
  } from "./api";

  let { oncomplete }: { oncomplete: () => void | Promise<void> } = $props();

  type Step = "language" | "download" | "microphone" | "accessibility" | "ready";

  let currentStep: Step = $state("language");
  let platform: string | null = $state(null);

  // Language selection — seeded from existing settings in onMount
  let selectedLanguage: OnboardingLanguage = $state("en");

  // Model download
  let downloading = $state(false);
  let downloadProgress = $state(0);
  let downloadError: string | null = $state(null);
  let downloadComplete = $state(false);

  // Language step / final "finish" step errors
  let languageError: string | null = $state(null);
  let languageSaving = $state(false);
  let finishError: string | null = $state(null);
  let accessibilityError: string | null = $state(null);

  // Permissions
  // micStatus: "authorized" | "not_determined" | "denied" | "restricted" | "unsupported" | "checking"
  let micStatus: string = $state("checking");
  let accessibilityGranted = $state(false);
  let accessibilityChecking = $state(false);
  let accessibilityOpening = $state(false);
  let savingManualPaste = $state(false);
  let pollTimer: ReturnType<typeof setTimeout> | null = null;
  let pollGeneration = 0;

  // Hotkey (read from settings)
  let hotkeyParts: string[] = $state(["Ctrl", "Shift", "Space"]);

  let componentDestroyed = false;

  // Registered as the component initializes, before any initialization invoke:
  // the component is interactive while those awaits are pending, so listeners
  // registered after them could miss a fast model-ready event.
  const downloadEvents = [
    useTauriEvent("model-download-progress", ({ progress }) => {
      downloadProgress = progress;
    }),
    useTauriEvent("model-ready", markDownloadComplete),
  ];

  // Fallback model ID per language (must match Rust serde rename). The backend
  // decides the real recommendation (Swedish is Pianissimo where supported,
  // otherwise KB-Whisper Medium), see getEffectiveModelInfo below.
  const recommendedModelId: Record<OnboardingLanguage, string> = {
    en: "base.en",
    sv: "kb-whisper-medium",
    no: "nb-whisper-base",
    fi: "base",
    da: "medium",
  };
  let recommendedSizeMb: number | null = $state(null);

  async function downloadRecommendedModel(language: OnboardingLanguage): Promise<void> {
    let id = recommendedModelId[language];
    try {
      const recommended = await getEffectiveModelInfo(language);
      id = recommended.id;
      recommendedSizeMb = recommended.size_mb > 0 ? recommended.size_mb : null;
    } catch {
      // Fall back to the static recommendation; the download reports real errors.
    }
    await downloadModelById(id);
  }

  function getSteps(): Step[] {
    if (platform === "macos") {
      return ["language", "download", "microphone", "accessibility", "ready"];
    }
    return ["language", "download", "ready"];
  }

  function nextStep() {
    const steps = getSteps();
    const idx = steps.indexOf(currentStep);
    if (idx < steps.length - 1) {
      currentStep = steps[idx + 1];
    }
  }

  function stepIndex(): number {
    return getSteps().indexOf(currentStep);
  }

  // -- Language --

  async function selectLanguageAndContinue() {
    if (languageSaving || platform === null) return;
    languageError = null;
    languageSaving = true;
    try {
      await setLanguage(selectedLanguage);
      nextStep();
      void startDownload();
    } catch (e) {
      languageError = toErrorMessage(e, "Failed to save language. Please try again.");
    } finally {
      languageSaving = false;
    }
  }

  // -- Model download --

  function markDownloadComplete() {
    const completed = completedDownloadState();
    downloading = completed.downloading;
    downloadComplete = completed.downloadComplete;
    downloadProgress = completed.downloadProgress;
  }

  async function startDownload() {
    downloading = true;
    downloadComplete = false;
    downloadError = null;
    downloadProgress = 0;

    try {
      await awaitDownloadCompletion(downloadRecommendedModel(selectedLanguage), markDownloadComplete);
      // The command result is authoritative. `model-ready` remains useful for
      // live progress, but a fast verification may emit it before listeners
      // finish registering.
    } catch (e) {
      downloading = false;
      downloadError = toErrorMessage(e, "Download failed. Check your internet connection.");
    }
  }

  // -- Microphone --

  function startMicrophonePoll(generation: number) {
    startPoll(generation, async (isCurrent) => {
      try {
        const polled = await microphoneStatus();
        if (!isCurrent()) return;
        micStatus = polled;
        if (polled === "authorized") stopPoll();
      } catch {
        // A transient query failure is retried by the serialized poll loop.
      }
    });
  }

  async function openMicrophoneSettingsAndPoll() {
    const generation = beginPollOperation();
    try {
      await openMicrophoneSettings();
      if (!isPollCurrent(generation)) return;
      startMicrophonePoll(generation);
    } catch {
      // Leave the denied state visible so the user can retry opening Settings.
    }
  }

  async function grantMicrophone() {
    const generation = beginPollOperation();
    try {
      micStatus = "checking";
      const result = await requestMicrophoneAccess();
      if (!isPollCurrent(generation)) return;
      micStatus = result;
      if (result === "denied") {
        // macOS won't re-show the dialog — open System Settings and poll
        await openMicrophoneSettings();
        if (!isPollCurrent(generation)) return;
        startMicrophonePoll(generation);
      }
    } catch {
      if (!isPollCurrent(generation)) return;
      // Prevent spinner getting stuck
      const fallback = await microphoneStatus().catch(() => "not_determined");
      if (!isPollCurrent(generation)) return;
      micStatus = fallback;
    }
  }

  // -- Accessibility --

  async function grantAccessibility() {
    const generation = beginPollOperation();
    const isCancelled = () => !isPollCurrent(generation);
    accessibilityError = null;
    accessibilityChecking = true;
    try {
      await requestAccessibilityPermission();
    } catch (e) {
      if (isCancelled()) return;
      accessibilityError = toErrorMessage(e, "Failed to request Accessibility permission. Please try again.");
      accessibilityChecking = false;
      return;
    }
    if (isCancelled()) return;

    let granted: boolean;
    try {
      // Until the user decides or leaves the step: `stopPoll()` cancels the wait.
      granted = await waitForAccessibility({ attempts: Infinity, isCancelled });
    } catch (e) {
      if (isCancelled()) return;
      stopPoll();
      accessibilityChecking = false;
      accessibilityError = toErrorMessage(e, "Failed to check Accessibility permission. Please try again.");
      return;
    }
    if (!granted) return;

    accessibilityGranted = true;
    accessibilityChecking = false;
    stopPoll();
    try {
      await retryHotkeyRegistration();
    } catch (e) {
      accessibilityError = `Accessibility was granted, but the saved dictation shortcut could not be registered: ${toErrorMessage(e, "unknown error")}`;
    }
  }

  async function reopenAccessibilitySettings() {
    if (accessibilityOpening) return;
    accessibilityError = null;
    accessibilityOpening = true;
    try {
      // Reopen the pane without asking macOS to show its modal permission prompt again.
      await openAccessibilitySettings();
    } catch (e) {
      accessibilityError = toErrorMessage(e, "Failed to open Accessibility settings. Please try again.");
    } finally {
      accessibilityOpening = false;
    }
  }

  async function continueWithAccessibility() {
    stopPoll();
    accessibilityChecking = true;
    accessibilityError = null;
    try {
      await setAutoPaste(true);
      nextStep();
    } catch (e) {
      accessibilityError = toErrorMessage(e, "Failed to enable auto-paste. Please try again.");
    } finally {
      accessibilityChecking = false;
    }
  }

  async function skipAccessibility() {
    stopPoll();
    accessibilityChecking = false;
    savingManualPaste = true;
    accessibilityError = null;
    try {
      // "Paste manually" is a product setting, not merely permission advice.
      // Persist it now so finishing onboarding cannot leave auto-paste enabled.
      await setAutoPaste(false);
      nextStep();
    } catch (e) {
      accessibilityError = toErrorMessage(e, "Failed to disable auto-paste. Please try again.");
    } finally {
      savingManualPaste = false;
    }
  }

  function beginPollOperation(): number {
    stopPoll();
    return pollGeneration;
  }

  function isPollCurrent(generation: number): boolean {
    return generation === pollGeneration;
  }

  function startPoll(generation: number, fn: (isCurrent: () => boolean) => Promise<void>) {
    if (!isPollCurrent(generation)) return;

    const run = async () => {
      if (!isPollCurrent(generation)) return;
      pollTimer = null;
      await fn(() => isPollCurrent(generation));
      if (!isPollCurrent(generation)) return;
      pollTimer = setTimeout(run, 1000);
    };

    pollTimer = setTimeout(run, 1000);
  }

  function stopPoll() {
    pollGeneration += 1;
    if (pollTimer) {
      clearTimeout(pollTimer);
      pollTimer = null;
    }
  }

  async function finish() {
    finishError = null;
    try {
      stopPoll();
      await setOnboardingCompleted();
      await oncomplete();
    } catch (e) {
      finishError = toErrorMessage(e, "Failed to complete setup. Please try again.");
    }
  }

  onMount(async () => {
    // Failed registrations are already logged; the download still completes
    // through its command result.
    await Promise.allSettled(downloadEvents.map((events) => events.ready));
    if (componentDestroyed) return;

    try {
      platform = await loadPlatform();
    } catch (error) {
      // Never fall back to the macOS-only permission flow when platform
      // detection fails. "unknown" follows the portable onboarding path.
      console.error("Failed to detect platform", error);
      platform = "unknown";
    }
    try {
      micStatus = await microphoneStatus();
    } catch {
      micStatus = "not_determined";
    }
    accessibilityGranted = await checkAccessibilityPermission();

    // Seed language and hotkey from existing settings
    try {
      const settings = await getSettings();
      hotkeyParts = settings.hotkey.split("+").map((part) => hotkeyKeyLabel(part, platform));
      // Map existing language to onboarding options (auto → en since onboarding requires a specific choice)
      const lang = settings.language;
      if (lang === "en" || lang === "sv" || lang === "no" || lang === "fi" || lang === "da") {
        selectedLanguage = lang;
      }
    } catch {
      // keep defaults
    }
  });

  onDestroy(() => {
    componentDestroyed = true;
    stopPoll();
  });
</script>

<div class="onboarding">
  <div class="titlebar">
    <span class="titlebar-text">Sagascript</span>
  </div>

  <div class="progress">
    {#each getSteps() as step, i (step)}
      <div class="dot" class:active={i === stepIndex()} class:completed={i < stepIndex()}></div>
    {/each}
  </div>

  <div class="content">
    <!-- Language -->
    {#if currentStep === "language"}
      <LanguageStep
        {selectedLanguage}
        selectLanguage={(language) => (selectedLanguage = language)}
        {languageError}
        {languageSaving}
        {platform}
        {selectLanguageAndContinue}
      />

      <!-- Download -->
    {:else if currentStep === "download"}
      <DownloadStep
        {selectedLanguage}
        {recommendedSizeMb}
        {downloadError}
        {downloadComplete}
        {downloading}
        {downloadProgress}
        {startDownload}
        {nextStep}
      />

      <!-- Microphone (macOS only) -->
    {:else if currentStep === "microphone"}
      <MicrophoneStep {micStatus} {nextStep} {stopPoll} {openMicrophoneSettingsAndPoll} {grantMicrophone} />

      <!-- Accessibility (macOS only) -->
    {:else if currentStep === "accessibility"}
      <div class="step">
        <div class="icon">
          <svg width="48" height="48" viewBox="0 0 48 48" fill="none">
            <rect width="48" height="48" rx="12" fill="var(--accent-dim)" />
            <circle cx="24" cy="17" r="3" stroke="var(--accent)" stroke-width="2" fill="none" />
            <path
              d="M16 23h16M24 23v10M20 36l4-3 4 3"
              stroke="var(--accent)"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
            />
          </svg>
        </div>
        <h1>Accessibility Permission</h1>
        <p class="description">
          This lets Sagascript paste transcribed text directly into any app after you dictate. Without it, text goes to your clipboard and
          you paste manually.
        </p>

        <div class="status-indicator" class:granted={accessibilityGranted}>
          <span class="status-dot"></span>
          <span>
            {accessibilityGranted
              ? "Accessibility granted"
              : accessibilityChecking
                ? "Waiting for permission in System Settings"
                : "Accessibility not granted"}
          </span>
        </div>

        <div class="actions">
          {#if accessibilityGranted}
            <button class="primary" onclick={continueWithAccessibility} disabled={accessibilityChecking}>
              {accessibilityChecking ? "Saving…" : "Continue"}
            </button>
          {:else if accessibilityChecking}
            <button class="primary" onclick={reopenAccessibilitySettings} disabled={accessibilityOpening}>
              {#if accessibilityOpening}
                <span class="button-spinner"></span>
                Opening…
              {:else}
                Open Accessibility Settings Again
              {/if}
            </button>
            <button class="secondary" onclick={skipAccessibility} disabled={savingManualPaste || accessibilityOpening}>
              I'll paste manually
            </button>
          {:else}
            <button class="primary" onclick={grantAccessibility}> Open System Settings </button>
            <button class="secondary" onclick={skipAccessibility} disabled={savingManualPaste}> I'll paste manually </button>
          {/if}
        </div>
        {#if accessibilityError}
          <div class="status-indicator error" role="alert" aria-live="assertive">
            <span class="status-dot"></span>
            <span>{accessibilityError}</span>
          </div>
        {/if}
      </div>

      <!-- Ready -->
    {:else if currentStep === "ready"}
      <ReadyStep {downloadComplete} {micStatus} {platform} {hotkeyParts} {finishError} {finish} />
    {/if}
  </div>
</div>

<style>
  .onboarding {
    height: 100vh;
    display: flex;
    flex-direction: column;
    background: var(--bg);
    overflow: hidden;
  }

  .titlebar {
    height: 38px;
    display: flex;
    align-items: center;
    justify-content: center;
    -webkit-app-region: drag;
    flex-shrink: 0;
  }

  .titlebar-text {
    font-size: 12px;
    color: var(--text-muted);
    font-weight: 500;
  }

  .progress {
    display: flex;
    justify-content: center;
    gap: 8px;
    padding: 8px 0 16px;
    flex-shrink: 0;
  }

  .dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--border);
    transition:
      background 0.3s,
      transform 0.3s;
  }

  .dot.active {
    background: var(--accent);
    transform: scale(1.3);
  }

  .dot.completed {
    background: color-mix(in srgb, var(--accent) 50%, transparent);
  }

  .content {
    flex: 1;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 0 40px 40px;
  }

  .step {
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
    max-width: 380px;
    animation: fadeIn 0.3s ease;
  }

  @keyframes fadeIn {
    from {
      opacity: 0;
      transform: translateY(8px);
    }
    to {
      opacity: 1;
      transform: translateY(0);
    }
  }

  .icon {
    margin-bottom: 20px;
  }

  h1 {
    font-size: 20px;
    font-weight: 600;
    color: var(--text);
    margin: 0 0 12px;
  }

  .description {
    font-size: 13px;
    line-height: 1.6;
    color: var(--text-muted);
    margin: 0 0 8px;
  }

  /* Status indicators */

  .status-indicator {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 8px 16px;
    border-radius: var(--radius);
    background: color-mix(in srgb, var(--danger) 8%, var(--bg-secondary));
    font-size: 12px;
    color: var(--text-muted);
    margin: 16px 0 24px;
    transition: background 0.3s;
  }

  .status-indicator.granted {
    background: color-mix(in srgb, var(--accent) 8%, var(--bg-secondary));
  }

  .status-indicator.error {
    background: color-mix(in srgb, var(--danger) 12%, var(--bg-secondary));
    color: var(--danger);
  }

  .status-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--danger);
    flex-shrink: 0;
    transition: background 0.3s;
  }

  .status-indicator.granted .status-dot {
    background: var(--accent);
  }

  .status-indicator.error .status-dot {
    background: var(--danger);
  }

  /* Actions */

  .actions {
    display: flex;
    flex-direction: column;
    gap: 10px;
    width: 100%;
    max-width: 300px;
  }

  .actions button {
    width: 100%;
    padding: 10px 20px;
    font-size: 13px;
  }

  .button-spinner {
    display: inline-block;
    width: 14px;
    height: 14px;
    border: 2px solid transparent;
    border-top-color: currentColor;
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
    vertical-align: middle;
    margin-right: 6px;
  }

  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
</style>
