<script lang="ts">
  import type {
    MeetingExportFormat,
    MeetingSegment,
    MeetingSpeaker,
    MeetingTranscript,
  } from "./meeting-types";

  interface Props {
    transcript: MeetingTranscript;
    busy?: boolean;
    error?: string | null;
    onRename: (id: string, label: string) => Promise<void>;
    onMerge: (from: string, into: string) => Promise<void>;
    onExport: (format: MeetingExportFormat) => Promise<void>;
  }

  let {
    transcript,
    busy = false,
    error = null,
    onRename,
    onMerge,
    onExport,
  }: Props = $props();

  const exportFormats: Array<{ format: MeetingExportFormat; label: string }> = [
    { format: "plain", label: "Plain text" },
    { format: "markdown", label: "Markdown" },
    { format: "json", label: "JSON" },
    { format: "srt", label: "SRT subtitles" },
    { format: "vtt", label: "WebVTT subtitles" },
  ];

  let labelDrafts: Record<string, string> = $state({});
  let mergeTargets: Record<string, string> = $state({});
  let renamingId: string | null = $state(null);
  let mergingId: string | null = $state(null);
  let exportingFormat: MeetingExportFormat | null = $state(null);
  let actionError: string = $state("");
  let draftSourceSha: string | null = $state(null);

  const speakerGroups = $derived.by(() => {
    const groups = new Map<string, { speaker: MeetingSpeaker | undefined; segments: MeetingSegment[] }>();
    for (const segment of transcript.segments) {
      const group = groups.get(segment.speaker) ?? {
        speaker: transcript.speakers.find((speaker) => speaker.id === segment.speaker),
        segments: [],
      };
      group.segments.push(segment);
      groups.set(segment.speaker, group);
    }
    return Array.from(groups.values());
  });

  $effect(() => {
    if (draftSourceSha !== transcript.source_sha256) {
      draftSourceSha = transcript.source_sha256;
      labelDrafts = {};
      mergeTargets = {};
    }
    for (const speaker of transcript.speakers) {
      if (labelDrafts[speaker.id] === undefined) labelDrafts[speaker.id] = speaker.label;
    }
  });

  function displayLabel(id: string): string {
    return transcript.speakers.find((speaker) => speaker.id === id)?.label ?? id;
  }

  function formatTimestamp(seconds: number): string {
    const wholeSeconds = Math.max(0, Math.floor(seconds));
    const minutes = Math.floor(wholeSeconds / 60);
    const remainder = String(wholeSeconds % 60).padStart(2, "0");
    return minutes + ":" + remainder;
  }

  function errorText(value: unknown): string {
    return value instanceof Error ? value.message : "The requested meeting action failed.";
  }

  async function renameSpeaker(speaker: MeetingSpeaker): Promise<void> {
    const label = (labelDrafts[speaker.id] ?? "").trim();
    if (!label || label === speaker.label || busy || renamingId !== null) return;
    actionError = "";
    renamingId = speaker.id;
    try {
      await onRename(speaker.id, label);
    } catch (renameError) {
      actionError = errorText(renameError);
    } finally {
      renamingId = null;
    }
  }

  async function mergeSpeaker(from: string): Promise<void> {
    const into = mergeTargets[from];
    if (!into || into === from || busy || mergingId !== null) return;
    actionError = "";
    mergingId = from;
    try {
      await onMerge(from, into);
    } catch (mergeError) {
      actionError = errorText(mergeError);
    } finally {
      mergingId = null;
    }
  }

  async function exportMeeting(format: MeetingExportFormat): Promise<void> {
    if (busy || exportingFormat !== null) return;
    actionError = "";
    exportingFormat = format;
    try {
      await onExport(format);
    } catch (exportError) {
      actionError = errorText(exportError);
    } finally {
      exportingFormat = null;
    }
  }
</script>

