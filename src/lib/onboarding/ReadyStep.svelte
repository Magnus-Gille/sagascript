<script lang="ts">
  let {
    downloadComplete,
    micStatus,
    platform,
    hotkeyParts,
    finishError,
    finish,
  }: {
    downloadComplete: boolean;
    micStatus: string;
    platform: string | null;
    hotkeyParts: string[];
    finishError: string | null;
    finish: () => void;
  } = $props();
</script>

<div class="step">
  <div class="icon">
    <svg width="48" height="48" viewBox="0 0 48 48" fill="none">
      <rect width="48" height="48" rx="12" fill="var(--accent-dim)" />
      <path d="M17 25l5 5 10-12" stroke="var(--accent)" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round" />
    </svg>
  </div>
  <h1>You're All Set!</h1>

  {#if !downloadComplete}
    <div class="status-indicator error" role="alert">
      <span class="status-dot"></span>
      <span>Speech engine is not installed yet. Download it from Dictate before your first use.</span>
    </div>
  {/if}

  {#if micStatus === "authorized" || platform !== "macos"}
    <div class="hotkey-display">
      <div class="hotkey-keys">
        {#each hotkeyParts as part, i (i)}
          {#if i > 0}<span class="key-sep">+</span>{/if}
          <kbd>{part}</kbd>
        {/each}
      </div>
      <p class="hotkey-hint hotkey-mode-intro">Configure separate shortcuts for these recording modes in Dictate.</p>
      <div class="hotkey-mode-copy">
        <p class="hotkey-mode-label">Hold to record</p>
        <p class="hotkey-hint">Hold the shortcut while speaking. Release to stop.</p>
        <p class="hotkey-mode-label">Press to start/stop</p>
        <p class="hotkey-hint">Press the shortcut to start recording. Press it again to stop.</p>
      </div>
    </div>
  {:else}
    <p class="description">
      Open the <strong>Transcribe</strong> tab to convert audio files to text. You can grant microphone access later in Settings if you want live
      dictation.
    </p>
  {/if}

  <p class="subdescription">You can change any of this in Settings.</p>

  {#if finishError}
    <div class="status-indicator error">
      <span class="status-dot"></span>
      <span>{finishError}</span>
    </div>
  {/if}

  <div class="actions">
    <button class="primary" onclick={finish}>Start Using Sagascript</button>
  </div>
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

  .subdescription {
    font-size: 12px;
    color: var(--text-muted);
    opacity: 0.7;
    margin: 0 0 24px;
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

  .hotkey-mode-copy {
    display: grid;
    gap: 2px;
    text-align: left;
  }

  .hotkey-mode-label {
    font-size: 12px;
    color: var(--text);
    font-weight: 600;
    margin: 4px 0 0;
  }

  strong {
    color: var(--accent);
    font-weight: 600;
  }
</style>
