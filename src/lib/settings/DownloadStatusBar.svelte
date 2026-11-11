<script lang="ts">
  import type { SettingsWindow } from "./settingsWindow.svelte";

  let { win }: { win: SettingsWindow } = $props();
  // The window is one object for the life of the component.
  // svelte-ignore state_referenced_locally
  const modelDownloads = win.downloads;

  const downloading = $derived(modelDownloads.id);
  const downloadingName = $derived(modelDownloads.name);
  const downloadProgress = $derived(modelDownloads.progress);
</script>

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

<style>
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
</style>
