<script lang="ts">
  import { onMount } from "svelte";
  import {
    getSettings,
    setLanguage,
    setHotkeyMode,
    setAutoPaste,
    setShowOverlay,
    setWhisperModel,
    getBuildInfo,
    getModelInfo,
    downloadModel,
    transcribeFile,
    getSupportedFormats,
    type Settings,
    type BuildInfo,
    type Language,
    type HotkeyMode,
    type WhisperModel,
  } from "./api";
  import { listen } from "@tauri-apps/api/event";
  import { open } from "@tauri-apps/plugin-dialog";
  import { getCurrentWebview } from "@tauri-apps/api/webview";

  let settings: Settings | null = $state(null);
  let buildInfo: BuildInfo | null = $state(null);
  let models: WhisperModel[] = $state([]);
  let activeTab: "general" | "model" | "advanced" | "transcribe" = $state("general");
  let downloading: string | null = $state(null);
  let downloadProgress: number = $state(0);

  // Transcribe tab state
  let supportedFormats: string[] = $state([]);
  let transcribing: boolean = $state(false);
  let transcriptionResult: string = $state("");
  let transcribeError: string = $state("");
  let dragOver: boolean = $state(false);

  onMount(async () => {
    settings = await getSettings();
    buildInfo = await getBuildInfo();
    models = await getModelInfo();
    supportedFormats = await getSupportedFormats();

    // Check URL params for initial tab
    const params = new URLSearchParams(window.location.search);
    const tab = params.get("tab");
    if (tab === "transcribe" || tab === "general" || tab === "model" || tab === "advanced") {
      activeTab = tab;
    }

    listen("model_download_progress", (event: any) => {
      downloadProgress = event.payload.progress;
    });

    listen("model_ready", async () => {
      downloading = null;
      downloadProgress = 0;
      models = await getModelInfo();
    });

    // Listen for tab navigation from tray menu
    listen("navigate_tab", (event: any) => {
      const t = event.payload;
      if (t === "transcribe" || t === "general" || t === "model" || t === "advanced") {
        activeTab = t;
      }
    });

    // Listen for drag-and-drop events
    const webview = getCurrentWebview();
    webview.onDragDropEvent((event) => {
      if (event.payload.type === "over") {
        dragOver = true;
      } else if (event.payload.type === "drop") {
        dragOver = false;
        const paths = event.payload.paths;
        if (paths.length > 0) {
          activeTab = "transcribe";
          handleFileTranscription(paths[0]);
        }
      } else {
        dragOver = false;
      }
    });
  });

  async function onLanguageChange(e: Event) {
    const value = (e.target as HTMLSelectElement).value as Language;
    await setLanguage(value);
    settings = await getSettings();
    models = await getModelInfo();
  }

  async function onHotkeyModeChange(e: Event) {
    const value = (e.target as HTMLSelectElement).value as HotkeyMode;
    await setHotkeyMode(value);
    settings = await getSettings();
  }

  async function onAutoPasteToggle() {
    if (!settings) return;
    await setAutoPaste(!settings.auto_paste);
    settings = await getSettings();
  }

  async function onShowOverlayToggle() {
    if (!settings) return;
    await setShowOverlay(!settings.show_overlay);
    settings = await getSettings();
  }

  async function selectModel(model: WhisperModel) {
    try {
      if (!model.downloaded) {
        downloading = model.id;
        downloadProgress = 0;
        await downloadModel(model.id);
        // model_ready event will refresh the list
      }
      await setWhisperModel(model.id);
      settings = await getSettings();
      models = await getModelInfo();
    } catch (e) {
      console.error("Model selection failed:", e);
    } finally {
      downloading = null;
      downloadProgress = 0;
    }
  }

  async function handleFileTranscription(filePath: string) {
    transcribing = true;
    transcribeError = "";
    transcriptionResult = "";
    try {
      transcriptionResult = await transcribeFile(filePath);
    } catch (e: any) {
      transcribeError = typeof e === "string" ? e : e.message || "Transcription failed";
    } finally {
      transcribing = false;
    }
  }

  async function onPickFile() {
    const exts = supportedFormats.length > 0 ? supportedFormats : ["wav", "mp3", "m4a", "mp4", "ogg", "flac"];
    const file = await open({
      multiple: false,
      filters: [
        {
          name: "Audio/Video",
          extensions: exts,
        },
      ],
    });
    if (file) {
      await handleFileTranscription(file);
    }
  }

  function languageLabel(lang: Language): string {
    switch (lang) {
      case "sv": return "Swedish";
      case "no": return "Norwegian";
      case "en": return "English";
      default: return "Auto-detect";
    }
  }
