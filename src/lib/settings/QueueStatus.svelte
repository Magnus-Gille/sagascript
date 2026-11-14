<script lang="ts">
  import { fileJobName } from "../transcription-queue";
  import type { SettingsWindow } from "./settingsWindow.svelte";

  let { win }: { win: SettingsWindow } = $props();
  // The window is one object for the life of the component.
  // svelte-ignore state_referenced_locally
  const { queue } = win;
</script>

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

<style>
  :global(.content) > * {
    min-width: 0;
    max-width: 100%;
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
