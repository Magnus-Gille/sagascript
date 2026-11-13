<script lang="ts">
  import type { DiarizationReport, MeetingExportFormat } from "../meeting-types";

  interface Props {
    idPrefix: string;
    acousticReport: DiarizationReport | null;
    pendingAction: string | null;
    persistenceDisabled: () => boolean;
    exportReview: (format: MeetingExportFormat) => Promise<void>;
  }

  let { idPrefix, acousticReport, pendingAction, persistenceDisabled, exportReview }: Props = $props();

  const exportFormats: Array<{ format: MeetingExportFormat; label: string }> = [
    { format: "plain", label: "Plain text" },
    { format: "markdown", label: "Markdown" },
    { format: "json", label: "Review JSON" },
    { format: "srt", label: "SRT subtitles" },
    { format: "vtt", label: "WebVTT subtitles" },
  ];
</script>

<section class="export-panel" aria-labelledby={idPrefix + "export-panel-title"}>
  <div class="section-heading">
    <div>
      <h2 id={idPrefix + "export-panel-title"}>Export reviewed transcript</h2>
      <p>Choose a format explicitly. Each export uses the current reviewed text and speaker labels.</p>
    </div>
  </div>
  {#if acousticReport}<p class="diagnostic-note export-limitation">
      Plain text and subtitle exports use transcript segments and cannot represent simultaneous acoustic activity. Review JSON retains the
      source report.
    </p>{/if}
  <div class="export-actions">
    {#each exportFormats as item (item.format)}
      <button type="button" class="secondary export-button" disabled={persistenceDisabled()} onclick={() => void exportReview(item.format)}>
        {pendingAction === "export-" + item.format ? "Exporting…" : item.label}
      </button>
    {/each}
  </div>
</section>

<style>
  .section-heading {
    display: flex;
    gap: 12px;
  }
  .section-heading {
    align-items: flex-start;
    justify-content: space-between;
  }
  h2 {
    font-size: 16px;
  }
  .section-heading p {
    color: var(--text-muted);
    font-size: 12px;
  }
  .export-panel {
    margin-top: 24px;
  }
  .section-heading {
    margin-bottom: 12px;
  }
  .section-heading p {
    margin-top: 3px;
  }
  .diagnostic-note {
    margin: 10px 0 0;
    color: var(--text-muted);
    font-size: 12px;
    line-height: 1.5;
  }
  .export-actions {
    display: grid;
    grid-template-columns: repeat(5, minmax(0, 1fr));
    gap: 8px;
  }
  .export-button {
    min-height: 40px;
  }
  .export-limitation {
    margin-bottom: 10px;
  }
  @media (max-width: 600px) {
    .section-heading {
      flex-direction: column;
    }
    .export-actions {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
  }
</style>
