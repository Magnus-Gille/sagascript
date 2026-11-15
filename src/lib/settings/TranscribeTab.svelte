<script lang="ts">
  import FileTranscription from "../FileTranscription.svelte";
  import { fileJobName } from "../transcription-queue";
  import { tick } from "svelte";
  import { isPianissimoId } from "../models.svelte";
  import { languageLabel } from "./languageLabel";
  import type { SettingsWindow } from "./settingsWindow.svelte";

  let { win, active }: { win: SettingsWindow; active: boolean } = $props();
  // The window is one object for the life of the panel.
  // svelte-ignore state_referenced_locally
  const { session, router, queue, fileModel, recovery, downloads: modelDownloads } = win;

  const settings = $derived(session.settings!); // the window renders panels only once the settings are loaded
  const downloading = $derived(modelDownloads.id);
  const downloadingName = $derived(modelDownloads.name);
  const downloadProgress = $derived(modelDownloads.progress);
  const updatePreparing = $derived(recovery.preparing);

  let showDiarizeInfo = $state(false);
  let diarizeDialog: HTMLDialogElement | undefined = $state();
  let diarizeInfoButton: HTMLButtonElement | undefined = $state();
  $effect(() => {
    if (showDiarizeInfo && diarizeDialog && !diarizeDialog.open) diarizeDialog.showModal();
  });
  async function closeDiarizeInfo(): Promise<void> {
    showDiarizeInfo = false;
    await tick();
    diarizeInfoButton?.focus();
  }
</script>

