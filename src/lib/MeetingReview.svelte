<script lang="ts">
  import { onDestroy, untrack } from "svelte";
  import { toErrorMessage } from "./errors";
  import { createMeetingReviewDraftSnapshot, restoreMeetingDraftSnapshot, type MeetingReviewDraftSnapshot } from "./meeting-review-drafts";
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
  import AcousticPanel from "./meeting-review/AcousticPanel.svelte";
  import ExportPanel from "./meeting-review/ExportPanel.svelte";
  import SpeakerPanel from "./meeting-review/SpeakerPanel.svelte";

  interface Props {
    idPrefix?: string;
    active?: boolean;
    review: MeetingReview;
    transcript: MeetingTranscript;
    busy?: boolean;
    error?: string | null;
    onApply: (operations: CorrectionOperation[]) => Promise<void>;
    onUndo: () => Promise<void>;
    onReset: () => Promise<void>;
    onSave: () => Promise<boolean>;
    onExport: (format: MeetingExportFormat) => Promise<boolean>;
    onAttachAudio: () => Promise<MeetingAudioAttachment | null>;
    onDetachAudio: (token: string) => Promise<void>;
    onDraftDirtyChange?: (dirty: boolean) => void;
    /** `undefined` means the parent has not finished loading recovery state; `null` means none exists. */
    initialDraftSnapshot?: MeetingReviewDraftSnapshot | null;
    onDraftSnapshotChange?: (snapshot: MeetingReviewDraftSnapshot) => void;
    resetDraftKey?: number;
  }

  let {
    idPrefix = "",
    active = true,
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
    initialDraftSnapshot,
    onDraftSnapshotChange = () => undefined,
    resetDraftKey = 0,
  }: Props = $props();

  let labelDrafts: Record<string, string> = $state({});
  let mergeTargets: Record<string, string> = $state({});
  let textDrafts: Record<string, string> = $state({});
  let speakerDrafts: Record<string, string> = $state({});
  let draftRevision: string | null = $state(null);
  let committedTranscript: MeetingTranscript | null = $state(null);
  let committedResetKey: number | null = $state(null);
  let appliedInitialSnapshotKey: string | null = $state(null);
  let draftsReady = $state(false);
  let discardDraftsOnNextRevision = $state(false);
  let pendingAction: string | null = $state(null);
  let actionError: string = $state("");
  let actionNotice: string = $state("");
  let audioEl: HTMLAudioElement | undefined = $state();
  let audioToken: string | null = $state(null);
  let audioUrl: string | null = $state(null);
  let audioMime: string | null = $state(null);
  let audioSourceSha: string | null = $state(null);
  let audioError: string = $state("");
  let playbackTime = $state(0);
  let followPlayback = $state(false);
  let disposed = false;

  const sortedSegments = $derived([...transcript.segments].sort((a, b) => a.start - b.start || a.end - b.end || a.id.localeCompare(b.id)));
  const acousticReport = $derived(transcript.diarization ?? null);
  const activeSegments = $derived(audioUrl ? activeSegmentsAtTime(transcript.segments, playbackTime) : []);
  const activeIds = $derived(new Set(activeSegments.map((segment) => segment.id)));

  $effect(() => {
    const initialSnapshotKey = initialDraftSnapshot
      ? `${initialDraftSnapshot.source_sha256}\u0000${initialDraftSnapshot.review_revision}`
      : null;
    if (initialSnapshotKey === null) appliedInitialSnapshotKey = null;
    const initialSnapshotMatches =
      initialDraftSnapshot != null &&
      initialDraftSnapshot.source_sha256 === review.original.source_sha256 &&
      initialDraftSnapshot.source_sha256 === transcript.source_sha256 &&
      initialDraftSnapshot.review_revision === review.revision;
    const shouldHydrateInitialSnapshot = initialSnapshotMatches && initialSnapshotKey !== appliedInitialSnapshotKey;
    if (
      draftRevision === review.revision &&
      committedTranscript !== null &&
      committedResetKey === resetDraftKey &&
      !shouldHydrateInitialSnapshot
    )
      return;
    draftsReady = false;
    const previousTranscript = untrack(() => committedTranscript);
    const previousDrafts: MeetingDraftState = untrack(() => ({
      labels: labelDrafts,
      mergeTargets,
      texts: textDrafts,
      speakers: speakerDrafts,
    }));
    const previousResetKey = untrack(() => committedResetKey);
    const shouldReset =
      previousTranscript === null ||
      previousResetKey !== resetDraftKey ||
      discardDraftsOnNextRevision ||
      previousTranscript.source_sha256 !== transcript.source_sha256 ||
      previousTranscript.language !== transcript.language ||
      previousTranscript.model !== transcript.model;
    let nextDrafts = reconcileMeetingDrafts(
      previousTranscript,
      transcript,
      previousDrafts,
      review.batches.at(-1)?.operations ?? [],
      shouldReset,
    );
    if (shouldHydrateInitialSnapshot) {
      nextDrafts = restoreMeetingDraftSnapshot(
        nextDrafts,
        initialDraftSnapshot,
        transcript,
        review.original.source_sha256,
        review.revision,
      );
      appliedInitialSnapshotKey = initialSnapshotKey;
    }
    draftRevision = review.revision;
    committedTranscript = transcript;
    committedResetKey = resetDraftKey;
    labelDrafts = nextDrafts.labels;
    mergeTargets = nextDrafts.mergeTargets;
    textDrafts = nextDrafts.texts;
    speakerDrafts = nextDrafts.speakers;
    discardDraftsOnNextRevision = false;
    draftsReady = true;
  });

  $effect(() => {
    const currentSourceSha = review.original.source_sha256;
    const currentTranscript = committedTranscript;
    if (
      !draftsReady ||
      draftRevision !== review.revision ||
      currentTranscript === null ||
      currentTranscript.source_sha256 !== transcript.source_sha256 ||
      currentTranscript.language !== transcript.language ||
      currentTranscript.model !== transcript.model
    )
      return;
    const snapshot = createMeetingReviewDraftSnapshot(currentSourceSha, review.revision, {
      labels: labelDrafts,
      mergeTargets,
      texts: textDrafts,
      speakers: speakerDrafts,
    });
    untrack(() => onDraftSnapshotChange(snapshot));
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
    if (!active || !followPlayback || activeSegments.length === 0 || typeof document === "undefined") return;
    const first = document.getElementById(idPrefix + "meeting-segment-" + activeSegments[0].id);
    first?.scrollIntoView({ block: "nearest" });
  });

  $effect(() => {
    if (!active && audioEl) audioEl.pause();
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

  function disabled(): boolean {
    return busy || pendingAction !== null;
  }

  async function runAction(name: string, action: () => Promise<boolean | void>): Promise<boolean> {
    if (disabled()) return false;
    actionError = "";
    actionNotice = "";
    pendingAction = name;
    try {
      return (await action()) !== false;
    } catch (actionFailure) {
      actionError = toErrorMessage(actionFailure, "The requested meeting action failed.");
      return false;
    } finally {
      pendingAction = null;
    }
  }

  async function renameSpeaker(speaker: MeetingSpeaker): Promise<void> {
    const label = (labelDrafts[speaker.id] ?? "").trim();
    if (!label || label === speaker.label) return;
    await runAction("rename-" + speaker.id, () =>
      onApply([
        {
          kind: "rename_speaker",
          speaker_id: speaker.id,
          label,
        },
      ]),
    );
  }

  async function mergeSpeaker(from: string): Promise<void> {
    const into = mergeTargets[from];
    if (!into || into === from) return;
    await runAction("merge-" + from, () =>
      onApply([
        {
          kind: "merge_speakers",
          from_id: from,
          into_id: into,
        },
      ]),
    );
  }

  function discardMerge(speakerId: string): void {
    mergeTargets[speakerId] = "";
  }

  function setLabelDraft(speakerId: string, label: string): void {
    labelDrafts[speakerId] = label;
  }

  function setMergeTarget(speakerId: string, target: string): void {
    mergeTargets[speakerId] = target;
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

  const hasUnsavedDrafts = $derived.by(() => {
    if (Object.values(mergeTargets).some(Boolean)) return true;
    if (transcript.speakers.some((speaker) => (labelDrafts[speaker.id] ?? speaker.label) !== speaker.label)) return true;
    return transcript.segments.some(
      (segment) =>
        (textDrafts[segment.id] ?? segment.text) !== segment.text || (speakerDrafts[segment.id] ?? segment.speaker) !== segment.speaker,
    );
  });

  $effect(() => {
    onDraftDirtyChange(hasUnsavedDrafts);
  });

  function persistenceDisabled(): boolean {
    return disabled() || hasUnsavedDrafts;
  }

  function handleKeyboardScroll(event: KeyboardEvent): void {
    if (!active) return;
    if (["ArrowDown", "ArrowUp", "PageDown", "PageUp", "Home", "End", " "].includes(event.key)) disableFollow();
  }

  async function resetReview(): Promise<void> {
    if (!window.confirm("Reset all meeting edits to the original machine transcript?")) return;
    discardDraftsOnNextRevision = true;
    const succeeded = await runAction("reset", onReset);
    if (!succeeded) discardDraftsOnNextRevision = false;
  }

  async function undoReview(): Promise<void> {
    if (hasUnsavedDrafts && !window.confirm("Undo the last correction and discard unsaved edits?")) return;
    discardDraftsOnNextRevision = true;
    const succeeded = await runAction("undo", onUndo);
    if (!succeeded) discardDraftsOnNextRevision = false;
  }

  async function saveReview(): Promise<void> {
    if (await runAction("save", onSave)) actionNotice = "Review saved.";
  }

  async function exportReview(format: MeetingExportFormat): Promise<void> {
    if (await runAction("export-" + format, () => onExport(format))) actionNotice = "Export saved.";
  }
</script>

<svelte:window onkeydown={handleKeyboardScroll} />

<section class="meeting-review" aria-labelledby={idPrefix + "meeting-review-title"}>
  <header class="review-header">
    <div>
      <p class="eyebrow">Meeting review</p>
      <h1 id={idPrefix + "meeting-review-title"}>Transcript</h1>
      <p class="metadata">
        {transcript.language.toUpperCase()} · {transcript.model} · {formatTimestamp(transcript.duration_seconds)}
      </p>
    </div>
    <div class="privacy-note">Review stays local. Files are written only when you choose Save or Export.</div>
  </header>

  {#if error || actionError}
    <div class="error" role="alert">
      <strong>Meeting notice</strong>
      <span>{error ?? actionError}</span>
      <small>Try again. Your transcript and unsaved edits remain visible here.</small>
    </div>
  {/if}
  {#if actionNotice}<p class="success-note" role="status">{actionNotice}</p>{/if}

  {#if acousticReport}
    <AcousticPanel {idPrefix} {acousticReport} {displayLabel} {formatTimestamp} />
  {/if}

  <section class="review-actions" aria-label="Review actions">
    <button type="button" class="primary" disabled={persistenceDisabled()} onclick={() => void saveReview()}>
      {pendingAction === "save" ? "Saving…" : "Save review"}
    </button>
    <button type="button" class="secondary" disabled={disabled() || review.batches.length === 0} onclick={() => void undoReview()}>
      {pendingAction === "undo" ? "Undoing…" : "Undo"}
    </button>
    <button type="button" class="secondary" disabled={disabled() || review.batches.length === 0} onclick={() => void resetReview()}>
      {pendingAction === "reset" ? "Resetting…" : "Reset"}
    </button>
    <span class="revision-note"
      >Revision {review.generation} · {review.batches.length} correction batch{review.batches.length === 1 ? "" : "es"}</span
    >
  </section>
  {#if hasUnsavedDrafts}
    <p class="draft-note" role="status">
      Unapplied edits are not included in saves/exports. Apply or discard them before saving or exporting.
    </p>
  {/if}

  <SpeakerPanel
    {idPrefix}
    {transcript}
    {labelDrafts}
    {mergeTargets}
    {disabled}
    {renameSpeaker}
    {mergeSpeaker}
    {discardMerge}
    {setLabelDraft}
    {setMergeTarget}
  />

  <section class="audio-panel" aria-labelledby={idPrefix + "audio-panel-title"}>
    <div class="section-heading">
      <div>
        <h2 id={idPrefix + "audio-panel-title"}>Audio playback</h2>
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
    <p class="audio-lifecycle-note">Audio pauses when you switch tabs. Audio access ends when you quit Sagascript.</p>
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

  <section class="transcript-panel" aria-labelledby={idPrefix + "transcript-panel-title"}>
    <div class="section-heading">
      <div>
        <h2 id={idPrefix + "transcript-panel-title"}>Conversation</h2>
        <p>{transcript.segments.length} segment{transcript.segments.length === 1 ? "" : "s"}, ordered by time.</p>
      </div>
      {#if activeSegments.length > 0}<span class="active-note"
          >Current timestamp: {activeSegments.map((segment) => displayLabel(segment.speaker)).join(", ")}</span
        >{/if}
    </div>
    {#if sortedSegments.length === 0}
      <p class="empty">No transcript segments yet.</p>
    {:else}
      <div class="conversation" role="region" onwheel={disableFollow} ontouchmove={disableFollow} aria-label="Transcript segments">
        {#each sortedSegments as segment (segment.id)}
          <article id={idPrefix + "meeting-segment-" + segment.id} class:active-segment={activeIds.has(segment.id)} class="segment-card">
            <div class="segment-heading">
              <button
                type="button"
                class="timestamp"
                onclick={() => seekTo(segment.start)}
                disabled={!audioUrl}
                aria-label={"Seek to " + formatTimestamp(segment.start)}
              >
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
              oninput={(event) => (textDrafts[segment.id] = (event.currentTarget as HTMLTextAreaElement).value)}></textarea>
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

  <ExportPanel {idPrefix} {acousticReport} {pendingAction} {persistenceDisabled} {exportReview} />
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
  .segment-heading,
  .segment-controls,
  .review-actions,
  .playback-options {
    display: flex;
    gap: 12px;
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
  .metadata,
  .section-heading p,
  .empty,
  label,
  .revision-note {
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
  .review-actions,
  .audio-panel,
  .transcript-panel {
    margin-top: 24px;
  }
  .review-actions {
    align-items: center;
    flex-wrap: wrap;
  }
  .draft-note {
    margin-top: 8px;
    color: var(--text-muted);
    font-size: 12px;
  }
  .section-heading {
    margin-bottom: 12px;
  }
  .section-heading p {
    margin-top: 3px;
  }
  .conversation {
    display: grid;
    gap: 8px;
  }
  .audio-actions {
    display: flex;
    gap: 8px;
    flex-wrap: wrap;
  }
  audio {
    width: 100%;
  }
  .playback-options {
    align-items: center;
    justify-content: space-between;
    margin-top: 8px;
  }
  .playback-options label {
    color: var(--text);
  }
  .active-note {
    color: var(--accent);
    font-size: 12px;
  }
  .segment-card {
    padding: 14px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg-secondary);
  }
  .segment-card.active-segment {
    border-color: var(--accent);
    box-shadow: 0 0 0 1px var(--accent);
  }
  .segment-heading {
    align-items: center;
    flex-wrap: wrap;
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
  .timestamp:disabled {
    cursor: default;
    opacity: 0.7;
  }
  .edited-badge {
    color: var(--accent);
    font-size: 11px;
  }
  .segment-card textarea {
    box-sizing: border-box;
    width: 100%;
    min-height: 54px;
    margin-top: 10px;
    padding: 8px 10px;
    resize: vertical;
    background: var(--bg-secondary);
    color: var(--text);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    font: inherit;
    line-height: 1.5;
  }
  .segment-card textarea:focus {
    border-color: var(--accent);
  }
  .segment-card textarea:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }
  .segment-controls {
    align-items: end;
    flex-wrap: wrap;
    margin-top: 8px;
  }
  .segment-controls label {
    display: grid;
    gap: 3px;
  }
  .segment-controls select {
    min-width: 150px;
  }
  @media (max-width: 600px) {
    .meeting-review {
      padding: 18px 14px 36px;
    }
    .review-header,
    .section-heading {
      flex-direction: column;
    }
    .privacy-note {
      max-width: none;
      text-align: left;
    }
  }
</style>
