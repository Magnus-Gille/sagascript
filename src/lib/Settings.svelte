<script lang="ts">
  import { fileJobName } from "./transcription-queue";
  import { tick } from "svelte";
  import DictateTab from "./settings/DictateTab.svelte";
  import SettingsTab from "./settings/SettingsTab.svelte";
  import TranscribeTab from "./settings/TranscribeTab.svelte";
  import { useSettingsWindow } from "./settings/settingsWindow.svelte";

  // The window's state and logic live in its controllers (settings/settingsWindow.svelte.ts),
  // which register their effects, listeners and mount callbacks here, as the component initializes.
  const win = useSettingsWindow();
  const { session, router, queue, recovery } = win;
  const modelDownloads = win.downloads;

  const settings = $derived(session.settings);
  const downloading = $derived(modelDownloads.id);
  const downloadingName = $derived(modelDownloads.name);
  const downloadProgress = $derived(modelDownloads.progress);
  const updatePreparing = $derived(recovery.preparing);

  // Height of the sticky notice, so focus scrolling can leave room for it.
  let unconfirmedNoticeHeight = $state(0);
  let reloadButtonEl: HTMLButtonElement | undefined = $state();
  let windowTitleEl: HTMLHeadingElement | undefined = $state();

  // When the lock engages, a focused control that just became disabled drops the
  // keyboard focus; hand it to Reload settings, the one thing to do next.
  let lockFocusHandled = false;
  $effect(() => {
    if (!session.locked) {
      lockFocusHandled = false;
      return;
    }
    if (lockFocusHandled) return;
    lockFocusHandled = true;
    void tick().then(() => {
      const active = document.activeElement;
      if (!active || active === document.body || active.matches(":disabled")) reloadButtonEl?.focus();
    });
  });

  async function reloadSettings(): Promise<void> {
    if (session.reloading) return;
    session.reloading = true;
    const hadFocus = document.activeElement === reloadButtonEl;
    try {
      await session.sync();
    } finally {
      session.reloading = false;
    }
    // The button leaves with the notice: keep a keyboard user's place in the window.
    if (hadFocus && !session.unconfirmed) {
      await tick();
      windowTitleEl?.focus();
    }
  }
</script>

