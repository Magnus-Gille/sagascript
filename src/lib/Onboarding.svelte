<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import { listen } from "@tauri-apps/api/event";
  import {
    getPlatform,
    getSettings,
    setLanguage,
    downloadModel,
    checkMicrophonePermission,
    requestMicrophonePermission,
    checkAccessibilityPermission,
    requestAccessibilityPermission,
    setOnboardingCompleted,
  } from "./api";

  let { oncomplete }: { oncomplete: () => void } = $props();

  type Step = "welcome" | "language" | "download" | "microphone" | "accessibility" | "ready";
  type OnboardingLanguage = "en" | "sv" | "no";

  let currentStep: Step = $state("welcome");
  let platform = $state("macos");

  // Language selection — seeded from existing settings in onMount
  let selectedLanguage: OnboardingLanguage = $state("en");

  // Model download
  let downloading = $state(false);
  let downloadProgress = $state(0);
  let downloadError: string | null = $state(null);
  let downloadComplete = $state(false);

  // Permissions
  let micGranted = $state(false);
  let accessibilityGranted = $state(false);
  let micChecking = $state(false);
  let accessibilityChecking = $state(false);
  let pollTimer: ReturnType<typeof setInterval> | null = $state(null);

  // Hotkey (read from settings)
  let hotkeyParts: string[] = $state(["Ctrl", "Shift", "Space"]);

  // Cleanup for event listeners
  let unlistenProgress: (() => void) | null = null;
  let unlistenReady: (() => void) | null = null;

  // Model info per onboarding language (no "auto" — onboarding always picks a specific language)
  const modelInfo: Record<OnboardingLanguage, { name: string; size: string }> = {
    en: { name: "Base English", size: "142 MB" },
    sv: { name: "KB-Whisper Base", size: "60 MB" },
    no: { name: "NB-Whisper Base", size: "55 MB" },
  };

  // Recommended model ID per language (must match Rust serde rename)
  const recommendedModelId: Record<OnboardingLanguage, string> = {
    en: "base.en",
    sv: "kb-whisper-base",
    no: "nb-whisper-base",
  };

  function getSteps(): Step[] {
    if (platform === "macos") {
      return ["welcome", "language", "download", "microphone", "accessibility", "ready"];
    }
    return ["welcome", "language", "download", "ready"];
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
    await setLanguage(selectedLanguage);
    nextStep();
  }

  // -- Model download --

  async function startDownload() {
    downloading = true;
    downloadError = null;
    downloadProgress = 0;

    try {
      await downloadModel(recommendedModelId[selectedLanguage]);
      // model-ready event will set downloadComplete
    } catch (e: any) {
      downloading = false;
      downloadError = typeof e === "string" ? e : e?.message ?? "Download failed. Check your internet connection.";
    }
  }

  function skipDownload() {
    downloading = false;
    downloadProgress = 0;
    nextStep();
  }

  // -- Microphone --

  async function grantMicrophone() {
    micChecking = true;
    const result = await requestMicrophonePermission();
    if (result) {
      micGranted = true;
      micChecking = false;
      return;
    }
    startPoll(async () => {
      const granted = await checkMicrophonePermission();
      if (granted) {
        micGranted = true;
        micChecking = false;
        stopPoll();
      }
    });
  }

  // -- Accessibility --

  async function grantAccessibility() {
    accessibilityChecking = true;
    await requestAccessibilityPermission();
    startPoll(async () => {
      const granted = await checkAccessibilityPermission();
      if (granted) {
        accessibilityGranted = true;
        accessibilityChecking = false;
        stopPoll();
      }
    });
  }

  function startPoll(fn: () => Promise<void>) {
    stopPoll();
    pollTimer = setInterval(fn, 1000);
  }

  function stopPoll() {
    if (pollTimer) {
      clearInterval(pollTimer);
      pollTimer = null;
    }
  }

  async function finish() {
    stopPoll();
    await setOnboardingCompleted();
    oncomplete();
  }

  function formatProgress(pct: number): string {
    return `${Math.round(pct)}%`;
  }

  onMount(async () => {
    platform = await getPlatform();
    micGranted = await checkMicrophonePermission();
    accessibilityGranted = await checkAccessibilityPermission();

    // Seed language and hotkey from existing settings
    try {
      const settings = await getSettings();
      hotkeyParts = settings.hotkey.split("+");
      // Map existing language to onboarding options (auto → en since onboarding requires a specific choice)
      const lang = settings.language;
      if (lang === "en" || lang === "sv" || lang === "no") {
        selectedLanguage = lang;
      }
    } catch {
      // keep defaults
    }

    // Listen for download progress — use backend-computed progress field
    unlistenProgress = await listen("model-download-progress", (event: any) => {
      downloadProgress = event.payload.progress;
    });

    unlistenReady = await listen("model-ready", () => {
      downloading = false;
      downloadComplete = true;
      downloadProgress = 100;
    });
  });

  onDestroy(() => {
    stopPoll();
    unlistenProgress?.();
    unlistenReady?.();
  });