</script>

<div class="settings-window">
  <div class="titlebar">FlowDictate Settings</div>

  <div class="tabs">
    <button class="tab" class:active={activeTab === "general"} onclick={() => (activeTab = "general")}>
      General
    </button>
    <button class="tab" class:active={activeTab === "model"} onclick={() => (activeTab = "model")}>
      Model
    </button>
    <button class="tab" class:active={activeTab === "transcribe"} onclick={() => (activeTab = "transcribe")}>
      Transcribe
    </button>
    <button class="tab" class:active={activeTab === "advanced"} onclick={() => (activeTab = "advanced")}>
      Advanced
    </button>
  </div>

  {#if settings}
    <div class="content">
      {#if activeTab === "general"}
        <div class="field">
          <label for="language">Language</label>
          <select id="language" value={settings.language} onchange={onLanguageChange}>
            <option value="en">English</option>
            <option value="sv">Swedish</option>
            <option value="no">Norwegian</option>
            <option value="auto">Auto-detect</option>
          </select>
        </div>

        <div class="field">
          <label for="hotkey-mode">Hotkey Mode</label>
          <select id="hotkey-mode" value={settings.hotkey_mode} onchange={onHotkeyModeChange}>
            <option value="push">Push-to-talk</option>
            <option value="toggle">Toggle</option>
          </select>
        </div>

        <div class="field">
          <label>Hotkey</label>
          <div class="hotkey-display">{settings.hotkey}</div>
        </div>

        <div class="field-row">
          <label>Auto-paste transcription</label>
          <div
            class="toggle"
            class:active={settings.auto_paste}
            onclick={onAutoPasteToggle}
            role="switch"
            tabindex="0"
            aria-checked={settings.auto_paste}
          ></div>
        </div>

      {:else if activeTab === "model"}
        <div class="model-section-label">
          {languageLabel(settings.language)} models
        </div>

        <div class="model-picker">
          {#each models as model}
            <button
              class="model-card"
              class:active={model.active}
              class:downloading={downloading === model.id}
              onclick={() => selectModel(model)}
              disabled={downloading !== null}
            >
              <div class="model-card-header">
                <span class="model-card-name">{model.display_name}</span>
                {#if model.active}
                  <span class="model-badge active-badge">Active</span>
                {:else if !model.downloaded}
                  <span class="model-badge download-badge">{model.size_mb} MB</span>
                {/if}
              </div>
              <div class="model-card-desc">{model.description}</div>
              {#if downloading === model.id}
                <div class="progress-bar">
                  <div class="progress-fill" style="width: {downloadProgress}%"></div>
                </div>
              {/if}
            </button>
          {/each}
        </div>

        <div class="model-hint">
          Pick a size. Larger models are more accurate but take longer to transcribe.
          {#if models.some(m => !m.downloaded && !m.active)}
            Models are downloaded once and stored locally.
          {/if}
        </div>

      {:else if activeTab === "transcribe"}
        <div
          class="drop-zone"
          class:drag-over={dragOver}
          class:transcribing={transcribing}
        >
          {#if transcribing}
            <div class="spinner"></div>
            <div class="drop-zone-text">Transcribing...</div>
          {:else}
            <div class="drop-zone-icon">&#x1F4C1;</div>
            <div class="drop-zone-text">Drop an audio or video file here</div>
            <button class="primary open-file-btn" onclick={onPickFile}>
              Open File...
            </button>
          {/if}
        </div>

        <div class="formats-hint">
          Supported: {supportedFormats.map(f => f.toUpperCase()).join(", ") || "WAV, MP3, M4A, AAC, MP4, MOV, OGG, WEBM, FLAC"}
        </div>

        {#if transcribeError}
          <div class="transcribe-error">{transcribeError}</div>
        {/if}

        {#if transcriptionResult}
          <div class="result-label">Result</div>
          <textarea class="transcribe-result" readonly>{transcriptionResult}</textarea>
        {/if}

      {:else if activeTab === "advanced"}
        <div class="field-row">
          <label>Show recording overlay</label>
          <div
            class="toggle"
            class:active={settings.show_overlay}
            onclick={onShowOverlayToggle}
            role="switch"
            tabindex="0"
            aria-checked={settings.show_overlay}
          ></div>
        </div>

        <div class="field">
          <label>Version</label>
          <div class="version-text">
            {#if buildInfo}
              FlowDictate {buildInfo.version} ({buildInfo.git_hash}) - Built {buildInfo.build_date}
            {:else}
              FlowDictate 0.1.0
            {/if}
          </div>
        </div>
      {/if}
    </div>
  {:else}
    <div class="loading">Loading settings...</div>
  {/if}
</div>

<style>
  .settings-window {
    display: flex;
    flex-direction: column;
    height: 100vh;
  }

  .titlebar {
    padding: 16px 20px 8px;
    font-size: 16px;
    font-weight: 700;
    color: var(--text);
    -webkit-app-region: drag;
  }

  .tabs {
    display: flex;
    padding: 0 20px;
    gap: 4px;
    border-bottom: 1px solid var(--border);
  }

  .tab {
    padding: 8px 16px;
    background: transparent;
    color: var(--text-muted);
    border-radius: 6px 6px 0 0;
    font-size: 13px;
    border-bottom: 2px solid transparent;
    margin-bottom: -1px;
  }

  .tab.active {
    color: var(--accent);
    border-bottom-color: var(--accent);
  }

  .tab:hover:not(.active) {
    color: var(--text);
  }

  .content {
    padding: 20px;
    flex: 1;
    overflow-y: auto;
  }

  .hotkey-display {
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 8px 12px;
    font-family: monospace;
    font-size: 13px;
    color: var(--accent);
  }

  /* Model picker */

  .model-section-label {
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--text-muted);
    margin-bottom: 10px;
    font-weight: 600;
  }

  .model-picker {
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .model-card {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding: 14px 16px;
    background: var(--bg-secondary);
    border: 2px solid var(--border);
    border-radius: 10px;
    cursor: pointer;
    text-align: left;
    transition: border-color 0.15s, background 0.15s;
    width: 100%;
  }

  .model-card:hover:not(:disabled) {
    border-color: var(--text-muted);
  }

  .model-card.active {
    border-color: var(--accent);
    background: color-mix(in srgb, var(--accent) 8%, var(--bg-secondary));
  }

  .model-card:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .model-card-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .model-card-name {
    font-size: 15px;
    font-weight: 600;
    color: var(--text);
  }

  .model-card-desc {
    font-size: 12px;
    color: var(--text-muted);
  }

  .model-badge {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 10px;
    font-weight: 500;
  }

  .active-badge {
    background: color-mix(in srgb, var(--accent) 20%, transparent);
    color: var(--accent);
  }

  .download-badge {
    background: var(--bg);
    color: var(--text-muted);
    border: 1px solid var(--border);
  }

  .progress-bar {
    height: 4px;
    background: var(--border);
    border-radius: 2px;
    margin-top: 6px;
    overflow: hidden;
  }

  .progress-fill {
    height: 100%;
    background: var(--accent);
    border-radius: 2px;
    transition: width 0.2s;
  }

  .model-hint {
    font-size: 12px;
    color: var(--text-muted);
    line-height: 1.5;
    margin-top: 12px;
  }

  .version-text {
    color: var(--text-muted);
    font-size: 12px;
  }

  .loading {
    padding: 40px 20px;
    text-align: center;
    color: var(--text-muted);
  }

  /* Transcribe tab */

  .drop-zone {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    padding: 32px 20px;
    border: 2px dashed var(--border);
    border-radius: 12px;
    text-align: center;
    transition: border-color 0.2s, background 0.2s;
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

  .spinner {
    width: 28px;
    height: 28px;
    border: 3px solid var(--border);
    border-top-color: var(--accent);
    border-radius: 50%;
    animation: spin 0.8s linear infinite;
  }

  @keyframes spin {
    to { transform: rotate(360deg); }
  }

  .formats-hint {
    font-size: 11px;
    color: var(--text-muted);
    margin-top: 10px;
    text-align: center;
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

  .result-label {
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--text-muted);
    margin-top: 14px;
    margin-bottom: 6px;
    font-weight: 600;
  }

  .transcribe-result {
    width: 100%;
    min-height: 100px;
    max-height: 180px;
    padding: 10px 12px;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    color: var(--text);
    font-family: inherit;
    font-size: 13px;
    line-height: 1.5;
    resize: vertical;
    outline: none;
  }

  .transcribe-result:focus {
    border-color: var(--accent);
  }
</style>
