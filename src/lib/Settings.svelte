<script lang="ts">
  import { tick } from "svelte";
  import DownloadStatusBar from "./settings/DownloadStatusBar.svelte";
  import QueueStatus from "./settings/QueueStatus.svelte";
  import TabPanel from "./settings/TabPanel.svelte";
  import { useSettingsWindow } from "./settings/settingsWindow.svelte";
  import { SETTINGS_TABS, type SettingsTabDefinition } from "./settings/tabs";

  // The tabs are SETTINGS_TABS; the app never passes `tabs`, a test does to show a panel is one entry.
  let { tabs = SETTINGS_TABS }: { tabs?: readonly SettingsTabDefinition[] } = $props();

  // The window's state and logic live in its controllers (settings/settingsWindow.svelte.ts),
  // which register their effects, listeners and mount callbacks here, as the component initializes.
  // The tabs are read once, as the window initializes.
  // svelte-ignore state_referenced_locally
  const win = useSettingsWindow(tabs.map((tab) => tab.id));
  const { session, router, recovery, errors } = win;

  const settings = $derived(session.settings);
  // A change that fails is reported on its own tab (settings/tabErrors.svelte.ts): the
  // message of the tab that shows is drawn here, and a tab that has one while another
  // tab shows is marked in the strip.
  const tabError = $derived(errors.of(router.active));
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
    {#each tabs as tab (tab.id)}
      {@const hasHiddenError = router.active !== tab.id && errors.of(tab.id).message !== ""}
      <button
        class="tab"
        class:active={router.active === tab.id}
        aria-label={hasHiddenError ? `${tab.label}, has an error` : undefined}
        onclick={() => router.request(tab.id)}
        disabled={router.held}
      >
        {tab.label}
        {#if hasHiddenError}<span class="tab-error-mark" aria-hidden="true"></span>{/if}
      </button>
    {/each}
  </div>

  {#if settings}
    <div class="content" style:--notice-clearance={session.unconfirmed ? `${unconfirmedNoticeHeight + 12}px` : null}>
      {#if session.initError}
        <div class="transcribe-error">{session.initError}</div>
      {/if}
      {#if recovery.readError}
        <div class="transcribe-error" role="alert">{recovery.readError}</div>
      {/if}
      {#if recovery.discardError}
        <div class="transcribe-error" role="alert">{recovery.discardError}</div>
      {/if}
      {#if tabError.message}
        <div class="transcribe-error" role={router.asking ? undefined : "alert"}>{tabError.message}</div>
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
      <QueueStatus {win} />
      {#each tabs as tab (tab.id)}
        <TabPanel {tab} {win} />
      {/each}
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

  <DownloadStatusBar {win} />
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

  /* The tab has an error of its own that shows when the tab is opened. */
  .tab-error-mark {
    display: inline-block;
    width: 7px;
    height: 7px;
    margin-left: 4px;
    border-radius: 50%;
    background: var(--danger);
    vertical-align: middle;
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
</style>