<section class="meeting-review" aria-labelledby="meeting-review-title">
  <header class="review-header">
    <div>
      <p class="eyebrow">Meeting review</p>
      <h1 id="meeting-review-title">Transcript</h1>
      <p class="metadata">
        {transcript.language.toUpperCase()} · {transcript.model} · {formatTimestamp(transcript.duration_seconds)}
      </p>
    </div>
    <div class="privacy-note">
      Review stays local. Nothing is exported or retained by this view unless you explicitly choose an export.
    </div>
  </header>

  {#if error || actionError}
    <div class="error" role="alert">
      <strong>Could not update this review.</strong>
      <span>{error ?? actionError}</span>
      <small>Try again. Your transcript and unsaved edits remain visible here.</small>
    </div>
  {/if}

  <section class="speaker-panel" aria-labelledby="speaker-panel-title">
    <div class="section-heading">
      <div>
        <h2 id="speaker-panel-title">Speakers</h2>
        <p>Rename a speaker or merge two speaker IDs when they belong to the same person.</p>
      </div>
    </div>
    {#if transcript.speakers.length === 0}
      <p class="empty">No speaker labels are available for this transcript.</p>
    {:else}
      <div class="speaker-list">
        {#each transcript.speakers as speaker, index (speaker.id)}
          <div class="speaker-row">
            <div class="speaker-identity">
              <span class="speaker-dot" aria-hidden="true">{index + 1}</span>
              <div>
                <label for={"speaker-name-" + index}>Speaker {index + 1} name</label>
                <input
                  id={"speaker-name-" + index}
                  type="text"
                  value={labelDrafts[speaker.id] ?? speaker.label}
                  aria-label={"Rename " + speaker.label}
                  disabled={busy || renamingId !== null}
                  oninput={(event) => (labelDrafts[speaker.id] = (event.currentTarget as HTMLInputElement).value)}
                  onkeydown={(event) => {
                    if (event.key === "Enter") void renameSpeaker(speaker);
                  }}
                />
              </div>
            </div>
            <div class="speaker-actions">
              <button
                type="button"
                class="secondary"
                disabled={busy || renamingId !== null || !(labelDrafts[speaker.id] ?? "").trim() || (labelDrafts[speaker.id] ?? "").trim() === speaker.label}
                onclick={() => void renameSpeaker(speaker)}
              >
                {renamingId === speaker.id ? "Saving…" : "Rename"}
              </button>
              {#if transcript.speakers.length > 1}
                <label class="merge-control">
                  <span>Merge into</span>
                  <select
                    aria-label={"Merge " + speaker.label + " into"}
                    value={mergeTargets[speaker.id] ?? ""}
                    disabled={busy || mergingId !== null}
                    onchange={(event) => (mergeTargets[speaker.id] = (event.currentTarget as HTMLSelectElement).value)}
                  >
                    <option value="">Choose speaker</option>
                    {#each transcript.speakers as target (target.id)}
                      {#if target.id !== speaker.id}
                        <option value={target.id}>{target.label}</option>
                      {/if}
                    {/each}
                  </select>
                </label>
                <button
                  type="button"
                  class="secondary"
                  disabled={busy || mergingId !== null || !mergeTargets[speaker.id]}
                  onclick={() => void mergeSpeaker(speaker.id)}
                >
                  {mergingId === speaker.id ? "Merging…" : "Merge"}
                </button>
              {/if}
            </div>
          </div>
        {/each}
      </div>
    {/if}
  </section>

  <section class="transcript-panel" aria-labelledby="transcript-panel-title">
    <div class="section-heading">
      <div>
        <h2 id="transcript-panel-title">Conversation</h2>
        <p>{transcript.segments.length} segment{transcript.segments.length === 1 ? "" : "s"}, grouped by speaker and ordered by time.</p>
      </div>
    </div>
    {#if speakerGroups.length === 0}
      <p class="empty">No transcript segments yet.</p>
    {:else}
      <div class="conversation">
        {#each speakerGroups as group (group.speaker?.id ?? group.segments[0].speaker)}
          <article class="speaker-group">
            <h3>{group.speaker?.label ?? displayLabel(group.segments[0].speaker)}</h3>
            {#each group.segments as segment (segment.id)}
              <div class="segment">
                <time datetime={"PT" + segment.start + "S"}>{formatTimestamp(segment.start)}</time>
                <p>{segment.text}</p>
              </div>
            {/each}
          </article>
        {/each}
      </div>
    {/if}
  </section>

  <section class="export-panel" aria-labelledby="export-panel-title">
    <div class="section-heading">
      <div>
        <h2 id="export-panel-title">Export</h2>
        <p>Choose a format explicitly. Export uses the current transcript and speaker labels.</p>
      </div>
    </div>
    <div class="export-actions">
      {#each exportFormats as item (item.format)}
        <button
          type="button"
          class="secondary export-button"
          disabled={busy || exportingFormat !== null}
          onclick={() => void exportMeeting(item.format)}
        >
          {exportingFormat === item.format ? "Exporting…" : item.label}
        </button>
      {/each}
    </div>
  </section>
</section>

<style>
  .meeting-review {
    width: min(100%, 900px);
    margin: 0 auto;
    padding: 24px clamp(16px, 4vw, 40px) 48px;
    overflow-wrap: anywhere;
  }

  .review-header,
  .section-heading,
  .speaker-row,
  .segment {
    display: flex;
    gap: 16px;
  }

  .review-header,
  .section-heading {
    align-items: flex-start;
    justify-content: space-between;
  }

  .review-header {
    gap: 24px;
    padding-bottom: 24px;
    border-bottom: 1px solid var(--border);
  }

  .eyebrow {
    color: var(--accent);
    font-size: 11px;
    font-weight: 700;
    letter-spacing: 0.08em;
    text-transform: uppercase;
  }

  h1 {
    margin-top: 2px;
    font-size: 24px;
  }

  h2 {
    font-size: 16px;
  }

  h3 {
    color: var(--text);
    font-size: 14px;
  }

  .metadata,
  .section-heading p,
  .empty,
  label,
  .merge-control span {
    color: var(--text-muted);
    font-size: 12px;
  }

  .privacy-note {
    max-width: 300px;
    color: var(--text-muted);
    font-size: 12px;
    text-align: right;
  }

  .error {
    display: grid;
    gap: 4px;
    margin-top: 16px;
    padding: 12px 14px;
    color: var(--text);
    background: rgba(255, 107, 107, 0.1);
    border: 1px solid var(--danger);
    border-radius: var(--radius);
  }

  .error span,
  .error small {
    color: #ffb0b0;
  }

  .speaker-panel,
  .transcript-panel,
  .export-panel {
    margin-top: 24px;
  }

  .section-heading {
    margin-bottom: 12px;
  }

  .section-heading p {
    margin-top: 3px;
  }

  .speaker-list,
  .conversation {
    display: grid;
    gap: 8px;
  }

  .speaker-row {
    align-items: center;
    justify-content: space-between;
    padding: 12px;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }

  .speaker-identity {
    display: flex;
    align-items: center;
    gap: 10px;
    min-width: 0;
  }

  .speaker-identity > div {
    min-width: 0;
  }

  .speaker-dot {
    display: grid;
    flex: 0 0 28px;
    place-items: center;
    width: 28px;
    height: 28px;
    color: var(--bg);
    background: var(--accent);
    border-radius: 50%;
    font-size: 12px;
    font-weight: 700;
  }

  .speaker-identity input {
    width: min(220px, 100%);
    margin-top: 3px;
  }

  .speaker-actions {
    display: flex;
    align-items: end;
    justify-content: flex-end;
    gap: 8px;
    flex-wrap: wrap;
  }

  .speaker-actions button {
    white-space: nowrap;
  }

  .merge-control {
    display: grid;
    gap: 3px;
    min-width: 140px;
  }

  .merge-control select {
    min-width: 0;
  }

  .speaker-group {
    padding: 14px 0;
    border-bottom: 1px solid var(--border);
  }

  .speaker-group h3 {
    margin-bottom: 4px;
  }

  .segment {
    align-items: flex-start;
    padding: 7px 0;
  }

  .segment time {
    flex: 0 0 48px;
    color: var(--accent);
    font-variant-numeric: tabular-nums;
    font-size: 12px;
  }

  .segment p {
    min-width: 0;
    color: var(--text);
    white-space: pre-wrap;
  }

  .export-actions {
    display: grid;
    grid-template-columns: repeat(5, minmax(0, 1fr));
    gap: 8px;
  }

  .export-button {
    min-height: 40px;
  }

  @media (max-width: 600px) {
    .meeting-review {
      padding: 18px 14px 36px;
    }

    .review-header,
    .section-heading,
    .speaker-row {
      flex-direction: column;
    }

    .privacy-note {
      max-width: none;
      text-align: left;
    }

    .speaker-actions {
      display: grid;
      grid-template-columns: minmax(0, 1fr);
      align-items: stretch;
      justify-content: stretch;
      width: 100%;
    }

    .speaker-actions > button,
    .merge-control {
      width: 100%;
      min-width: 0;
    }

    .export-actions {
      grid-template-columns: repeat(2, minmax(0, 1fr));
    }
  }
</style>
