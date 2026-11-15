<script lang="ts">
  import type { DiarizationReport } from "../meeting-types";

  interface Props {
    idPrefix: string;
    acousticReport: DiarizationReport;
    displayLabel: (id: string) => string;
    formatTimestamp: (seconds: number) => string;
  }

  let { idPrefix, acousticReport, displayLabel, formatTimestamp }: Props = $props();

  const overlapSpans = $derived(acousticReport?.activity.filter((span) => span.speakers.length > 1) ?? []);
  const renderedOverlapSpans = $derived(overlapSpans.slice(0, 100));
  const omittedOverlapCount = $derived(Math.max(0, overlapSpans.length - renderedOverlapSpans.length));
  const overlapDuration = $derived(overlapSpans.reduce((total, span) => total + Math.max(0, span.end - span.start), 0));
  const fallbackRegionCount = $derived(acousticReport?.regions.filter((region) => region.used_track_fallback).length ?? 0);
  const attributionReasons = $derived.by(() => {
    const counts: Record<string, number> = Object.create(null);
    for (const attribution of acousticReport?.attributions ?? []) {
      counts[attribution.reason] = (counts[attribution.reason] ?? 0) + 1;
    }
    return Object.entries(counts).sort(([left], [right]) => left.localeCompare(right));
  });
</script>

<section class="acoustic-panel" aria-labelledby={idPrefix + "acoustic-panel-title"}>
  <div class="section-heading">
    <div>
      <h2 id={idPrefix + "acoustic-panel-title"}>Acoustic activity</h2>
      <p>Original speaker activity from the recording, kept separate from reviewed transcript text.</p>
    </div>
    {#if acousticReport.transcript_modified}<span class="edited-badge">Transcript edited</span>{/if}
  </div>
  {#if overlapSpans.length > 0}
    <p class="overlap-notice" role="status">
      Simultaneous speaker activity appears in {overlapSpans.length} interval{overlapSpans.length === 1 ? "" : "s"}
      ({overlapDuration.toFixed(2)} seconds). Transcript words remain shown once; this panel does not duplicate words for overlapping speakers.
    </p>
    <div class="acoustic-overlap-list" role="list" aria-label="Simultaneous speaker activity">
      {#each renderedOverlapSpans as span (span.start + "-" + span.end + "-" + span.speakers.join(","))}
        <div class="acoustic-overlap-row" role="listitem">
          <span class="timestamp">{formatTimestamp(span.start)}–{formatTimestamp(span.end)}</span>
          <span>{span.speakers.map(displayLabel).join(", ")}</span>
        </div>
      {/each}
    </div>
    {#if omittedOverlapCount > 0}
      <p class="diagnostic-note">
        {omittedOverlapCount} additional overlap interval{omittedOverlapCount === 1 ? " was" : "s were"} omitted from this view.
      </p>
    {/if}
  {:else}
    <p class="empty">No simultaneous speaker activity was recorded in the acoustic timeline.</p>
  {/if}
  <p class="diagnostic-note">
    Diagnostic evidence only: activity and any included attribution evidence are not calibrated speaker confidence.
    {#if acousticReport.diagnostics_included === true}
      {#if acousticReport.decoder}
        Decoder {acousticReport.decoder.strategy}, beam {acousticReport.decoder.beam_size}, VAD {acousticReport.decoder.vad_enabled
          ? "on"
          : "off"}.{/if}
    {:else}
      Additional diagnostic evidence was not included for this run.
    {/if}
  </p>
  {#if acousticReport.diagnostics_included === true && (fallbackRegionCount > 0 || attributionReasons.length > 0)}
    <div class="diagnostic-details" aria-label="Diarization diagnostic evidence">
      {#if fallbackRegionCount > 0}<span>{fallbackRegionCount} region{fallbackRegionCount === 1 ? "" : "s"} used track fallback.</span>{/if}
      {#if attributionReasons.length > 0}<span
          >Attribution reasons: {attributionReasons.map(([reason, count]) => `${reason} (${count})`).join(", ")}.</span
        >{/if}
    </div>
  {/if}
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
  .section-heading p,
  .empty {
    color: var(--text-muted);
    font-size: 12px;
  }
  .acoustic-panel {
    margin-top: 24px;
  }
  .section-heading {
    margin-bottom: 12px;
  }
  .section-heading p {
    margin-top: 3px;
  }
  .acoustic-panel {
    padding: 14px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg-secondary);
  }
  .overlap-notice {
    margin: 0 0 10px;
    color: var(--text);
    font-size: 13px;
    line-height: 1.5;
  }
  .acoustic-overlap-list {
    display: grid;
    gap: 6px;
  }
  .acoustic-overlap-row {
    display: flex;
    align-items: baseline;
    gap: 12px;
    padding: 8px 10px;
    border-left: 3px solid var(--accent);
    background: var(--bg);
    font-size: 13px;
  }
  .diagnostic-note {
    margin: 10px 0 0;
    color: var(--text-muted);
    font-size: 12px;
    line-height: 1.5;
  }
  .diagnostic-details {
    display: grid;
    gap: 3px;
    margin-top: 8px;
    color: var(--text-muted);
    font-size: 12px;
  }
  .timestamp {
    padding: 0;
    color: var(--accent);
    background: transparent;
    border: 0;
    font: inherit;
    font-variant-numeric: tabular-nums;
    cursor: pointer;
  }
  .edited-badge {
    color: var(--accent);
    font-size: 11px;
  }
  @media (max-width: 600px) {
    .section-heading {
      flex-direction: column;
    }
  }
</style>