<div class="settings-window" class:settings-locked={session.locked}>
  <header class="window-header">
    <h1 class="window-title" bind:this={windowTitleEl} tabindex="-1">Sagascript</h1>
    <div class="build-info" aria-label="Build information">
      {#if win.buildInfo}
        Version {win.buildInfo.version} · Build {win.buildInfo.git_hash} · {win.buildInfo.build_date}{win.engineHostIdentity
          ? ` · ${win.engineHostIdentity}`
          : ""}
      {:else}
        Version information unavailable
      {/if}
    </div>
  </header>

  <div class="tabs">
    <button class="tab" class:active={router.active === "dictate"} onclick={() => router.request("dictate")} disabled={router.held}>
      Dictate
    </button>
    <button class="tab" class:active={router.active === "transcribe"} onclick={() => router.request("transcribe")} disabled={router.held}>
      Transcribe
    </button>
    <button class="tab" class:active={router.active === "settings"} onclick={() => router.request("settings")} disabled={router.held}>
      Settings
    </button>
  </div>

  {#if settings}
    <div class="content" style:--notice-clearance={session.unconfirmed ? `${unconfirmedNoticeHeight + 12}px` : null}>
      {#if session.initError}
        <div class="transcribe-error">{session.initError}</div>
      {/if}
      {#if recovery.readError}
        <div class="transcribe-error" role="alert">{recovery.readError}</div>
      {/if}
      {#if session.error}
        <div class="transcribe-error" role={router.asking ? undefined : "alert"}>{session.error}</div>
      {/if}
      {#if session.unconfirmed}
        <section
          class="recovery-notice settings-unconfirmed"
          role="alert"
          aria-label="Settings not confirmed"
          bind:offsetHeight={unconfirmedNoticeHeight}
        >
          <span>
            {session.unconfirmed.afterSave
              ? "Your change was saved, but could not be read back to confirm it"
              : "The settings could not be re-read"}
            ({session.unconfirmed.detail}). Values shown may be out of date; reload before changing anything.
          </span>
          <button
            class="secondary"
            type="button"
            bind:this={reloadButtonEl}
            aria-disabled={session.reloading ? "true" : "false"}
            onclick={() => void reloadSettings()}>Reload settings</button
          >
        </section>
      {/if}
      {#if recovery.draftsNotice}
        <section class="recovery-notice" role="status" aria-label="Recovered drafts">
          <strong>Recovered drafts</strong>
          <span>Unsaved dictation and transcription results were restored after the update.</span>
          <button class="secondary" type="button" disabled={updatePreparing} onclick={() => void recovery.discardDrafts()}
            >Discard recovered drafts</button
          >
        </section>
      {/if}
      <p
        class="queue-summary"
        class:queue-empty={queue.jobs.length === 0}
        role="status"
        aria-label="File transcription status"
        aria-atomic="true"
      >
        {#if queue.jobs.length}
          {queue.jobs.filter((job) => job.status === "completed" && !queue.busy[job.id]).length} completed ·
          {queue.jobs.filter((job) => job.status === "failed").length} failed ·
          {queue.jobs.filter((job) => job.status === "cancelled").length} cancelled ·
          {queue.jobs.filter((job) => job.status === "queued").length} queued ·
          {queue.jobs.filter((job) => job.status === "running" || queue.busy[job.id]).length} running ·
          {queue.needingRetry.length} needs retry
        {/if}
      </p>
      {#if queue.needingRetry.length}
        <div class="queue-attention">
          {#each queue.needingRetry as job (job.id)}
            <div>
              <span
                >{fileJobName(job.path)} needs a status check.{#if queue.jobs.some((file) => file.status === "queued")}
                  The queue is paused.{/if}</span
              >
              <button class="secondary" onclick={() => queue.showNeedingRetry(job.id)}>Show {fileJobName(job.path)}</button>
            </div>
          {/each}
        </div>
      {/if}
      {#if router.active === "dictate"}
        <DictateTab {win} active={true} />
      {/if}
      <TranscribeTab {win} active={router.active === "transcribe"} />
      <SettingsTab {win} active={router.active === "settings"} />
    </div>
  {:else}
    <div class="loading">
      {#if session.initError}
        <div class="transcribe-error">{session.initError}</div>
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
        <div class="download-status-fill" style:width="{downloadProgress}%"></div>
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

  /* While the settings are unconfirmed, the controls that save a setting are disabled;
     make that visible. Only then: other disabled states keep their look. Placed before the
     scoped :disabled rules below so those win where they set the same properties. */
  .settings-locked button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
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

  /* Programmatic focus (after Reload settings) rings the title only for keyboard users. */
  .window-title:focus:not(:focus-visible) {
    outline: none;
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
    /* One value for the padding and for the sticky notice's offset inside it. */
    --content-pad: 20px;
    padding: var(--content-pad);
    /* While the notice is shown, focus scrolling keeps a control clear of it. */
    scroll-padding-top: var(--notice-clearance, 0px);
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

  .loading {
    padding: 40px 20px;
    text-align: center;
    color: var(--text-muted);
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

  /* The notice must stay in view while the user scrolls to the control they changed.
     Sticky offsets are measured inside the scroller's padding, so minus that padding
     sticks the notice flush with the scroller's top edge. The first shadow paints over
     the 12px margin below it, so scrolled content does not show in that gap. */
  .settings-unconfirmed {
    position: sticky;
    top: calc(-1 * var(--content-pad));
    z-index: 5;
    background: var(--bg);
    box-shadow:
      0 12px 0 0 var(--bg),
      0 18px 10px -6px rgba(0, 0, 0, 0.6);
    padding: 8px 12px;
    font-size: 12px;
    line-height: 1.4;
  }

  .recovery-notice span {
    flex: 1;
    color: var(--text-muted);
  }
  .recovery-notice button {
    flex-shrink: 0;
  }

  .queue-summary {
    font-size: 12px;
    color: var(--text-muted);
    margin-bottom: 12px;
    line-height: 1.6;
  }

  .queue-summary.queue-empty {
    margin: 0;
  }

  .queue-attention {
    border: 1px solid var(--accent);
    border-radius: var(--radius);
    padding: 10px 12px;
    margin-bottom: 12px;
    font-size: 13px;
  }

  .queue-attention > div {
    display: flex;
    align-items: center;
    gap: 12px;
    justify-content: space-between;
  }
  .queue-attention button {
    flex-shrink: 0;
  }
</style>
