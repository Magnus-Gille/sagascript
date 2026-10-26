<script lang="ts">
  import type { LivePhase, LiveSegment, LiveSnapshot, LiveSource, SourceStatus } from "./live-meeting-types";
  import { deriveSourceStatus, sortLiveGaps, sortLiveSegments } from "./live-meeting-state";

  interface Props {
    snapshot: LiveSnapshot | null;
    busy?: boolean;
    supported?: boolean;
    onStart: () => Promise<void>;
    onStop: () => Promise<void>;
    onDiscard: () => Promise<void>;
    onSaveText: () => Promise<void>;
    onSaveAudio: () => Promise<void>;
    onRefresh: () => Promise<void>;
    error?: string | null;
  }

  let {
    snapshot,
    busy = false,
    supported = true,
    onStart,
    onStop,
    onDiscard,
    onSaveText,
    onSaveAudio,
    onRefresh,
    error = null,
  }: Props = $props();

  let pendingAction = $state<string | null>(null);
  let actionError = $state<string | null>(null);
  let discardConfirm = $state(false);
  let followLatest = $state(true);
  let transcriptElement = $state<HTMLElement | null>(null);
  let transcriptPage = $state(0);

  const TRANSCRIPT_PAGE_SIZE = 200;

  const phaseLabels: Record<LivePhase, string> = {
    starting: "Starting",
    running: "Listening",
    draining: "Finishing transcript",
    completed: "Completed",
    cancelled: "Cancelled",
    failed: "Failed",
  };

  const sortedSegments = $derived(sortLiveSegments((snapshot?.segments ?? []).filter((segment) => segment.text.trim().length > 0)));
  const transcriptPageCount = $derived(Math.max(1, Math.ceil(sortedSegments.length / TRANSCRIPT_PAGE_SIZE)));
  const activeTranscriptPage = $derived(followLatest ? transcriptPageCount - 1 : Math.min(transcriptPage, transcriptPageCount - 1));
  const displayedSegments = $derived(
    sortedSegments.slice(activeTranscriptPage * TRANSCRIPT_PAGE_SIZE, (activeTranscriptPage + 1) * TRANSCRIPT_PAGE_SIZE),
  );
  // Keep scrolling reactive to content changes, including a preview whose text
  // changes while the segment count stays the same.
  const transcriptSignature = $derived(
    displayedSegments.map((segment) => `${segment.id}:${segment.revision}:${segment.text}`).join("\u0000"),
  );

  const hasTranscriptPages = $derived(transcriptPageCount > 1);
  const terminal = $derived(snapshot?.phase === "completed" || snapshot?.phase === "cancelled" || snapshot?.phase === "failed");
  const recordingStopped = $derived(snapshot?.phase === "draining" || terminal);
  const hasSession = $derived(snapshot !== null);
  const canStart = $derived(!hasSession && !busy && pendingAction === null && supported);
  const canStop = $derived((snapshot?.phase === "running" || snapshot?.phase === "starting") && !busy && pendingAction === null);
  const exportable = $derived(snapshot?.phase === "completed" || snapshot?.phase === "failed");
  const canExport = $derived(exportable && !busy && pendingAction === null);
  const currentError = $derived(actionError ?? error ?? snapshot?.error ?? null);
  const displayedGaps = $derived(sortLiveGaps(snapshot?.gaps ?? []).slice(-20));
  const omittedGapCount = $derived(Math.max(0, (snapshot?.gaps.length ?? 0) - displayedGaps.length));

  $effect(() => {
    void transcriptSignature;
    if (followLatest && transcriptElement) transcriptElement.scrollTop = transcriptElement.scrollHeight;
  });

  $effect(() => {
    if (followLatest) transcriptPage = transcriptPageCount - 1;
    else if (transcriptPage >= transcriptPageCount) transcriptPage = transcriptPageCount - 1;
  });

  function sourceLabel(source: LiveSource): string {
    return source === "microphone" ? "Jag · mikrofon" : "Mötet · systemljud";
  }

  function formatDuration(milliseconds: number): string {
    const seconds = Math.max(0, Math.floor((Number.isFinite(milliseconds) ? milliseconds : 0) / 1000));
    return `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
  }

  function formatLag(milliseconds: number | null): string {
    if (milliseconds === null) return "No text yet";
    return `${(Math.max(0, milliseconds) / 1_000).toFixed(1)} s`;
  }

  function runAction(name: string, action: () => Promise<void>): void {
    if (busy || pendingAction !== null) return;
    actionError = null;
    pendingAction = name;
    void Promise.resolve()
      .then(action)
      .catch((failure: unknown) => {
        actionError = failure instanceof Error ? failure.message : String(failure);
      })
      .finally(() => {
        pendingAction = null;
      });
  }

  function start(): void {
    if (!canStart) return;
    runAction("start", onStart);
  }

  function stop(): void {
    if (!canStop) return;
    runAction("stop", onStop);
  }

  function refresh(): void {
    if (busy || pendingAction !== null) return;
    runAction("refresh", onRefresh);
  }

  function saveText(): void {
    if (!canExport) return;
    runAction("save-text", onSaveText);
  }

  function saveAudio(): void {
    if (!canExport) return;
    runAction("save-audio", onSaveAudio);
  }

  function requestDiscard(): void {
    if (!hasSession || busy || pendingAction !== null) return;
    discardConfirm = true;
  }

  function cancelDiscard(): void {
    discardConfirm = false;
  }

  function confirmDiscard(): void {
    if (!hasSession || busy || pendingAction !== null) return;
    discardConfirm = false;
    runAction("discard", onDiscard);
  }

  function channelStatus(source: SourceStatus) {
    const health = deriveSourceStatus(source);
    if (recordingStopped && !source.error) return { ...health, health: "silent" as const, label: "Stopped" };
    return health;
  }

  function segmentAriaLabel(segment: LiveSegment): string {
    return `${sourceLabel(segment.source)}, ${segment.status === "preview" ? "Preview" : "Final"}, ${formatDuration(segment.start_ms)}`;
  }

  function previousTranscriptPage(): void {
    followLatest = false;
    transcriptPage = Math.max(0, activeTranscriptPage - 1);
  }

  function nextTranscriptPage(): void {
    followLatest = false;
    transcriptPage = Math.min(transcriptPageCount - 1, activeTranscriptPage + 1);
  }

  function returnToLatest(): void {
    followLatest = true;
    transcriptPage = transcriptPageCount - 1;
  }
</script>

<section class="live-meeting" aria-label="Live meeting">
  <header class="header">
    <div>
      <h2>Live meeting</h2>
      <p class="temporary-note">Audio and text are temporary. Save what you want, then discard the temporary session before quitting.</p>
    </div>
    {#if snapshot}
      <span class:terminal class="phase" aria-label={`Live meeting state: ${phaseLabels[snapshot.phase]}`}>
        {phaseLabels[snapshot.phase]}
      </span>
    {/if}
  </header>

  {#if !supported}
    <p class="notice">Live meetings are unavailable on this device.</p>
  {/if}

  {#if currentError}
    <p class="error" role="alert">{currentError}</p>
  {/if}

  <div class="actions" aria-label="Live meeting actions">
    {#if !hasSession}
      <button class="primary" type="button" disabled={!canStart} onclick={start}>
        {pendingAction === "start" ? "Starting…" : "Start live meeting"}
      </button>
    {:else if snapshot?.phase === "running" || snapshot?.phase === "starting"}
      <button class="danger" type="button" disabled={!canStop} onclick={stop}>
        {pendingAction === "stop" ? "Stopping…" : snapshot.phase === "starting" ? "Cancel starting" : "Stop live meeting"}
      </button>
    {/if}
    {#if hasSession}
      <button class="secondary" type="button" disabled={busy || pendingAction !== null} onclick={refresh}>
        {pendingAction === "refresh" ? "Refreshing…" : "Refresh"}
      </button>
      {#if exportable}
        <button class="secondary" type="button" disabled={!canExport} onclick={saveText}>
          {pendingAction === "save-text" ? "Saving text…" : "Save text"}
        </button>
        <button class="secondary" type="button" disabled={!canExport || snapshot?.audio_available !== true} onclick={saveAudio}>
          {pendingAction === "save-audio" ? "Saving audio…" : "Save audio"}
        </button>
      {/if}
      {#if !discardConfirm}
        <button class="secondary" type="button" disabled={busy || pendingAction !== null} onclick={requestDiscard}>Discard</button>
      {:else}
        <span class="discard-confirm" role="group" aria-label="Confirm discard">
          <span>Discard temporary audio and text?</span>
          <button class="danger" type="button" disabled={busy || pendingAction !== null} onclick={confirmDiscard}>Yes, discard</button>
          <button class="secondary" type="button" disabled={busy || pendingAction !== null} onclick={cancelDiscard}>Keep</button>
        </span>
      {/if}
    {/if}
  </div>

  {#if snapshot}
    <div class="sources" aria-label="Channel health">
      {#each snapshot.sources as source (source.source)}
        {@const health = channelStatus(source)}
        <article class="source" class:source-error={health.health === "failed" || health.health === "disconnected"}>
          <div class="source-heading">
            <strong>{sourceLabel(source.source)}</strong>
            <span>{health.label}</span>
          </div>
          <div class="source-meta">
            <span>Text delay: {formatLag(health.lag_ms)}</span>
            <span>Queue: {formatLag(source.queued_ms)}</span>
            <span>Dropped: {source.dropped_frames}</span>
          </div>
          <meter
            min="0"
            max="1"
            value={recordingStopped ? 0 : Math.max(0, Math.min(1, source.signal_level))}
            aria-label={`${sourceLabel(source.source)} signal level`}
          ></meter>
        </article>
      {/each}
    </div>

    {#if snapshot.gaps.length > 0}
      <aside class="gap-banner" role="status" aria-label="Incomplete audio">
        <strong>Incomplete audio · {snapshot.gaps.length} gaps</strong>
        <p>Some audio was unavailable. These intervals remain marked in exports.</p>
        <details>
          <summary>Show {displayedGaps.length} latest gaps</summary>
          <ul>
            {#each displayedGaps as gap, index (`${gap.source}:${gap.start_ms}:${gap.end_ms}:${index}`)}
              <li>
                <strong>{sourceLabel(gap.source)}</strong>
                <span>{formatDuration(gap.start_ms)}–{formatDuration(gap.end_ms)} · {gap.reason}</span>
              </li>
            {/each}
          </ul>
          {#if omittedGapCount > 0}
            <p>{omittedGapCount} earlier gaps are omitted here; all gaps remain in exports.</p>
          {/if}
        </details>
      </aside>
    {/if}

    <div class="transcript-heading">
      <h3>Live transcript</h3>
      <label class="follow-toggle">
        <input type="checkbox" bind:checked={followLatest} />
        Follow latest
      </label>
      {#if !followLatest}
        <button class="link-button" type="button" onclick={returnToLatest}>Return to latest</button>
      {/if}
    </div>

    {#if hasTranscriptPages}
      <div class="pagination" aria-label="Transcript pages">
        <button class="link-button" type="button" disabled={activeTranscriptPage === 0} onclick={previousTranscriptPage}>Previous</button>
        <span>Page {activeTranscriptPage + 1} of {transcriptPageCount} · {sortedSegments.length} segments</span>
        <button class="link-button" type="button" disabled={activeTranscriptPage === transcriptPageCount - 1} onclick={nextTranscriptPage}
          >Next</button
        >
      </div>
    {/if}
    <div class="transcript" bind:this={transcriptElement} role="log" aria-live="polite" aria-label="Live transcript">
      {#if displayedSegments.length === 0}
        <p class="empty">Waiting for speech…</p>
      {:else}
        {#each displayedSegments as segment (segment.id)}
          <article class:preview={segment.status === "preview"} class="transcript-row" aria-label={segmentAriaLabel(segment)}>
            <div class="segment-meta">
              <span>{sourceLabel(segment.source)}</span>
              <span>{segment.status === "preview" ? "Preview" : "Final"} · {formatDuration(segment.start_ms)}</span>
            </div>
            <p>{segment.text}</p>
          </article>
        {/each}
      {/if}
    </div>
  {/if}
</section>

<style>
  .live-meeting {
    display: flex;
    flex-direction: column;
    gap: 12px;
    width: 100%;
    max-width: 720px;
    padding: 16px;
    color: var(--text, #f2f2ef);
    background: var(--bg-secondary, #1a1a1d);
    border: 1px solid var(--border, #303034);
    border-radius: var(--radius, 7px);
  }

  .header,
  .source-heading,
  .transcript-heading,
  .actions,
  .discard-confirm,
  .segment-meta,
  .source-meta {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .header,
  .transcript-heading,
  .source-heading {
    justify-content: space-between;
  }

  h2,
  h3,
  p {
    margin: 0;
  }

  h2 {
    font-size: 16px;
  }
  h3 {
    font-size: 13px;
  }

  .temporary-note,
  .notice,
  .empty,
  .source-meta,
  .segment-meta {
    color: var(--text-muted, #9a9a9f);
    font-size: 11px;
  }

  .temporary-note {
    margin-top: 2px;
  }
  .phase {
    color: var(--accent, #72e6cf);
    font-size: 12px;
  }
  .phase.terminal {
    color: var(--text-muted, #9a9a9f);
  }
  .actions {
    flex-wrap: wrap;
  }

  button {
    padding: 7px 11px;
  }
  .discard-confirm {
    flex-wrap: wrap;
    font-size: 11px;
    color: var(--text-muted, #9a9a9f);
  }
  .error {
    color: var(--danger, #ff6b6b);
    font-size: 12px;
  }
  .gap-banner {
    display: grid;
    gap: 5px;
    padding: 9px;
    border: 1px solid var(--danger, #ff6b6b);
    border-radius: var(--radius, 7px);
    color: var(--text, #f2f2ef);
    font-size: 12px;
  }
  .gap-banner p,
  .gap-banner summary,
  .gap-banner li {
    color: var(--text-muted, #9a9a9f);
    font-size: 11px;
  }
  .gap-banner details {
    margin-top: 2px;
  }
  .gap-banner summary {
    cursor: pointer;
  }
  .gap-banner ul {
    display: grid;
    gap: 4px;
    margin: 7px 0 0;
    padding-left: 18px;
  }
  .gap-banner li {
    display: flex;
    gap: 6px;
    flex-wrap: wrap;
  }
  .sources {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 8px;
  }
  .source {
    padding: 9px;
    border: 1px solid var(--border, #303034);
    border-radius: var(--radius, 7px);
  }
  .source.source-error {
    border-color: var(--danger, #ff6b6b);
  }
  .source-heading {
    font-size: 12px;
  }
  .source-heading span {
    color: var(--text-muted, #9a9a9f);
    font-size: 11px;
  }
  .source-meta {
    flex-wrap: wrap;
    margin-top: 5px;
  }
  meter {
    width: 100%;
    height: 5px;
    margin-top: 7px;
  }
  .follow-toggle {
    display: flex;
    align-items: center;
    gap: 5px;
    margin-left: auto;
    color: var(--text-muted, #9a9a9f);
    font-size: 11px;
  }
  .link-button {
    padding: 0;
    color: var(--accent, #72e6cf);
    background: transparent;
    font-size: 11px;
  }
  .pagination {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 8px;
    color: var(--text-muted, #9a9a9f);
    font-size: 11px;
  }
  .pagination .link-button:disabled {
    opacity: 0.45;
    cursor: default;
  }
  .transcript {
    max-height: 360px;
    min-height: 82px;
    overflow-y: auto;
    padding: 4px;
    border: 1px solid var(--border, #303034);
    border-radius: var(--radius, 7px);
  }
  .transcript-row {
    padding: 8px;
    border-bottom: 1px solid var(--border, #303034);
    user-select: text;
  }
  .transcript-row:last-child {
    border-bottom: 0;
  }
  .transcript-row.preview {
    background: rgba(114, 230, 207, 0.06);
  }
  .segment-meta {
    justify-content: space-between;
  }
  .transcript-row p {
    margin-top: 3px;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }

  @media (max-width: 520px) {
    .sources {
      grid-template-columns: 1fr;
    }
    .header {
      align-items: flex-start;
      flex-direction: column;
    }
  }
</style>
