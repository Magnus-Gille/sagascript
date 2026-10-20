<script lang="ts">
  // The way out of a failed transcription, offered under its error (ADR 0006,
  // #360). The caller decides which with `backendErrorRecovery(error)`, which
  // reads the error's `code` and `retryable` and never its text: "Try again"
  // restarts the user's action (record again, run the file again), "Download
  // the model again" repairs the engine's model.
  //
  // One button carries both labels, so a finished download turns "Download the
  // model again" into "Try again" without losing the focus the person has on
  // it. While the action cannot start it is `aria-disabled` and guarded, not
  // `disabled`: a disabled button drops the focus to the page.
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

  function act(): void {
    if (busy) return;
    if (recovery === "retry") onRetry();
    else if (recovery === "download_model") onDownloadModel();
  }
</script>

{#if recovery}
  <!-- aria-live: the relabel (a finished download turns the button into "Try
       again") is a change inside this region, which a screen reader announces.
       The region that holds the label is the one that says it; the download bar
       is another element and does not say that the model is ready. -->
  <div class="error-recovery" aria-live="polite">
    <button class="secondary" type="button" aria-disabled={busy ? "true" : undefined} onclick={act}>
      {recovery === "retry" ? "Try again" : "Download the model again"}
    </button>
  </div>
{/if}

<style>
  .error-recovery { margin-top: 8px; }
  .error-recovery button { margin: 0; }
  button[aria-disabled="true"] { opacity: 0.5; cursor: default; }
</style>
