<script lang="ts">
  let {
    micStatus,
    nextStep,
    stopPoll,
    openMicrophoneSettingsAndPoll,
    grantMicrophone,
  }: {
    micStatus: string;
    nextStep: () => void;
    stopPoll: () => void;
    openMicrophoneSettingsAndPoll: () => void;
    grantMicrophone: () => void;
  } = $props();
</script>

<div class="step">
  <div class="icon">
    <svg width="48" height="48" viewBox="0 0 48 48" fill="none">
      <rect width="48" height="48" rx="12" fill="var(--accent-dim)" />
      <path d="M24 12a4 4 0 0 0-4 4v8a4 4 0 0 0 8 0v-8a4 4 0 0 0-4-4z" stroke="var(--accent)" stroke-width="2" fill="none" />
      <path d="M16 22v2a8 8 0 0 0 16 0v-2M24 32v4m-4 0h8" stroke="var(--accent)" stroke-width="2" stroke-linecap="round" />
    </svg>
  </div>
  <h1>Microphone Access</h1>
  <p class="description">
    Sagascript can record your voice for live dictation. All audio is processed locally on your device — nothing leaves your device.
  </p>

  {#if micStatus === "authorized"}
    <div class="status-indicator granted">
      <span class="status-dot"></span>
      <span>Microphone access granted</span>
    </div>
    <div class="actions">
      <button class="primary" onclick={nextStep}>Continue</button>
    </div>
  {:else if micStatus === "denied"}
    <div class="status-indicator">
      <span class="status-dot"></span>
      <span>Permission was denied</span>
    </div>
    <p class="description">
      Please enable Sagascript in
      <strong>System Settings &rsaquo; Privacy &amp; Security &rsaquo; Microphone</strong>.
    </p>
    <div class="actions">
      <button class="primary" onclick={openMicrophoneSettingsAndPoll}>
        <span class="button-spinner"></span>
        Waiting — Open System Settings
      </button>
      <button
        class="secondary"
        onclick={() => {
          stopPoll();
          nextStep();
        }}
      >
        I don't need this — I'll only transcribe files
      </button>
    </div>
  {:else if micStatus === "restricted"}
    <div class="status-indicator">
      <span class="status-dot"></span>
      <span>Restricted by your organization</span>
    </div>
    <div class="actions">
      <button class="secondary" onclick={nextStep}>Skip</button>
    </div>
  {:else if micStatus === "unsupported"}
    <div class="status-indicator">
      <span class="status-dot"></span>
      <span>Permission checks require a built app bundle. Skip to continue.</span>
    </div>
    <div class="actions">
      <button class="primary" onclick={nextStep}>Skip</button>
    </div>
  {:else}
    <!-- not_determined or checking -->
    <div class="status-indicator">
      <span class="status-dot"></span>
      <span>Microphone access not granted</span>
    </div>
    <div class="actions">
      <button class="primary" onclick={grantMicrophone} disabled={micStatus === "checking"}>
        {#if micStatus === "checking"}
          <span class="button-spinner"></span>
          Waiting for permission...
        {:else}
          Grant Microphone Access
        {/if}
      </button>
      <button
        class="secondary"
        onclick={() => {
          stopPoll();
          nextStep();
        }}
      >
        I don't need this — I'll only transcribe files
      </button>
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

  strong {
    color: var(--accent);
    font-weight: 600;
  }
</style>
