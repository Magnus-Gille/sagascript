<script lang="ts">
  import { onMount, onDestroy } from "svelte";
  import {
    getPlatform,
    checkMicrophonePermission,
    requestMicrophonePermission,
    checkAccessibilityPermission,
    requestAccessibilityPermission,
  } from "./api";

  export let oncomplete: () => void;

  type Step = "welcome" | "microphone" | "accessibility" | "ready";

  let currentStep: Step = "welcome";
  let platform = "macos";
  let micGranted = false;
  let accessibilityGranted = false;
  let micChecking = false;
  let accessibilityChecking = false;
  let pollTimer: ReturnType<typeof setInterval> | null = null;

  // Determine the ordered list of steps based on platform
  function getSteps(): Step[] {
    if (platform === "macos") {
      return ["welcome", "microphone", "accessibility", "ready"];
    }
    return ["welcome", "ready"];
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

  function totalSteps(): number {
    return getSteps().length;
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
    // Poll until granted or user moves on
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
    // Poll until granted or user moves on
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
    // Write hasCompletedOnboarding to store
    const { load } = await import("@tauri-apps/plugin-store");
    const store = await load("flowdictate-settings.json");
    await store.set("hasCompletedOnboarding", true);
    await store.save();
    oncomplete();
  }

  onMount(async () => {
    platform = await getPlatform();
    // Pre-check current permission states
    micGranted = await checkMicrophonePermission();
    accessibilityGranted = await checkAccessibilityPermission();
  });

  onDestroy(() => {
    stopPoll();
  });
</script>

<div class="onboarding">
  <div class="titlebar">
    <span class="titlebar-text">FlowDictate</span>
  </div>

  <!-- Progress dots -->
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
    <!-- Step 0: Welcome -->
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
        <h1>Welcome to FlowDictate</h1>
        <p class="description">
          Turn speech into text — dictate anywhere with a hotkey, or transcribe
          audio files. All processing happens locally on your device.
        </p>
        <p class="subdescription">Let's walk through a few optional setup steps.</p>
        <div class="actions">
          <button class="primary" on:click={nextStep}>Get Started</button>
        </div>
      </div>

    <!-- Step 1: Microphone (macOS only) -->
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
          FlowDictate can record your voice for live dictation. All audio is
          processed locally on your device — nothing leaves your Mac.
        </p>

        <div class="status-indicator" class:granted={micGranted}>
          <span class="status-dot"></span>
          <span>{micGranted ? "Microphone access granted" : "Microphone access not granted"}</span>
        </div>

        <div class="actions">
          {#if micGranted}
            <button class="primary" on:click={nextStep}>Continue</button>
          {:else}
            <button class="primary" on:click={grantMicrophone} disabled={micChecking}>
              {#if micChecking}
                <span class="button-spinner"></span>
                Waiting for permission...
              {:else}
                Grant Microphone Access
              {/if}
            </button>
            <button class="secondary" on:click={() => { stopPoll(); micChecking = false; nextStep(); }}>
              I don't need this — I'll only transcribe files
            </button>
          {/if}
        </div>
      </div>

    <!-- Step 2: Accessibility (macOS only) -->
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
          This lets FlowDictate paste transcribed text directly into any app
          after you dictate. Without it, text goes to your clipboard and you
          paste manually.
        </p>

        <div class="status-indicator" class:granted={accessibilityGranted}>
          <span class="status-dot"></span>
          <span>{accessibilityGranted ? "Accessibility granted" : "Accessibility not granted"}</span>
        </div>

        <div class="actions">
          {#if accessibilityGranted}
            <button class="primary" on:click={nextStep}>Continue</button>
          {:else}
            <button class="primary" on:click={grantAccessibility} disabled={accessibilityChecking}>
              {#if accessibilityChecking}
                <span class="button-spinner"></span>
                Waiting for permission...
              {:else}
                Open System Settings
              {/if}
            </button>
            <button class="secondary" on:click={() => { stopPoll(); accessibilityChecking = false; nextStep(); }}>
              I'll paste manually
            </button>
          {/if}
        </div>
      </div>

    <!-- Step 3: Ready -->
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
              <kbd>Ctrl</kbd><span class="key-sep">+</span><kbd>Shift</kbd><span class="key-sep">+</span><kbd>Space</kbd>
            </div>
            <p class="hotkey-hint">Hold to record, release to transcribe</p>
          </div>
        {:else}
          <p class="description">
            Open the <strong>Transcribe</strong> tab to convert audio files to text.
            You can grant microphone access later in Settings if you want live dictation.
          </p>
        {/if}

        <p class="subdescription">You can change any of this in Settings.</p>

        <div class="actions">
          <button class="primary" on:click={finish}>Start Using FlowDictate</button>
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
