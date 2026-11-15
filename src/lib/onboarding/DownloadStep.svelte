<script lang="ts">
  import type { OnboardingLanguage } from "./types";

  let {
    selectedLanguage,
    recommendedSizeMb,
    downloadError,
    downloadComplete,
    downloading,
    downloadProgress,
    startDownload,
    nextStep,
  }: {
    selectedLanguage: OnboardingLanguage;
    recommendedSizeMb: number | null;
    downloadError: string | null;
    downloadComplete: boolean;
    downloading: boolean;
    downloadProgress: number;
    startDownload: () => void;
    nextStep: () => void;
  } = $props();

  const engineSize: Record<OnboardingLanguage, string> = {
    en: "142 MB",
    sv: "about 600 MB",
    no: "55 MB",
    fi: "142 MB",
    da: "1530 MB",
  };

  function formatProgress(pct: number): string {
    return `${Math.round(pct)}%`;
  }
</script>

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
    Preparing the local speech engine ({recommendedSizeMb !== null ? `${recommendedSizeMb} MB` : engineSize[selectedLanguage]}). Your
    recordings are processed on this device.
  </p>

  {#if downloadError}
    <div class="status-indicator error">
      <span class="status-dot"></span>
      <span>{downloadError}</span>
    </div>
    <div class="actions">
      <button class="primary" onclick={startDownload}>Try Again</button>
      <button class="secondary" onclick={nextStep}>Continue without speech engine</button>
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
      <div class="progress-bar" style:width="{downloadProgress}%"></div>
    </div>
    <p class="progress-text">{formatProgress(downloadProgress)}</p>
  {:else}
    <div class="actions">
      <button class="primary" onclick={startDownload}>Download</button>
    </div>
  {/if}
</div>

<style>
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
</style>