<section hidden={!active} aria-label="File transcription">
  <div class="transcribe-settings-row">
    <button class="active-config-bar" onclick={() => router.request("settings")}>
      <div class="active-config-row">
        <span class="active-config-label">Language</span>
        <span class="active-config-value">{languageLabel(queue.language())}</span>
      </div>
      <span class="active-config-link">Settings</span>
    </button>
    <div class="field profile-field">
      <label for="transcribe-profile">Profile (optional)</label>
      <select
        id="transcribe-profile"
        value={queue.selectedProfile()?.id ?? ""}
        onchange={queue.onProfileChange}
        disabled={queue.transcribing}
      >
        {#each session.profiles() as profile (profile.id)}
          <option value={profile.id}>{profile.name} · {languageLabel(profile.language)}</option>
        {/each}
      </select>
    </div>
  </div>

  <div class="transcribe-options">
    <div class="field profile-field">
      <label for="file-model">File transcription model</label>
      <select
        id="file-model"
        value={settings.file_transcription_model}
        onchange={(event) => void fileModel.onChange(event)}
        disabled={queue.transcribing || fileModel.saving || downloading !== null || session.locked}
      >
        <option value="auto"
          >Auto — {fileModel.auto?.display_name ?? "loading…"}{fileModel.auto &&
          fileModel.recommended &&
          fileModel.auto.id !== fileModel.recommended.id
            ? " (downloaded)"
            : " (recommended)"}</option
        >
        {#if settings.file_transcription_model !== "auto" && !fileModel.options.some((model) => model.id === settings?.file_transcription_model)}
          <option value={settings.file_transcription_model}>Current choice is incompatible with {languageLabel(queue.language())}</option>
        {/if}
        {#each fileModel.options as model (model.id)}
          <option value={model.id}>{model.display_name} · {model.size_mb} MB{model.downloaded ? " · ready" : " · download needed"}</option>
        {/each}
      </select>
      <div class="hotkey-hint">
        Effective model: {settings.file_transcription_model === "auto"
          ? (fileModel.auto?.display_name ?? "loading…")
          : (fileModel.options.find((model) => model.id === settings?.file_transcription_model)?.display_name ??
            "incompatible with this language")}.
        {#if settings.file_transcription_model === "auto" && fileModel.auto && fileModel.recommended && fileModel.auto.id !== fileModel.recommended.id}
          {fileModel.recommended.display_name} is recommended{fileModel.recommended.size_mb > 0
            ? ` (${fileModel.recommended.size_mb} MB download)`
            : ""}; Auto keeps using the model you already have.
        {/if}
        This choice affects files only; dictation shortcuts keep their own model.
      </div>
      {#if (settings.file_transcription_model !== "auto" && fileModel.options.some((model) => model.id === settings?.file_transcription_model && !model.downloaded)) || (settings.file_transcription_model === "auto" && fileModel.recommended && !fileModel.recommended.downloaded)}
        <button class="secondary" onclick={() => void fileModel.downloadSelected()} disabled={downloading !== null || queue.transcribing}>
          {downloading !== null
            ? `Downloading ${downloadingName}… ${downloadProgress}%`
            : settings.file_transcription_model === "auto" && fileModel.recommended
              ? `Download ${fileModel.recommended.display_name}`
              : "Download selected model"}
        </button>
      {/if}
      {#if isPianissimoId(settings.file_transcription_model)}
        <div class="hotkey-hint">
          Requires macOS 14+ on Apple Silicon or Windows on ARM (Snapdragon). Runs locally (Neural Engine on Mac, CPU on Windows){fileModel.pianissimoSizeMb >
          0
            ? `; one-time download of about ${fileModel.pianissimoSizeMb} MB`
            : "; downloaded once"}.
        </div>
      {/if}
      {#if modelDownloads.fileError}<div class="transcribe-error" role="alert">{modelDownloads.fileError}</div>{/if}
    </div>
    {#if queue.selectedProfile()}
      <div class="hotkey-hint">This profile fixes the file language and uses its personal dictionary.</div>
    {/if}
    <div class="diarize-row">
      <label class="diarize-option">
        <input
          type="checkbox"
          bind:checked={queue.diarize}
          disabled={queue.transcribing || isPianissimoId(settings.file_transcription_model)}
        />
        Speaker diarization
      </label>
      <button
        class="info-dot"
        bind:this={diarizeInfoButton}
        aria-label="What is speaker diarization?"
        aria-expanded={showDiarizeInfo}
        onclick={() => {
          showDiarizeInfo = !showDiarizeInfo;
        }}>?</button
      >
    </div>
    {#if showDiarizeInfo}
      <dialog
        bind:this={diarizeDialog}
        class="popover-card"
        aria-label="What is speaker diarization?"
        oncancel={closeDiarizeInfo}
        onclose={closeDiarizeInfo}
      >
        <h3>Speaker diarization</h3>
        <p>Detects <strong>who speaks when</strong> and labels each part of the transcript — [Speaker 1], [Speaker 2], …</p>
        <p>Slower than plain transcription, and needs the diarization models (downloaded once).</p>
        <p>Leave it off for a plain transcript.</p>
        <button class="secondary" onclick={closeDiarizeInfo}>Close</button>
      </dialog>
    {/if}
    <textarea
      class="prompt-input"
      aria-label="Extra context for this file"
      placeholder="Extra context, e.g. names to listen for: Astrid, Grimnir (optional)"
      bind:value={queue.prompt}
      rows="2"
      disabled={queue.transcribing || isPianissimoId(settings.file_transcription_model)}></textarea>
  </div>

  <div class="drop-zone" class:drag-over={queue.dragOver} class:transcribing={queue.transcribing}>
    <div class="drop-zone-icon">&#x1F4C1;</div>
    <div class="drop-zone-text">Drop audio or video files here</div>
    <button class="primary open-file-btn" onclick={queue.pickFiles} disabled={updatePreparing}>Open Files...</button>
    {#if queue.recent.length}
      <div class="rerun-controls">
        <button
          class="secondary rerun-highlight"
          onclick={queue.rerunSelected}
          title="Queue a new run with current model settings and this file's saved profile/context">Re-run</button
        >
        <select
          aria-label="Recent files to re-run (last 5, this session)"
          bind:value={queue.selectedRerunPath}
          title={queue.selectedRerunPath}
        >
          {#each queue.recent as run (run.path)}
            <option value={run.path}
              >{queue.recent.filter((item) => fileJobName(item.path) === fileJobName(run.path)).length > 1
                ? run.path
                : fileJobName(run.path)}</option
            >
          {/each}
        </select>
      </div>
    {/if}
    <button class="secondary" onclick={() => queue.addFiles(["Saved review"], true)} disabled={queue.transcribing || updatePreparing}>
      Open saved meeting...
    </button>
  </div>
  {#if queue.rerunError}<div class="transcribe-error" role="alert">{queue.rerunError}</div>{/if}

  <div class="formats-hint">
    Supported: {queue.supportedFormats.map((f) => f.toUpperCase()).join(", ") || "WAV, MP3, M4A, AAC, MP4, MOV, OGG, WEBM, FLAC"}
  </div>

  {#if queue.jobs.length}
    <div class="result-tabs" role="tablist" aria-label="Transcription results">
      {#each queue.jobs as job, index (job.id)}
        <button
          class="result-tab"
          class:active={queue.selectedId === job.id}
          id={`file-tab-${job.id}`}
          role="tab"
          aria-selected={queue.selectedId === job.id}
          aria-controls={`file-panel-${job.id}`}
          tabindex={queue.selectedId === job.id ? 0 : -1}
          title={job.path}
          onclick={() => {
            queue.selectedId = job.id;
          }}
          onkeydown={(event) => queue.onResultTabKeydown(event, index)}
        >
          <span class="result-filename">{fileJobName(job.path)}</span>
          <span class="result-status">{queue.attention[job.id] ? "needs retry" : job.status}</span>
        </button>
      {/each}
    </div>
  {/if}
  {#each queue.jobs as job (job.id)}
    <div
      id={`file-panel-${job.id}`}
      role="tabpanel"
      aria-labelledby={`file-tab-${job.id}`}
      inert={updatePreparing}
      tabindex="0"
      hidden={queue.selectedId !== job.id}
    >
      <FileTranscription
        {job}
        bind:this={() => recovery.resultPanel(job.id), (panel) => recovery.setResultPanel(job.id, panel)}
        otherBusy={updatePreparing ||
          queue.jobs.some((other) => other.id !== job.id && other.status === "running") ||
          Object.entries(queue.busy).some(([id, busy]) => id !== job.id && busy)}
        openReview={queue.savedReviewIds.includes(job.id)}
        onComplete={queue.complete}
        onBusyChange={queue.setBusy}
        onMissingFile={queue.forgetMissing}
        onRetry={queue.retryJob}
        retryRefusal={queue.retryRefusals[job.id] ?? ""}
        onDownloadModel={() => fileModel.downloadEngineModelAgain([fileModel.pianissimoIdFor(job)])}
        downloadBusy={modelDownloads.busy || modelDownloads.repairing}
        {updatePreparing}
        onAttentionChange={queue.setAttention}
        initialRecoveryFile={recovery.fileEntries.find((entry) => entry.job_id === job.id) ?? null}
        initialRecoveryMeeting={recovery.meetingEntries.find((entry) => entry.job_id === job.id) ?? null}
        onFileRecoveryChange={(entry) => recovery.onFileRecoveryChange(entry, job.id)}
        onMeetingRecoveryChange={(entry) => recovery.onMeetingRecoveryChange(entry, job.id)}
        active={active && queue.selectedId === job.id}
      />
    </div>
  {/each}
</section>

<style>
  :global(.content) > * {
    min-width: 0;
    max-width: 100%;
  }

  /* While the settings are unconfirmed, the controls that save a setting are disabled;
     make that visible (Settings.svelte, .settings-locked). Placed before the scoped
     :disabled rules below so those win where they set the same properties. */
  :global(.settings-locked) button:disabled,
  :global(.settings-locked) select:disabled,
  :global(.settings-locked) input:disabled,
  :global(.settings-locked) textarea:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .active-config-bar {
    display: flex;
    align-items: center;
    gap: 16px;
    width: 100%;
    padding: 8px 12px;
    margin-bottom: 16px;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    cursor: pointer;
    transition: border-color 0.15s;
    min-width: 0;
  }

  .active-config-bar:hover {
    border-color: var(--text-muted);
  }

  .active-config-row {
    display: flex;
    flex-direction: column;
    gap: 1px;
    text-align: left;
    min-width: 0;
  }

  .active-config-label {
    font-size: 10px;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--text-muted);
    font-weight: 600;
  }

  .active-config-value {
    font-size: 13px;
    color: var(--text);
    overflow-wrap: anywhere;
  }

  .active-config-link {
    font-size: 11px;
    color: var(--accent);
    font-weight: 500;
    margin-left: auto;
    flex-shrink: 0;
  }

  .hotkey-hint {
    margin-top: 4px;
    font-size: 11px;
    color: var(--text-secondary, #888);
  }

  /* Transcribe tab */
  .transcribe-settings-row {
    display: flex;
    gap: 8px;
    align-items: flex-start;
    margin-bottom: 8px;
  }
  .transcribe-settings-row .active-config-bar {
    flex: 1 1 0;
    width: auto;
    margin-bottom: 0;
    min-width: 0;
  }
  .transcribe-settings-row .profile-field {
    flex: 1.2 1 0;
    margin-bottom: 0;
    min-width: 0;
  }
  .rerun-controls {
    display: flex;
    align-items: center;
    gap: 8px;
    width: 100%;
    min-width: 0;
  }
  .rerun-controls button {
    flex: 0 0 auto;
  }
  .rerun-controls select {
    flex: 1;
    min-width: 0;
    width: 0;
  }
  button.secondary.rerun-highlight {
    border-color: #eab308;
    color: #eab308;
  }
  button.secondary.rerun-highlight:hover:not(:disabled) {
    background: color-mix(in srgb, #eab308 12%, transparent);
  }
  .diarize-row {
    display: flex;
    align-items: center;
    gap: 6px;
  }
  .info-dot {
    width: 18px;
    height: 18px;
    padding: 0;
    border: 1px solid var(--text-muted);
    border-radius: 50%;
    background: transparent;
    color: var(--text-muted);
  }
  .popover-card {
    margin: auto;
    width: 280px;
    max-width: calc(100vw - 48px);
    padding: 14px 16px;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: 12px;
    color: var(--text);
    font-size: 12px;
    line-height: 1.55;
  }
  .popover-card::backdrop {
    background: rgba(0, 0, 0, 0.45);
  }
  .popover-card p {
    margin: 8px 0;
  }

  .drop-zone {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    padding: 12px 16px;
    border: 2px dashed var(--border);
    border-radius: 12px;
    text-align: center;
    transition:
      border-color 0.2s,
      background 0.2s;
  }

  .drop-zone.drag-over {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 6%, var(--bg));
  }

  .drop-zone.transcribing {
    border-style: solid;
    border-color: var(--accent);
  }

  .drop-zone-icon {
    font-size: 28px;
    line-height: 1;
  }

  .drop-zone-text {
    font-size: 13px;
    color: var(--text-muted);
  }

  .open-file-btn {
    margin-top: 4px;
  }

  .formats-hint {
    font-size: 11px;
    color: var(--text-muted);
    margin-top: 10px;
    text-align: center;
  }

  .transcribe-options {
    display: flex;
    flex-direction: column;
    gap: 8px;
    margin-top: 0;
    margin-bottom: 8px;
  }

  .diarize-option {
    display: flex;
    align-items: center;
    gap: 8px;
    font-size: 13px;
    color: var(--text);
    cursor: pointer;
    user-select: none;
  }

  .diarize-option input[type="checkbox"] {
    width: 15px;
    height: 15px;
    accent-color: var(--accent);
    cursor: pointer;
    flex-shrink: 0;
  }

  .prompt-input {
    width: 100%;
    padding: 8px 10px;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    color: var(--text);
    font-family: inherit;
    font-size: 12px;
    line-height: 1.5;
    resize: vertical;
    outline: none;
    box-sizing: border-box;
  }

  .prompt-input::placeholder {
    color: var(--text-muted);
  }

  .prompt-input:focus {
    border-color: var(--accent);
  }

  .transcribe-error {
    margin-top: 12px;
    padding: 10px 14px;
    background: color-mix(in srgb, var(--danger) 12%, var(--bg));
    border: 1px solid var(--danger);
    border-radius: var(--radius);
    color: var(--danger);
    font-size: 12px;
  }

  .result-tabs {
    display: flex;
    gap: 6px;
    overflow-x: auto;
    margin: 18px 0 12px;
    padding-bottom: 4px;
  }

  .result-tab {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 4px;
    min-width: 110px;
    max-width: 210px;
    padding: 10px 12px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg-secondary);
    color: var(--text);
    cursor: pointer;
  }
  .result-tab.active {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 10%, var(--bg));
  }
  .result-filename {
    max-width: 100%;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .result-status {
    font-size: 11px;
    color: var(--text-muted);
    text-transform: capitalize;
  }
  [hidden] {
    display: none !important;
  }
</style>
