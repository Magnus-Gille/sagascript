<script lang="ts">
  import { onMount } from "svelte";
  import { open } from "@tauri-apps/plugin-dialog";
  import { loadPlatform } from "./platform";
  import { useRecorderEvent } from "./record-events";
  import {
    acknowledgeRecordingConsent,
    confirmRecordingFolder,
    deleteRecording,
    getRecordingFolderInfo,
    getRecordingSession,
    listRecordings,
    readRecorderSettings,
    recoverRecordings,
    revealRecording,
    setRecordingDirectory,
    setRecordingQuality,
    startRecordingSession,
    stopRecordingSession,
    transcribeRecording,
  } from "./record-api";
  import { RECORDING_PLAYBACK_FALLBACK, recordingAudioUrl } from "./record-playback";
  import {
    canStart,
    canStop,
    describeRecordingError,
    endedEarlyText,
    fileManagerName,
    firstUseStep,
    formatBytes,
    formatDuration,
    formatElapsed,
    formatLevelDb,
    formatSizePerHour,
    formatStartedAt,
    gapText,
    initialRecorderState,
    isSessionActive,
    levelToPercent,
    noteworthyRecoveries,
    recoveryHeading,
    reduceRecorder,
    sessionNotices,
    toRecorderError,
    trashName,
    warningText,
    type RecorderAction,
    type RecorderError,
  } from "./record-state";
  import type { RecordingFolderInfo, RecordingQuality, RecordingSessionInfo, RecordingSummary, RecoveryNote } from "./record-types";

  interface Props {
    /** See RECORDING_PLAYBACK_FALLBACK; a prop so a test can exercise both. */
    playbackFallback?: boolean;
  }
  let { playbackFallback = RECORDING_PLAYBACK_FALLBACK }: Props = $props();

  const QUALITIES: { id: RecordingQuality; label: string }[] = [
    { id: "lossless", label: "Lossless" },
    { id: "compact", label: "Compact" },
  ];

  let recorder = $state(initialRecorderState);
  let quality = $state<RecordingQuality>("lossless");
  let folder = $state<RecordingFolderInfo | null>(null);
  let recordings = $state<RecordingSummary[]>([]);
  let recovery = $state<RecoveryNote[]>([]);
  let platform = $state("unknown");
  /** A failure of something other than the recording itself (settings, list, reveal, trash, …). */
  let actionError = $state<RecorderError | null>(null);
  let statusMessage = $state("");
  /** The action running per recording folder. */
  let busy = $state<Record<string, string>>({});
  /** Transcribed text per recording folder; "" when the backend returned none. */
  let transcripts = $state<Record<string, string>>({});
  let playing = $state<string | null>(null);
  let playbackFailed = $state<Record<string, boolean>>({});

  const phase = $derived(recorder.phase);
  const active = $derived(isSessionActive(phase));
  const step = $derived(firstUseStep(recorder.firstUse));
  const session = $derived(recorder.session);
  const live = $derived(active ? session : null);
  const percent = $derived(live ? levelToPercent(live.level) : 0);
  const notices = $derived(live ? sessionNotices(live) : []);
  const finished = $derived(recorder.finished);
  const shownRecordings = $derived(
    recordings.filter((recording) => recording.folder_name !== (phase === "finished" ? finished?.summary.folder_name : null)),
  );
  const startError = $derived(recorder.error ? describeRecordingError(recorder.error) : null);
  const actionErrorText = $derived(actionError ? describeRecordingError(actionError) : null);
  const recoveryNotes = $derived(noteworthyRecoveries(recovery));
  const recoveryTitle = $derived(recoveryHeading(recovery));
  const needsRecovery = $derived(recordings.some((recording) => recording.needs_recovery));
  const reveal = $derived(`Reveal in ${fileManagerName(platform)}`);
  const trash = $derived(`Move to ${trashName(platform)}`);

  function dispatch(action: RecorderAction) {
    recorder = reduceRecorder(recorder, action);
  }

  function fail(error: unknown) {
    actionError = toRecorderError(error);
  }

  async function refreshRecordings() {
    try {
      recordings = await listRecordings();
    } catch (error) {
      fail(error);
    }
  }

  async function refreshFolder() {
    try {
      folder = await getRecordingFolderInfo();
    } catch (error) {
      fail(error);
    }
  }

  let revision = 0;
  const sessionEvents = useRecorderEvent("recording-session-changed", (info: RecordingSessionInfo) => {
    revision++;
    const wasActive = isSessionActive(recorder.phase);
    dispatch({ type: "session", info });
    // A session that ended (here or elsewhere) has a new folder to list.
    if (wasActive && (info.state === "done" || info.state === "idle")) void refreshRecordings();
  });
  const recoveredEvents = useRecorderEvent("recordings-recovered", (notes: RecoveryNote[]) => {
    recovery = notes;
    void refreshRecordings();
  });

  onMount(() => {
    let disposed = false;
    const initial = async () => {
      await Promise.all([recoveredEvents.ready, sessionEvents.ready]).catch(() => undefined);
      if (disposed) return;
      const initialRevision = revision;
      loadPlatform().then(
        (name) => (platform = name),
        () => undefined,
      );
      await Promise.all([
        readRecorderSettings().then(
          (settings) => {
            quality = settings.recording_quality;
            dispatch({
              type: "first-use-loaded",
              folderConfirmed: settings.recording_folder_confirmed,
              consentAcknowledged: settings.recording_consent_acknowledged,
            });
          },
          (error) => fail(error),
        ),
        refreshFolder(),
        refreshRecordings(),
        getRecordingSession().then(
          (info) => {
            // Only a running session matters at open; the last finished one is in the list.
            const running = info.state === "starting" || info.state === "recording" || info.state === "finalizing";
            if (!disposed && revision === initialRevision && running) dispatch({ type: "session", info });
          },
          (error) => fail(error),
        ),
        recoverRecordings().then(
          (notes) => {
            if (!disposed && notes.length > 0) recovery = notes;
          },
          (error) => fail(error),
        ),
      ]);
    };
    void initial();
    return () => {
      disposed = true;
    };
  });

  async function start() {
    if (!canStart(recorder)) return;
    actionError = null;
    statusMessage = "";
    transcripts = {};
    dispatch({ type: "start-requested" });
    try {
      dispatch({ type: "start-succeeded", info: await startRecordingSession(null) });
    } catch (error) {
      dispatch({ type: "start-failed", error: toRecorderError(error) });
    }
  }

  async function stop() {
    if (!canStop(recorder)) return;
    dispatch({ type: "stop-requested" });
    try {
      dispatch({ type: "stop-succeeded", finished: await stopRecordingSession() });
      await refreshRecordings();
    } catch (error) {
      dispatch({ type: "stop-failed", error: toRecorderError(error) });
    }
  }

  async function retry() {
    dispatch({ type: "dismiss" });
    await start();
  }

  async function chooseQuality(next: RecordingQuality) {
    if (active || next === quality) return;
    const previous = quality;
    quality = next;
    actionError = null;
    try {
      await setRecordingQuality(next);
    } catch (error) {
      quality = previous;
      fail(error);
    }
  }

  async function confirmFolder() {
    actionError = null;
    try {
      await confirmRecordingFolder();
      dispatch({ type: "folder-confirmed" });
    } catch (error) {
      fail(error);
    }
  }

  async function acknowledgeConsent() {
    actionError = null;
    try {
      await acknowledgeRecordingConsent();
      dispatch({ type: "consent-acknowledged" });
    } catch (error) {
      fail(error);
    }
  }

  async function changeFolder() {
    if (active) return;
    actionError = null;
    try {
      const picked = await open({ directory: true, multiple: false, title: "Choose the recordings folder" });
      if (typeof picked !== "string") return;
      await setRecordingDirectory(picked);
      // Choosing the folder is the confirmation.
      await confirmRecordingFolder();
      dispatch({ type: "folder-confirmed" });
      await Promise.all([refreshFolder(), refreshRecordings()]);
    } catch (error) {
      fail(error);
    }
  }

  function nameOf(recording: RecordingSummary): string {
    return recording.title.trim() || recording.folder_name;
  }

  function metaOf(recording: RecordingSummary): string {
    const gaps = gapText(recording.gap_count, recording.gap_seconds);
    return [formatStartedAt(recording.started_at), formatDuration(recording.duration_seconds), formatBytes(recording.bytes), gaps]
      .filter(Boolean)
      .join(" · ");
  }

  async function run(recording: RecordingSummary, action: string, work: () => Promise<void>) {
    if (busy[recording.folder_name]) return;
    actionError = null;
    busy[recording.folder_name] = action;
    try {
      await work();
    } catch (error) {
      fail(error);
    } finally {
      delete busy[recording.folder_name];
    }
  }

  const revealIt = (recording: RecordingSummary) => run(recording, "reveal", () => revealRecording(recording.folder_name));

  const transcribeIt = (recording: RecordingSummary) =>
    run(recording, "transcribe", async () => {
      const text = await transcribeRecording(recording.folder_name);
      transcripts[recording.folder_name] = typeof text === "string" ? text : "";
    });

  const trashIt = (recording: RecordingSummary) =>
    run(recording, "trash", async () => {
      await deleteRecording(recording.folder_name);
      recordings = recordings.filter((other) => other.folder_name !== recording.folder_name);
      if (playing === recording.folder_name) playing = null;
      if (finished?.summary.folder_name === recording.folder_name) dispatch({ type: "dismiss" });
      statusMessage = `Moved “${nameOf(recording)}” to the ${trashName(platform)}.`;
    });

  async function recoverAll() {
    actionError = null;
    if (phase === "error") dispatch({ type: "dismiss" });
    try {
      recovery = await recoverRecordings();
      await refreshRecordings();
    } catch (error) {
      fail(error);
    }
  }

  function togglePlayback(recording: RecordingSummary) {
    playing = playing === recording.folder_name ? null : recording.folder_name;
  }

  const earlyText = $derived(finished ? endedEarlyText(finished.ended_early) : null);