</script>

<div class="onboarding">
  <div class="titlebar">
    <span class="titlebar-text">Sagascript</span>
  </div>

  <div class="progress">
    {#each getSteps() as _, i}
      <div
        class="dot"
        class:active={i === stepIndex()}
        class:completed={i < stepIndex()}
      ></div>
    {/each}
  </div>

  <div class="content">
    <!-- Welcome -->
    {#if currentStep === "welcome"}
      <div class="step">
        <div class="icon">
          <svg width="48" height="48" viewBox="0 0 48 48" fill="none">
            <rect width="48" height="48" rx="12" fill="var(--accent-dim)" />
            <path
              d="M24 14v8m0 0v8m0-8h8m-8 0h-8"
              stroke="var(--accent)"
              stroke-width="2.5"
              stroke-linecap="round"
            />
            <circle cx="24" cy="24" r="14" stroke="var(--accent)" stroke-width="2" fill="none" />
          </svg>
        </div>
        <h1>Welcome to Sagascript</h1>
        <p class="description">
          Turn speech into text — dictate anywhere with a hotkey, or transcribe
          audio files. All processing happens locally on your device.
        </p>
        <p class="subdescription">Let's get you set up in a few quick steps.</p>
        <div class="actions">
          <button class="primary" onclick={nextStep}>Get Started</button>
        </div>
      </div>

    <!-- Language -->
    {:else if currentStep === "language"}
      <div class="step">
        <div class="icon">
          <svg width="48" height="48" viewBox="0 0 48 48" fill="none">
            <rect width="48" height="48" rx="12" fill="var(--accent-dim)" />
            <path
              d="M14 18h10M19 14v4M16 22c1.5 3 4 5.5 7 7M22 22c-1.5 3-4 5.5-7 7"
              stroke="var(--accent)"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
            />
            <path
              d="M26 34l3-8 3 8M27 32h4"
              stroke="var(--accent)"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
            />
          </svg>
        </div>
        <h1>What language do you speak?</h1>
        <p class="description">
          We'll download a speech engine optimised for your language.
          You can add more languages later in Settings.
        </p>
        <div class="language-options">
          <button
            class="language-option"
            class:selected={selectedLanguage === "en"}
            onclick={() => selectedLanguage = "en"}
          >
            <span class="lang-flag">EN</span>
            <span class="lang-name">English</span>
          </button>
          <button
            class="language-option"
            class:selected={selectedLanguage === "sv"}
            onclick={() => selectedLanguage = "sv"}
          >
            <span class="lang-flag">SV</span>
            <span class="lang-name">Svenska</span>
          </button>
          <button
            class="language-option"
            class:selected={selectedLanguage === "no"}
            onclick={() => selectedLanguage = "no"}
          >
            <span class="lang-flag">NO</span>
            <span class="lang-name">Norsk</span>
          </button>
        </div>
        <div class="actions">
          <button class="primary" onclick={selectLanguageAndContinue}>Continue</button>
        </div>
      </div>

    <!-- Download -->
    {:else if currentStep === "download"}
      <div class="step">
        <div class="icon">
          <svg width="48" height="48" viewBox="0 0 48 48" fill="none">
            <rect width="48" height="48" rx="12" fill="var(--accent-dim)" />
            <path
              d="M24 16v12m0 0l-4-4m4 4l4-4M16 32h16"
              stroke="var(--accent)"
              stroke-width="2"
              stroke-linecap="round"
              stroke-linejoin="round"
            />
          </svg>
        </div>
        <h1>Setting up speech engine</h1>
        <p class="description">
          Downloading {modelInfo[selectedLanguage].name} ({modelInfo[selectedLanguage].size}).
          This runs entirely on your device — no cloud needed.
        </p>

        {#if downloadError}
          <div class="status-indicator error">
            <span class="status-dot"></span>
            <span>{downloadError}</span>
          </div>
          <div class="actions">
            <button class="primary" onclick={startDownload}>Try Again</button>
            <button class="secondary" onclick={nextStep}>Skip for now</button>
          </div>
        {:else if downloadComplete}
          <div class="status-indicator granted">
            <span class="status-dot"></span>
            <span>Speech engine ready</span>
          </div>
          <div class="actions">
            <button class="primary" onclick={nextStep}>Continue</button>
          </div>
        {:else if downloading}
          <div class="progress-bar-container">
            <div class="progress-bar" style="width: {downloadProgress}%"></div>
          </div>
          <p class="progress-text">{formatProgress(downloadProgress)}</p>
          <div class="actions">
            <button class="secondary" onclick={skipDownload}>Skip for now</button>
          </div>
        {:else}
          <div class="actions">
            <button class="primary" onclick={startDownload}>Download</button>
            <button class="secondary" onclick={nextStep}>Skip for now</button>
          </div>
        {/if}
      </div>

    <!-- Microphone (macOS only) -->
    {:else if currentStep === "microphone"}
      <div class="step">
        <div class="icon">
          <svg width="48" height="48" viewBox="0 0 48 48" fill="none">
            <rect width="48" height="48" rx="12" fill="var(--accent-dim)" />
            <path
              d="M24 12a4 4 0 0 0-4 4v8a4 4 0 0 0 8 0v-8a4 4 0 0 0-4-4z"
              stroke="var(--accent)"
              stroke-width="2"
              fill="none"
            />
            <path
              d="M16 22v2a8 8 0 0 0 16 0v-2M24 32v4m-4 0h8"
              stroke="var(--accent)"
              stroke-width="2"
              stroke-linecap="round"
            />
          </svg>
        </div>
        <h1>Microphone Access</h1>
        <p class="description">
          Sagascript can record your voice for live dictation. All audio is
          processed locally on your device — nothing leaves your device.
        </p>

        <div class="status-indicator" class:granted={micGranted}>
          <span class="status-dot"></span>
          <span>{micGranted ? "Microphone access granted" : "Microphone access not granted"}</span>
        </div>

        <div class="actions">
          {#if micGranted}
            <button class="primary" onclick={nextStep}>Continue</button>
          {:else}
            <button class="primary" onclick={grantMicrophone} disabled={micChecking}>
              {#if micChecking}
                <span class="button-spinner"></span>
                Waiting for permission...
              {:else}
                Grant Microphone Access
              {/if}
            </button>
            <button class="secondary" onclick={() => { stopPoll(); micChecking = false; nextStep(); }}>
              I don't need this — I'll only transcribe files
            </button>
          {/if}
        </div>
      </div>

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
          This lets Sagascript paste transcribed text directly into any app
          after you dictate. Without it, text goes to your clipboard and you
          paste manually.
        </p>

        <div class="status-indicator" class:granted={accessibilityGranted}>
          <span class="status-dot"></span>
          <span>{accessibilityGranted ? "Accessibility granted" : "Accessibility not granted"}</span>
        </div>

        <div class="actions">
          {#if accessibilityGranted}
            <button class="primary" onclick={nextStep}>Continue</button>
          {:else}
            <button class="primary" onclick={grantAccessibility} disabled={accessibilityChecking}>
              {#if accessibilityChecking}
                <span class="button-spinner"></span>
                Waiting for permission...
              {:else}
                Open System Settings
              {/if}
            </button>
            <button class="secondary" onclick={() => { stopPoll(); accessibilityChecking = false; nextStep(); }}>
              I'll paste manually
            </button>
          {/if}
        </div>
      </div>

    <!-- Ready -->
    {:else if currentStep === "ready"}
      <div class="step">
        <div class="icon">
          <svg width="48" height="48" viewBox="0 0 48 48" fill="none">
            <rect width="48" height="48" rx="12" fill="var(--accent-dim)" />
            <path
              d="M17 25l5 5 10-12"
              stroke="var(--accent)"
              stroke-width="2.5"
              stroke-linecap="round"
              stroke-linejoin="round"
            />
          </svg>
        </div>
        <h1>You're All Set!</h1>

        {#if micGranted || platform !== "macos"}
          <div class="hotkey-display">
            <div class="hotkey-keys">
              {#each hotkeyParts as part, i}
                {#if i > 0}<span class="key-sep">+</span>{/if}
                <kbd>{part}</kbd>
              {/each}
            </div>
            <p class="hotkey-hint">Hold to record, release to transcribe</p>
          </div>
        {:else}
          <p class="description">
            Open the <strong>Transcribe</strong> tab to convert audio files to text.
            You can grant microphone access later in Settings if you want live dictation.
          </p>
        {/if}

        {#if !downloadComplete}
          <p class="subdescription warning">
            You skipped the speech engine download. Open Settings to download one before dictating.
          </p>
        {/if}

        <p class="subdescription">You can change any of this in Settings.</p>

        <div class="actions">
          <button class="primary" onclick={finish}>Start Using Sagascript</button>
        </div>
      </div>
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
    transition: background 0.3s, transform 0.3s;
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
    from { opacity: 0; transform: translateY(8px); }
    to { opacity: 1; transform: translateY(0); }
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

  .subdescription {
    font-size: 12px;
    color: var(--text-muted);
    opacity: 0.7;
    margin: 0 0 24px;
  }

  .subdescription.warning {
    color: var(--danger);
    opacity: 1;
  }

  /* Language selector */

  .language-options {
    display: flex;
    gap: 10px;
    margin: 20px 0 24px;
  }

  .language-option {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 6px;
    padding: 14px 20px;
    background: var(--bg-secondary);
    border: 2px solid var(--border);
    border-radius: 10px;
    cursor: pointer;
    transition: border-color 0.2s, background 0.2s;
    min-width: 90px;
  }

  .language-option:hover {
    border-color: var(--text-muted);
  }

  .language-option.selected {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 8%, var(--bg-secondary));
  }

  .lang-flag {
    font-size: 14px;
    font-weight: 700;
    color: var(--accent);
    letter-spacing: 1px;
  }

  .lang-name {
    font-size: 12px;
    color: var(--text-muted);
  }

  .language-option.selected .lang-name {
    color: var(--text);
  }

  /* Progress bar */

  .progress-bar-container {
    width: 100%;
    max-width: 300px;
    height: 6px;
    background: var(--bg-secondary);
    border-radius: 3px;
    overflow: hidden;
    margin: 20px 0 8px;
  }

  .progress-bar {
    height: 100%;
    background: var(--accent);
    border-radius: 3px;
    transition: width 0.3s ease;
  }

  .progress-text {
    font-size: 12px;
    color: var(--text-muted);
    margin: 0 0 16px;
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
    to { transform: rotate(360deg); }
  }

  /* Hotkey display */

  .hotkey-display {
    margin: 16px 0 8px;
  }

  .hotkey-keys {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 4px;
    margin-bottom: 8px;
  }

  kbd {
    display: inline-block;
    padding: 6px 12px;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: 6px;
    font-family: inherit;
    font-size: 14px;
    font-weight: 600;
    color: var(--accent);
    box-shadow: 0 2px 0 var(--border);
  }

  .key-sep {
    color: var(--text-muted);
    font-size: 14px;
  }

  .hotkey-hint {
    font-size: 12px;
    color: var(--text-muted);
    margin: 0;
  }

  strong {
    color: var(--accent);
    font-weight: 600;
  }
</style>
