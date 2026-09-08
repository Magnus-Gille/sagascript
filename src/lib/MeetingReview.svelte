<script lang="ts">
  import { onDestroy, untrack } from "svelte";
  import { convertFileSrc } from "@tauri-apps/api/core";
  import { reconcileMeetingDrafts } from "./meeting-types";
  import type {
    CorrectionOperation,
    MeetingAudioAttachment,
    MeetingDraftState,
    MeetingExportFormat,
    MeetingReview,
    MeetingSegment,
    MeetingSpeaker,
    MeetingTranscript,
  } from "./meeting-types";
  import { activeSegmentsAtTime } from "./meeting-playback";

  interface Props {
    review: MeetingReview;
    transcript: MeetingTranscript;
    busy?: boolean;
    error?: string | null;
    onApply: (operations: CorrectionOperation[]) => Promise<void>;
    onUndo: () => Promise<void>;
    onReset: () => Promise<void>;
    onSave: () => Promise<void>;
    onExport: (format: MeetingExportFormat) => Promise<void>;
    onAttachAudio: () => Promise<MeetingAudioAttachment | null>;
    onDetachAudio: (token: string) => Promise<void>;
    onDraftDirtyChange?: (dirty: boolean) => void;
    resetDraftKey?: number;
  }

  let {
    review,
    transcript,
    busy = false,
    error = null,
    onApply,
    onUndo,
    onReset,
    onSave,
    onExport,
    onAttachAudio,
    onDetachAudio,
    onDraftDirtyChange = () => undefined,
    resetDraftKey = 0,
  }: Props = $props();

  const exportFormats: Array<{ format: MeetingExportFormat; label: string }> = [
    { format: "plain", label: "Plain text" },
    { format: "markdown", label: "Markdown" },
    { format: "json", label: "Review JSON" },
    { format: "srt", label: "SRT subtitles" },
    { format: "vtt", label: "WebVTT subtitles" },
  ];

  let labelDrafts: Record<string, string> = $state({});
  let mergeTargets: Record<string, string> = $state({});
  let textDrafts: Record<string, string> = $state({});
  let speakerDrafts: Record<string, string> = $state({});
  let draftRevision: string | null = $state(null);
  let committedTranscript: MeetingTranscript | null = $state(null);
  let committedResetKey: number | null = $state(null);
  let discardDraftsOnNextRevision = $state(false);
  let pendingAction: string | null = $state(null);
  let actionError: string = $state("");
  let audioEl: HTMLAudioElement | undefined = $state();
  let audioToken: string | null = $state(null);
  let audioUrl: string | null = $state(null);
  let audioMime: string | null = $state(null);
  let audioSourceSha: string | null = $state(null);
  let audioError: string = $state("");
  let playbackTime = $state(0);
  let followPlayback = $state(false);
  let disposed = false;

  const sortedSegments = $derived(
    [...transcript.segments].sort((a, b) => a.start - b.start || a.end - b.end || a.id.localeCompare(b.id)),
  );
  const activeSegments = $derived(audioUrl ? activeSegmentsAtTime(transcript.segments, playbackTime) : []);
  const activeIds = $derived(new Set(activeSegments.map((segment) => segment.id)));

  $effect(() => {
    if (
      draftRevision === review.revision
      && committedTranscript !== null
      && committedResetKey === resetDraftKey
    ) return;
    const previousTranscript = untrack(() => committedTranscript);
    const previousDrafts: MeetingDraftState = untrack(() => ({
      labels: labelDrafts,
      mergeTargets,
      texts: textDrafts,
      speakers: speakerDrafts,
    }));
    const previousResetKey = untrack(() => committedResetKey);
    const shouldReset =
      previousTranscript === null
      || previousResetKey !== resetDraftKey
      || discardDraftsOnNextRevision
      || previousTranscript.source_sha256 !== transcript.source_sha256
      || previousTranscript.language !== transcript.language
      || previousTranscript.model !== transcript.model;
    const nextDrafts = reconcileMeetingDrafts(
      previousTranscript,
      transcript,
      previousDrafts,
      review.batches.at(-1)?.operations ?? [],
      shouldReset,
    );
    draftRevision = review.revision;
    committedTranscript = transcript;
    committedResetKey = resetDraftKey;
    labelDrafts = nextDrafts.labels;
    mergeTargets = nextDrafts.mergeTargets;
    textDrafts = nextDrafts.texts;
    speakerDrafts = nextDrafts.speakers;
    discardDraftsOnNextRevision = false;
  });

  $effect(() => {
    const sourceSha = review.original.source_sha256;
    if (audioSourceSha === null) {
      audioSourceSha = sourceSha;
    } else if (audioSourceSha !== sourceSha) {
      const staleToken = audioToken;
      audioSourceSha = sourceSha;
      audioToken = null;
      audioMime = null;
      audioUrl = null;
      audioError = "";
      playbackTime = 0;
      if (staleToken) void onDetachAudio(staleToken).catch(() => undefined);
    }
  });

  $effect(() => {
    if (!followPlayback || activeSegments.length === 0 || typeof document === "undefined") return;
    const first = document.getElementById("meeting-segment-" + activeSegments[0].id);
    first?.scrollIntoView({ block: "nearest" });
  });

  onDestroy(() => {
    disposed = true;
    onDraftDirtyChange(false);
    const staleToken = audioToken;
    if (staleToken) void onDetachAudio(staleToken).catch(() => undefined);
  });

  function displayLabel(id: string): string {
    return transcript.speakers.find((speaker) => speaker.id === id)?.label ?? id;
  }

  function originalSegment(id: string): MeetingSegment | undefined {
    return review.original.segments.find((segment) => segment.id === id);
  }

  function isEdited(segment: MeetingSegment): boolean {
    const original = originalSegment(segment.id);
    return !original || original.text !== segment.text || original.speaker !== segment.speaker;
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

  function disabled(): boolean {
    return busy || pendingAction !== null;
  }

  async function runAction(name: string, action: () => Promise<void>): Promise<boolean> {
    if (disabled()) return false;
    actionError = "";
    pendingAction = name;
    try {
      await action();
      return true;
    } catch (actionFailure) {
      actionError = errorText(actionFailure);
      return false;
    } finally {
      pendingAction = null;
    }
  }

  async function renameSpeaker(speaker: MeetingSpeaker): Promise<void> {
    const label = (labelDrafts[speaker.id] ?? "").trim();
    if (!label || label === speaker.label) return;
    await runAction("rename-" + speaker.id, () => onApply([{
      kind: "rename_speaker",
      speaker_id: speaker.id,
      label,
    }]));
  }

  async function mergeSpeaker(from: string): Promise<void> {
    const into = mergeTargets[from];
    if (!into || into === from) return;
    await runAction("merge-" + from, () => onApply([{
      kind: "merge_speakers",
      from_id: from,
      into_id: into,
    }]));
  }

  async function applySegment(segment: MeetingSegment): Promise<void> {
    const text = textDrafts[segment.id] ?? segment.text;
    const speaker = speakerDrafts[segment.id] ?? segment.speaker;
    const operation: CorrectionOperation = {
      kind: "edit_segment",
      segment_id: segment.id,
      ...(text !== segment.text ? { text } : {}),
      ...(speaker !== segment.speaker ? { speaker_id: speaker } : {}),
    };
    if (text === segment.text && speaker === segment.speaker) return;
    await runAction("edit-" + segment.id, () => onApply([operation]));
  }

  function discardSegment(segment: MeetingSegment): void {
    textDrafts[segment.id] = segment.text;
    speakerDrafts[segment.id] = segment.speaker;
  }

  async function attachAudio(): Promise<void> {
    await runAction("attach-audio", async () => {
      const attachment = await onAttachAudio();
      if (!attachment || disposed) {
        if (attachment && disposed) await onDetachAudio(attachment.token).catch(() => undefined);
        return;
      }
      const oldToken = audioToken;
      audioToken = attachment.token;
      audioMime = attachment.mime;
      audioUrl = convertFileSrc(attachment.token, "meeting-audio");
      audioError = "";
      playbackTime = 0;
      if (oldToken && oldToken !== attachment.token) await onDetachAudio(oldToken);
    });
  }

  async function detachAudio(): Promise<void> {
    const token = audioToken;
    if (!token) return;
    await runAction("detach-audio", async () => {
      await onDetachAudio(token);
      audioToken = null;
      audioMime = null;
      audioUrl = null;
      audioError = "";
      playbackTime = 0;
    });
  }

  function handleAudioError(): void {
    audioError = "This audio cannot be played. Corrections and exports remain available. Reattach the source or choose a supported codec.";
  }

  function seekTo(seconds: number): void {
    if (!audioEl || !audioUrl) return;
    audioEl.currentTime = Math.max(0, Math.min(seconds, transcript.duration_seconds));
    playbackTime = audioEl.currentTime;
  }

  function disableFollow(): void {
    followPlayback = false;
  }

  function hasUnsavedDrafts(): boolean {
    if (Object.values(mergeTargets).some(Boolean)) return true;
    for (const speaker of transcript.speakers) {
      if ((labelDrafts[speaker.id] ?? speaker.label) !== speaker.label) return true;
    }
    for (const segment of transcript.segments) {
      if ((textDrafts[segment.id] ?? segment.text) !== segment.text) return true;
      if ((speakerDrafts[segment.id] ?? segment.speaker) !== segment.speaker) return true;
    }
    return false;
  }

  $effect(() => {
    onDraftDirtyChange(hasUnsavedDrafts());
  });

  function persistenceDisabled(): boolean {
    return disabled() || hasUnsavedDrafts();
  }

  function handleKeyboardScroll(event: KeyboardEvent): void {
    if (["ArrowDown", "ArrowUp", "PageDown", "PageUp", "Home", "End", " "].includes(event.key)) disableFollow();
  }

  async function resetReview(): Promise<void> {
    if (!window.confirm("Reset all meeting edits to the original machine transcript?")) return;
    discardDraftsOnNextRevision = true;
    const succeeded = await runAction("reset", onReset);
    if (!succeeded) discardDraftsOnNextRevision = false;
  }

  async function undoReview(): Promise<void> {
    if (hasUnsavedDrafts() && !window.confirm("Undo the last correction and discard unsaved edits?")) return;
    discardDraftsOnNextRevision = true;
    const succeeded = await runAction("undo", onUndo);
    if (!succeeded) discardDraftsOnNextRevision = false;
  }
</script>

<svelte:window onkeydown={handleKeyboardScroll} />

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
      <strong>Meeting notice</strong>
      <span>{error ?? actionError}</span>
      <small>Try again. Your transcript and unsaved edits remain visible here.</small>
    </div>
  {/if}

  <section class="review-actions" aria-label="Review actions">
    <button type="button" class="primary" disabled={persistenceDisabled()} onclick={() => void runAction("save", onSave)}>
      {pendingAction === "save" ? "Saving…" : "Save review"}
    </button>
    <button type="button" class="secondary" disabled={disabled() || review.batches.length === 0} onclick={() => void undoReview()}>
      {pendingAction === "undo" ? "Undoing…" : "Undo"}
    </button>
    <button type="button" class="secondary" disabled={disabled() || review.batches.length === 0} onclick={() => void resetReview()}>
      {pendingAction === "reset" ? "Resetting…" : "Reset"}
    </button>
    <span class="revision-note">Revision {review.generation} · {review.batches.length} correction batch{review.batches.length === 1 ? "" : "es"}</span>
  </section>
  {#if hasUnsavedDrafts()}
    <p class="draft-note" role="status">Unapplied edits are not included in saves/exports. Apply or discard them before saving or exporting.</p>
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
                  disabled={disabled()}
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
                disabled={disabled() || !(labelDrafts[speaker.id] ?? "").trim() || (labelDrafts[speaker.id] ?? "").trim() === speaker.label}
                onclick={() => void renameSpeaker(speaker)}
              >
                Rename
              </button>
              {#if transcript.speakers.length > 1}
                <label class="merge-control">
                  <span>Merge into</span>
                  <select
                    aria-label={"Merge " + speaker.label + " into"}
                    value={mergeTargets[speaker.id] ?? ""}
                    disabled={disabled()}
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
                <button type="button" class="secondary" disabled={disabled() || !mergeTargets[speaker.id]} onclick={() => void mergeSpeaker(speaker.id)}>
                  Merge
                </button>
              {/if}
            </div>
          </div>
        {/each}
      </div>
    {/if}
  </section>

  <section class="audio-panel" aria-labelledby="audio-panel-title">
    <div class="section-heading">
      <div>
        <h2 id="audio-panel-title">Audio playback</h2>
        <p>Choose the original source explicitly. Editing text never starts playback.</p>
      </div>
      <div class="audio-actions">
        <button type="button" class="secondary" disabled={disabled()} onclick={() => void attachAudio()}>
          {pendingAction === "attach-audio" ? "Checking…" : audioUrl ? "Replace audio" : "Attach audio"}
        </button>
        {#if audioUrl}
          <button type="button" class="secondary" disabled={disabled()} onclick={() => void detachAudio()}>Detach</button>
        {/if}
      </div>
    </div>
    {#if audioUrl}
      <audio
        bind:this={audioEl}
        controls
        preload="metadata"
        src={audioUrl}
        aria-label="Meeting audio"
        onerror={handleAudioError}
        ontimeupdate={(event) => (playbackTime = (event.currentTarget as HTMLAudioElement).currentTime)}
      ></audio>
      {#if audioError}<p class="error" role="alert">{audioError}</p>{/if}
      <div class="playback-options">
        <label><input type="checkbox" bind:checked={followPlayback} /> Follow playback</label>
        {#if audioMime}<span>{audioMime}</span>{/if}
      </div>
    {:else}
      <p class="empty">No audio is attached. Corrections and exports remain available.</p>
    {/if}
  </section>

  <section class="transcript-panel" aria-labelledby="transcript-panel-title">
    <div class="section-heading">
      <div>
        <h2 id="transcript-panel-title">Conversation</h2>
        <p>{transcript.segments.length} segment{transcript.segments.length === 1 ? "" : "s"}, ordered by time.</p>
      </div>
      {#if activeSegments.length > 0}<span class="active-note">Current timestamp: {activeSegments.map((segment) => displayLabel(segment.speaker)).join(", ")}</span>{/if}
    </div>
    {#if sortedSegments.length === 0}
      <p class="empty">No transcript segments yet.</p>
    {:else}
      <div class="conversation" role="region" onwheel={disableFollow} ontouchmove={disableFollow} aria-label="Transcript segments">
        {#each sortedSegments as segment (segment.id)}
          <article id={"meeting-segment-" + segment.id} class:active-segment={activeIds.has(segment.id)} class="segment-card">
            <div class="segment-heading">
              <button type="button" class="timestamp" onclick={() => seekTo(segment.start)} disabled={!audioUrl} aria-label={"Seek to " + formatTimestamp(segment.start)}>
                {formatTimestamp(segment.start)}
              </button>
              <strong>{displayLabel(segment.speaker)}</strong>
              {#if isEdited(segment)}<span class="edited-badge">Edited</span>{/if}
            </div>
            <textarea
              aria-label={"Edit transcript segment at " + formatTimestamp(segment.start)}
              rows="2"
              value={textDrafts[segment.id] ?? segment.text}
              disabled={disabled()}
              oninput={(event) => (textDrafts[segment.id] = (event.currentTarget as HTMLTextAreaElement).value)}
            ></textarea>
            <div class="segment-controls">
              <label>
                Speaker
                <select
                  aria-label={"Edit speaker for " + formatTimestamp(segment.start)}
                  value={speakerDrafts[segment.id] ?? segment.speaker}
                  disabled={disabled()}
                  onchange={(event) => (speakerDrafts[segment.id] = (event.currentTarget as HTMLSelectElement).value)}
                >
                  {#each transcript.speakers as speaker (speaker.id)}<option value={speaker.id}>{speaker.label}</option>{/each}
                </select>
              </label>
              <button type="button" class="secondary" disabled={disabled()} onclick={() => void applySegment(segment)}>Apply</button>
              <button type="button" class="secondary" disabled={disabled()} onclick={() => discardSegment(segment)}>Discard</button>
            </div>
          </article>
        {/each}
      </div>
    {/if}
  </section>

  <section class="export-panel" aria-labelledby="export-panel-title">
    <div class="section-heading">
      <div>
        <h2 id="export-panel-title">Export reviewed transcript</h2>
        <p>Choose a format explicitly. Each export uses the current reviewed text and speaker labels.</p>
      </div>
    </div>
    <div class="export-actions">
      {#each exportFormats as item (item.format)}
        <button type="button" class="secondary export-button" disabled={persistenceDisabled()} onclick={() => void runAction("export-" + item.format, () => onExport(item.format))}>
          {pendingAction === "export-" + item.format ? "Exporting…" : item.label}
        </button>
      {/each}
    </div>
  </section>
</section>

<style>
  .meeting-review { width: min(100%, 900px); margin: 0 auto; padding: 24px clamp(16px, 4vw, 40px) 48px; overflow-wrap: anywhere; }
  .review-header, .section-heading, .speaker-row, .segment-heading, .segment-controls, .review-actions, .playback-options { display: flex; gap: 12px; }
  .review-header, .section-heading { align-items: flex-start; justify-content: space-between; }
  .review-header { gap: 24px; padding-bottom: 24px; border-bottom: 1px solid var(--border); }
  .eyebrow { color: var(--accent); font-size: 11px; font-weight: 700; letter-spacing: 0.08em; text-transform: uppercase; }
  h1 { margin-top: 2px; font-size: 24px; } h2 { font-size: 16px; }
  .metadata, .section-heading p, .empty, label, .merge-control span, .revision-note { color: var(--text-muted); font-size: 12px; }
  .privacy-note { max-width: 300px; color: var(--text-muted); font-size: 12px; text-align: right; }
  .error { display: grid; gap: 4px; margin-top: 16px; padding: 12px 14px; color: var(--text); background: rgba(255, 107, 107, 0.1); border: 1px solid var(--danger); border-radius: var(--radius); }
  .error span, .error small { color: #ffb0b0; }
  .review-actions, .speaker-panel, .audio-panel, .transcript-panel, .export-panel { margin-top: 24px; }
  .review-actions { align-items: center; flex-wrap: wrap; }
  .draft-note { margin-top: 8px; color: var(--text-muted); font-size: 12px; }
  .section-heading { margin-bottom: 12px; } .section-heading p { margin-top: 3px; }
  .speaker-list, .conversation { display: grid; gap: 8px; }
  .speaker-row { align-items: center; justify-content: space-between; padding: 12px; background: var(--bg-secondary); border: 1px solid var(--border); border-radius: var(--radius); }
  .speaker-identity { display: flex; align-items: center; gap: 10px; min-width: 0; } .speaker-identity > div { min-width: 0; }
  .speaker-dot { display: grid; flex: 0 0 28px; place-items: center; width: 28px; height: 28px; color: var(--bg); background: var(--accent); border-radius: 50%; font-size: 12px; font-weight: 700; }
  .speaker-identity input { width: min(220px, 100%); margin-top: 3px; } .speaker-actions { display: flex; align-items: end; justify-content: flex-end; gap: 8px; flex-wrap: wrap; }
  .speaker-actions button { white-space: nowrap; } .merge-control { display: grid; gap: 3px; min-width: 140px; } .merge-control select { min-width: 0; }
  .audio-actions { display: flex; gap: 8px; flex-wrap: wrap; } audio { width: 100%; } .playback-options { align-items: center; justify-content: space-between; margin-top: 8px; } .playback-options label { color: var(--text); }
  .active-note { color: var(--accent); font-size: 12px; } .segment-card { padding: 14px; border: 1px solid var(--border); border-radius: var(--radius); background: var(--bg-secondary); } .segment-card.active-segment { border-color: var(--accent); box-shadow: 0 0 0 1px var(--accent); }
  .segment-heading { align-items: center; flex-wrap: wrap; } .timestamp { padding: 0; color: var(--accent); background: transparent; border: 0; font: inherit; font-variant-numeric: tabular-nums; cursor: pointer; } .timestamp:disabled { cursor: default; opacity: 0.7; }
  .edited-badge { color: var(--accent); font-size: 11px; } .segment-card textarea { box-sizing: border-box; width: 100%; min-height: 54px; margin-top: 10px; padding: 8px 10px; resize: vertical; background: var(--bg-secondary); color: var(--text); border: 1px solid var(--border); border-radius: var(--radius); font: inherit; line-height: 1.5; } .segment-card textarea:focus { border-color: var(--accent); } .segment-card textarea:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; } .segment-controls { align-items: end; flex-wrap: wrap; margin-top: 8px; } .segment-controls label { display: grid; gap: 3px; } .segment-controls select { min-width: 150px; }
  .export-actions { display: grid; grid-template-columns: repeat(5, minmax(0, 1fr)); gap: 8px; } .export-button { min-height: 40px; }
  @media (max-width: 600px) { .meeting-review { padding: 18px 14px 36px; } .review-header, .section-heading, .speaker-row { flex-direction: column; } .privacy-note { max-width: none; text-align: left; } .speaker-actions { display: grid; grid-template-columns: minmax(0, 1fr); align-items: stretch; width: 100%; } .speaker-actions > button, .merge-control { width: 100%; min-width: 0; } .export-actions { grid-template-columns: repeat(2, minmax(0, 1fr)); } }
</style>