</script>

{#snippet actions(recording: RecordingSummary)}
  {@const name = nameOf(recording)}
  {@const working = busy[recording.folder_name]}
  <div class="actions">
    <button
      type="button"
      class="secondary"
      aria-label="{reveal}: {name}"
      disabled={working === "reveal"}
      onclick={() => void revealIt(recording)}
    >
      {reveal}
    </button>
    {#if recording.needs_recovery}
      <button type="button" class="secondary" aria-label="Recover {name}" onclick={() => void recoverAll()}>Recover</button>
    {:else}
      <button
        type="button"
        class="secondary"
        aria-label="Transcribe {name}"
        disabled={working === "transcribe"}
        onclick={() => void transcribeIt(recording)}
      >
        {working === "transcribe" ? "Transcribing…" : "Transcribe"}
      </button>
    {/if}
    <button
      type="button"
      class="danger"
      aria-label="{trash}: {name}"
      disabled={working === "trash"}
      onclick={() => void trashIt(recording)}
    >
      {trash}
    </button>
  </div>
  {#if recording.playable}
    {#if playbackFallback}
      <p class="muted">Playback not verified, use Reveal.</p>
    {:else}
      <button
        type="button"
        class="secondary play"
        aria-pressed={playing === recording.folder_name}
        onclick={() => togglePlayback(recording)}
      >
        {playing === recording.folder_name ? `Hide player: ${name}` : `Play: ${name}`}
      </button>
      {#if playing === recording.folder_name}
        {#if playbackFailed[recording.folder_name]}
          <p class="muted">This recording could not be played here. Use Reveal instead.</p>
        {:else}
          <audio
            controls
            preload="metadata"
            src={recordingAudioUrl(recording.folder_name)}
            aria-label="Recording audio: {name}"
            onerror={() => (playbackFailed[recording.folder_name] = true)}
          ></audio>
        {/if}
      {/if}
    {/if}
  {/if}
  {#if recording.folder_name in transcripts}
    {#if transcripts[recording.folder_name]}
      <div class="transcript" role="region" aria-label="Transcript: {name}">{transcripts[recording.folder_name]}</div>
    {:else}
      <p class="muted" role="status">Transcription finished.</p>
    {/if}
  {/if}
{/snippet}

<div class="record">
  <h1>Record</h1>

  {#if active}
    <div class="indicator" role="status" aria-label="Recording state">
      <span class="dot" aria-hidden="true"></span>
      <span class="indicator-text">
        {#if phase === "starting"}Starting…{:else if phase === "stopping"}Saving the recording…{:else}Recording{/if}
      </span>
    </div>
  {/if}

  <div class="timer" role="timer" aria-label="Elapsed time">{formatElapsed(live?.elapsed_ms ?? 0)}</div>

  <div class="meter-row">
    <div
      class="meter"
      role="meter"
      aria-label="Input level"
      aria-valuemin="0"
      aria-valuemax="100"
      aria-valuenow={percent}
      aria-valuetext={live ? formatLevelDb(live.level) : "not recording"}
    >
      <div class="meter-fill" class:clipped={live?.clipped} style:width="{percent}%"></div>
    </div>
    <span class="meter-db" aria-hidden="true">{live ? formatLevelDb(live.level) : ""}</span>
  </div>

  {#if notices.length > 0}
    <ul class="notices" aria-label="Recording notices">
      {#each notices as notice (notice)}
        <li>{notice}</li>
      {/each}
    </ul>
  {/if}

  {#if phase === "recording" || phase === "stopping"}
    <button type="button" class="primary big stop" disabled={phase === "stopping"} onclick={() => void stop()}>
      {phase === "stopping" ? "Saving…" : "Stop recording"}
    </button>
  {:else}
    <button
      type="button"
      class="primary big"
      disabled={!canStart(recorder)}
      aria-describedby={step === "done" ? undefined : "first-use"}
      onclick={() => void start()}
    >
      {phase === "starting" ? "Starting…" : "Start recording"}
    </button>
  {/if}

  {#if recorder.error && startError}
    <div class="alert" role="alert">
      <p>{startError.text}</p>
      {#if startError.hint}<p class="muted">{startError.hint}</p>{/if}
      <div class="actions">
        {#if recorder.error.code === "recording_recovery_needed"}
          <button type="button" class="secondary" onclick={() => void recoverAll()}>Recover</button>
        {:else if canStart(recorder)}
          <button type="button" class="secondary" onclick={() => void retry()}>Try again</button>
        {/if}
        <button type="button" class="secondary" onclick={() => dispatch({ type: "dismiss" })}>Dismiss</button>
      </div>
    </div>
  {/if}

  {#if actionError && actionErrorText}
    <div class="alert" role="alert">
      <p>{actionErrorText.text}</p>
      {#if actionErrorText.hint}<p class="muted">{actionErrorText.hint}</p>{/if}
      <div class="actions">
        <button type="button" class="secondary" onclick={() => (actionError = null)}>Dismiss</button>
      </div>
    </div>
  {/if}

  {#if step === "folder"}
    <section class="step" id="first-use" aria-labelledby="step-folder">
      <h2 id="step-folder">Where should recordings go?</h2>
      <p>
        Recordings are saved in
        {#if folder}<code>{folder.path}</code>{:else}the default folder{/if}. Use this folder, or change it first.
      </p>
      <button type="button" class="primary" onclick={() => void confirmFolder()}>Use this folder</button>
    </section>
  {:else if step === "consent"}
    <section class="step" id="first-use" aria-labelledby="step-consent">
      <h2 id="step-consent">Before you record</h2>
      <p>
        Recording other people can require their consent. Make sure everyone who can be heard knows they are being recorded and agrees to
        it.
      </p>
      <button type="button" class="primary" onclick={() => void acknowledgeConsent()}>I understand</button>
    </section>
  {/if}

  <fieldset class="quality" disabled={active}>
    <legend>Quality</legend>
    {#each QUALITIES as option (option.id)}
      <label class="choice" class:selected={quality === option.id}>
        <input
          type="radio"
          name="recording-quality"
          value={option.id}
          checked={quality === option.id}
          aria-labelledby="name-{option.id}"
          aria-describedby="size-{option.id}"
          onchange={() => void chooseQuality(option.id)}
        />
        <span class="choice-name" id="name-{option.id}">{option.label}</span>
        <span class="choice-size" id="size-{option.id}">{formatSizePerHour(option.id)}</span>
      </label>
    {/each}
  </fieldset>

  <section class="folder" aria-label="Recording folder">
    <div class="folder-row">
      <div class="folder-path">
        <span class="field-label">Folder{folder?.is_default ? " (default)" : ""}</span>
        <code>{folder?.path ?? "…"}</code>
        {#if folder && folder.free_bytes !== null}<span class="muted">{formatBytes(folder.free_bytes)} free</span>{/if}
      </div>
      <button type="button" class="secondary" disabled={active} onclick={() => void changeFolder()}>Change…</button>
    </div>
    {#if folder && !folder.exists}
      <p class="muted">The folder does not exist yet; it is created with the first recording.</p>
    {/if}
    {#if folder && folder.warnings.length > 0}
      <ul class="notices" aria-label="Folder warnings">
        {#each folder.warnings as warning (warning)}
          <li>{warningText(warning)}</li>
        {/each}
      </ul>
    {/if}
  </section>

  {#if phase === "finished" && finished}
    {@const summary = finished.summary}
    <section class="finished" aria-label="Finished recording">
      <h2>Saved: {nameOf(summary)}</h2>
      <p class="meta">{formatDuration(summary.duration_seconds)} · {formatBytes(summary.bytes)}</p>
      {#if gapText(summary.gap_count, summary.gap_seconds)}
        <p class="meta">{gapText(summary.gap_count, summary.gap_seconds)}</p>
      {/if}
      {#if earlyText}<p class="alert" role="alert">{earlyText}</p>{/if}
      {@render actions(summary)}
    </section>
  {/if}

  <div class="status" role="status" aria-label="Last action" aria-live="polite">{statusMessage}</div>

  {#if recoveryTitle}
    <section class="recovery" aria-label="Recovered recordings">
      <h2>{recoveryTitle}</h2>
      <ul>
        {#each recoveryNotes as note (note.folder_name)}
          <li><strong>{note.title || note.folder_name}</strong>: {note.message}</li>
        {/each}
      </ul>
      <button type="button" class="secondary" onclick={() => (recovery = [])}>Dismiss</button>
    </section>
  {/if}

  <section class="recent" aria-labelledby="recent-title">
    <h2 id="recent-title">Recent recordings</h2>
    {#if needsRecovery}
      <p class="muted">Some recordings were interrupted. Recover them to keep what was saved.</p>
    {/if}
    {#if shownRecordings.length === 0}
      <p class="muted">No recordings yet.</p>
    {:else}
      <ul class="list">
        {#each shownRecordings as recording (recording.folder_name)}
          <li class="item">
            <div class="item-head">
              <span class="item-name">{nameOf(recording)}</span>
              {#if recording.recovered}<span class="badge">Recovered</span>{/if}
              {#if recording.needs_recovery}<span class="badge warn">Interrupted</span>{/if}
            </div>
            <div class="meta">{metaOf(recording)}</div>
            {@render actions(recording)}
          </li>
        {/each}
      </ul>
    {/if}
  </section>
</div>

<style>
  .record {
    display: flex;
    flex-direction: column;
    gap: 14px;
    padding: 20px;
    max-width: 520px;
    margin: 0 auto;
  }

  h1 {
    font-size: 18px;
  }

  h2 {
    font-size: 14px;
    margin-bottom: 6px;
  }

  p {
    margin: 0;
  }

  code {
    font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
    font-size: 12px;
    word-break: break-all;
    user-select: text;
  }

  .muted,
  .meta {
    color: var(--text-muted);
    font-size: 12px;
  }

  .indicator {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 10px;
    padding: 10px 16px;
    border-radius: var(--radius);
    background: rgba(255, 59, 48, 0.14);
    border: 1px solid #ff3b30;
    color: #ff6b61;
    font-weight: 700;
    font-size: 15px;
    letter-spacing: 0.04em;
    text-transform: uppercase;
  }

  .dot {
    width: 14px;
    height: 14px;
    border-radius: 50%;
    background: #ff3b30;
    animation: pulse 1.2s ease-in-out infinite;
    flex-shrink: 0;
  }

  .timer {
    text-align: center;
    font-size: 44px;
    font-weight: 600;
    font-variant-numeric: tabular-nums;
    line-height: 1.1;
  }

  .meter-row {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .meter {
    flex: 1;
    height: 10px;
    border-radius: 5px;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    overflow: hidden;
  }

  .meter-fill {
    height: 100%;
    background: var(--accent);
    transition: width 0.1s linear;
  }

  .meter-fill.clipped {
    background: var(--danger);
  }

  .meter-db {
    width: 52px;
    text-align: right;
    font-size: 11px;
    color: var(--text-muted);
    font-variant-numeric: tabular-nums;
  }

  button.big {
    padding: 14px 20px;
    font-size: 16px;
  }

  button.stop {
    background: #ff3b30;
    color: white;
  }

  button:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  button:focus-visible,
  input:focus-visible + .choice-name {
    outline: 2px solid var(--accent);
    outline-offset: 2px;
  }

  .alert {
    border: 1px solid var(--danger);
    border-radius: var(--radius);
    padding: 10px 12px;
    background: rgba(255, 107, 107, 0.08);
  }

  .alert .actions {
    margin-top: 8px;
  }

  .notices {
    list-style: none;
    border: 1px solid #ff9f0a;
    border-radius: var(--radius);
    padding: 8px 12px;
    background: rgba(255, 159, 10, 0.08);
    font-size: 12px;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .step,
  .finished,
  .recovery {
    border: 1px solid var(--accent);
    border-radius: var(--radius);
    background: var(--accent-dim);
    padding: 12px 14px;
    display: flex;
    flex-direction: column;
    gap: 8px;
    align-items: flex-start;
  }

  .recovery ul {
    padding-left: 18px;
    user-select: text;
  }

  fieldset.quality {
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 8px 12px 10px;
    display: flex;
    gap: 16px;
  }

  fieldset.quality legend {
    padding: 0 6px;
    color: var(--text-muted);
    font-size: 12px;
  }

  .choice {
    display: grid;
    grid-template-columns: auto 1fr;
    column-gap: 8px;
    align-items: baseline;
    margin: 0;
    flex: 1;
    color: var(--text);
    cursor: pointer;
  }

  /* A disabled radio (while recording) loses its checked look: the chosen
     quality stays readable by its weight and colour. */
  .choice.selected .choice-name {
    color: var(--accent);
    font-weight: 600;
  }

  .choice-size {
    grid-column: 2;
    color: var(--text-muted);
    font-size: 11px;
  }

  .folder-row {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 12px;
  }

  .folder-path {
    display: flex;
    flex-direction: column;
    gap: 2px;
    min-width: 0;
  }

  .folder .notices {
    margin-top: 8px;
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 8px;
  }

  .list {
    list-style: none;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .item {
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 10px 12px;
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .item-head {
    display: flex;
    gap: 8px;
    align-items: center;
  }

  .item-name {
    font-weight: 600;
  }

  .badge {
    font-size: 11px;
    padding: 1px 8px;
    border-radius: 10px;
    border: 1px solid var(--border);
    color: var(--text-muted);
  }

  .badge.warn {
    border-color: #ff9f0a;
    color: #ff9f0a;
  }

  .transcript {
    white-space: pre-wrap;
    user-select: text;
    max-height: 180px;
    overflow-y: auto;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 8px 10px;
    background: var(--bg-secondary);
  }

  audio {
    width: 100%;
  }

  .status:empty {
    display: none;
  }

  @keyframes pulse {
    0%,
    100% {
      opacity: 1;
      transform: scale(1);
    }
    50% {
      opacity: 0.45;
      transform: scale(0.85);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .dot {
      animation: none;
    }

    .meter-fill {
      transition: none;
    }
  }
</style>
