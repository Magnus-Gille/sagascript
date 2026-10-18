<script lang="ts">
  // The way out of a failed transcription, offered under its error (ADR 0006,
  // #360). The caller decides which with `backendErrorRecovery(error)`, which
  // reads the error's `code` and `retryable` and never its text: "Try again"
  // restarts the user's action (record again, run the file again), "Download
  // the model again" repairs the engine's model.
  import type { BackendErrorRecovery } from "./errors";

  let {
    recovery,
    busy = false,
    onRetry,
    onDownloadModel,
  }: {
    recovery: BackendErrorRecovery | null;
    /** The action cannot start now: a download is running, or other work is. */
    busy?: boolean;
    onRetry: () => void;
    onDownloadModel: () => void;
  } = $props();
</script>

{#if recovery === "retry"}
  <div class="error-recovery">
    <button class="secondary" type="button" onclick={onRetry} disabled={busy}>Try again</button>
  </div>
{:else if recovery === "download_model"}
  <div class="error-recovery">
    <button class="secondary" type="button" onclick={onDownloadModel} disabled={busy}>Download the model again</button>
  </div>
{/if}

<style>
  .error-recovery { margin-top: 8px; }
  .error-recovery button { margin: 0; }
  button:disabled { opacity: 0.5; cursor: default; }
</style>
