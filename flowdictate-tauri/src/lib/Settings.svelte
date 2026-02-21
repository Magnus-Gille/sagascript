<script lang="ts">
  import { onMount } from "svelte";
  import {
    getSettings,
    setLanguage,
    setHotkeyMode,
    setHotkey,
    setAutoPaste,
    setShowOverlay,
    setWhisperModel,
    getBuildInfo,
    getModelInfo,
    getLoadedModel,
    downloadModel,
    transcribeFile,
    getSupportedFormats,
    getPlatform,
    startRecording,
    stopAndTranscribe,
    type Settings,
    type BuildInfo,
    type Language,
    type HotkeyMode,
    type WhisperModel,
    type LoadedModelInfo,
  } from "./api";
  import { listen } from "@tauri-apps/api/event";
  import { open } from "@tauri-apps/plugin-dialog";
  import { getCurrentWebview } from "@tauri-apps/api/webview";

  let settings: Settings | null = $state(null);
  let buildInfo: BuildInfo | null = $state(null);
  let models: WhisperModel[] = $state([]);
  let loadedModel: LoadedModelInfo | null = $state(null);
  let activeTab: "dictate" | "transcribe" | "settings" = $state("dictate");
  let downloading: string | null = $state(null);
  let downloadingName: string = $state("");
  let downloadProgress: number = $state(0);

  let platform: string = $state("macos");

  // Hotkey recorder state
  let recordingHotkey: boolean = $state(false);
  let hotkeyError: string = $state("");
  let hotkeyRecorderEl: HTMLButtonElement | undefined = $state();

  $effect(() => {
    if (recordingHotkey && hotkeyRecorderEl) {
      hotkeyRecorderEl.focus();
    }
  });

  // Dictate test state
  let testRecording: boolean = $state(false);
  let testTranscribing: boolean = $state(false);
  let testResult: string = $state("");
  let testError: string = $state("");

  // Transcribe tab state
  let supportedFormats: string[] = $state([]);
  let transcribing: boolean = $state(false);
  let transcriptionResult: string = $state("");
  let transcribeError: string = $state("");
  let dragOver: boolean = $state(false);

  onMount(async () => {
    settings = await getSettings();
    platform = await getPlatform();
    buildInfo = await getBuildInfo();
    models = await getModelInfo();
    loadedModel = await getLoadedModel();
    supportedFormats = await getSupportedFormats();

    // Check URL params for initial tab
    const params = new URLSearchParams(window.location.search);
    const tab = params.get("tab");
    if (tab === "dictate" || tab === "transcribe" || tab === "settings") {
      activeTab = tab;
    }

    listen("model-download-progress", (event: any) => {
      downloadProgress = event.payload.progress;
    });

    listen("model-ready", async () => {
      downloading = null;
      downloadProgress = 0;
      models = await getModelInfo();
      loadedModel = await getLoadedModel();
    });

    // Listen for tab navigation from tray menu
    listen("navigate_tab", (event: any) => {
      const t = event.payload;
      if (t === "dictate" || t === "transcribe" || t === "settings") {
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
    loadedModel = await getLoadedModel();
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
        downloadingName = model.display_name;
        downloadProgress = 0;
        await downloadModel(model.id);
        // model_ready event will refresh the list
      }
      await setWhisperModel(model.id);
      settings = await getSettings();
      models = await getModelInfo();
      loadedModel = await getLoadedModel();
    } catch (e) {
      console.error("Model selection failed:", e);
    } finally {
      downloading = null;
      downloadProgress = 0;
    }
  }

  async function onTestRecord() {
    if (testRecording) {
      // Stop and transcribe
      testRecording = false;
      testTranscribing = true;
      testError = "";
      try {
        const text = await stopAndTranscribe();
        testResult = testResult ? testResult + " " + text : text;
      } catch (e: any) {
        testError = typeof e === "string" ? e : e.message || "Transcription failed";
      } finally {
        testTranscribing = false;
      }
    } else {
      // Start recording
      testError = "";
      try {
        await startRecording();
        testRecording = true;
      } catch (e: any) {
        testError = typeof e === "string" ? e : e.message || "Failed to start recording";
      }
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

  /** Map DOM key names to Tauri global-shortcut format */
  function tauriKeyName(key: string): string | null {
    if (key === " ") return "Space";
    if (key === "Meta") return null; // modifier only
    if (key === "Control") return null;
    if (key === "Alt") return null;
    if (key === "Shift") return null;
    // F-keys
    if (/^F\d{1,2}$/.test(key)) return key;
    // Single letter/digit
    if (/^[a-zA-Z0-9]$/.test(key)) return key.toUpperCase();
    // Arrow keys
    if (key.startsWith("Arrow")) return key;
    // Named keys
    const mapped: Record<string, string> = {
      Tab: "Tab", Enter: "Enter", Backspace: "Backspace", Delete: "Delete",
      Escape: "Escape", Home: "Home", End: "End", PageUp: "PageUp",
      PageDown: "PageDown", Insert: "Insert",
    };
    return mapped[key] ?? null;
  }

  /** Platform-correct modifier display names */
  function modifierNames(): { ctrl: string; alt: string; meta: string } {
    const mac = platform === "macos";
    return {
      ctrl: mac ? "Control" : "Ctrl",
      alt: mac ? "Option" : "Alt",
      meta: mac ? "Cmd" : "Win",
    };
  }

  /** Format a shortcut string for display (e.g. "Control+Shift+Space" → "Ctrl + Shift + Space") */
  function formatHotkeyDisplay(shortcut: string): string {
    const m = modifierNames();
    return shortcut
      .replace(/Control/g, m.ctrl)
      .replace(/Alt/g, m.alt)
      .replace(/Super/g, m.meta)
      .split("+")
      .join(" + ");
  }

  function onHotkeyKeydown(e: KeyboardEvent) {
    e.preventDefault();
    e.stopPropagation();

    // Escape cancels recording
    if (e.key === "Escape") {
      recordingHotkey = false;
      hotkeyError = "";
      return;
    }

    // Ignore bare modifier presses — wait for a non-modifier key
    if (["Control", "Shift", "Alt", "Meta"].includes(e.key)) return;

    const keyName = tauriKeyName(e.key);
    if (!keyName) {
      hotkeyError = `"${e.key}" is not a supported key. Use A–Z, 0–9, F1–F12, Space, Arrow keys, or Tab/Enter/Delete.`;
      return;
    }

    // Must have at least one modifier
    const hasModifier = e.ctrlKey || e.altKey || e.metaKey || e.shiftKey;
    if (!hasModifier) {
      const m = modifierNames();
      hotkeyError = `Shortcut must include a modifier (${m.ctrl}, ${m.alt}, ${m.meta}, or Shift)`;
      return;
    }

    // Build Tauri-format shortcut string (order: Control, Alt, Super, Shift, Key)
    // Note: muda crate uses "Super" (not "Meta") for Cmd on macOS / Win key on Windows
    const parts: string[] = [];
    if (e.ctrlKey) parts.push("Control");
    if (e.altKey) parts.push("Alt");
    if (e.metaKey) parts.push("Super");
    if (e.shiftKey) parts.push("Shift");
    parts.push(keyName);

    const shortcut = parts.join("+");
    hotkeyError = "";

    setHotkey(shortcut)
      .then(async () => {
        recordingHotkey = false;
        settings = await getSettings();
      })
      .catch((err: any) => {
        hotkeyError = typeof err === "string" ? err : err.message || "Failed to set hotkey";
      });
  }

  function languageLabel(lang: Language): string {
    switch (lang) {
      case "sv": return "Swedish";
      case "no": return "Norwegian";
      case "en": return "English";
      default: return "Auto-detect";
    }
  }

  function activeModelName(): string {
    if (loadedModel) return loadedModel.effective_model;
    const active = models.find(m => m.active);
    return active ? active.display_name : "Not selected";
  }

  function modelStatus(): string {
    if (!loadedModel) return "";
    if (!loadedModel.is_downloaded) return "Not downloaded";
    return "";
  }
</script>

<div class="settings-window">
  <div class="titlebar">Sagascript</div>

  <div class="tabs">
    <button class="tab" class:active={activeTab === "dictate"} onclick={() => (activeTab = "dictate")}>
      Dictate
    </button>
    <button class="tab" class:active={activeTab === "transcribe"} onclick={() => (activeTab = "transcribe")}>
      Transcribe
    </button>
    <button class="tab" class:active={activeTab === "settings"} onclick={() => (activeTab = "settings")}>
      Language & Model
    </button>
  </div>

  {#if settings}
    <div class="content">
      {#if activeTab === "dictate"}
        <button class="active-config-bar" onclick={() => (activeTab = "settings")}>
          <div class="active-config-row">
            <span class="active-config-label">Language</span>
            <span class="active-config-value">{languageLabel(settings.language)}</span>
          </div>
          <div class="active-config-row">
            <span class="active-config-label">Model</span>
            <span class="active-config-value">
              {activeModelName()}
              {#if modelStatus()}<span class="active-config-warn"> · {modelStatus()}</span>{/if}
            </span>
          </div>
          <span class="active-config-link">Change</span>
        </button>

        <div class="field">
          <label>Hotkey</label>
          {#if recordingHotkey}
            <button
              class="hotkey-recorder recording"
              bind:this={hotkeyRecorderEl}
              onkeydown={onHotkeyKeydown}
              onblur={() => { recordingHotkey = false; hotkeyError = ""; }}
            >
              Press shortcut...
            </button>
          {:else}
            <button
              class="hotkey-recorder"
              onclick={() => { recordingHotkey = true; hotkeyError = ""; }}
            >
              {formatHotkeyDisplay(settings.hotkey)}
            </button>
          {/if}
          {#if hotkeyError}
            <div class="hotkey-error">{hotkeyError}</div>
          {/if}
          <div class="hotkey-hint">Modifier ({modifierNames().meta}, {modifierNames().ctrl}, {modifierNames().alt}, Shift) + key (A–Z, 0–9, F1–F12, Space, arrows)</div>
        </div>

        <div class="field">
          <label for="hotkey-mode">Hotkey Mode</label>
          <select id="hotkey-mode" value={settings.hotkey_mode} onchange={onHotkeyModeChange}>
            <option value="push">Push-to-talk</option>
            <option value="toggle">Toggle</option>
          </select>
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

        <div class="test-section">
          <div class="test-section-label">Try it out</div>
          <button
            class="test-record-btn"
            class:recording={testRecording}
            class:transcribing={testTranscribing}
            onclick={onTestRecord}
            disabled={testTranscribing}
          >
            {#if testTranscribing}
              <div class="spinner small"></div>
              Transcribing...
            {:else if testRecording}
              <div class="recording-dot"></div>
              Stop recording
            {:else}
              Start recording
            {/if}
          </button>
          {#if testError}
            <div class="transcribe-error">{testError}</div>
          {/if}
          <textarea
            class="test-result"
            bind:value={testResult}
            placeholder="Click here and use your hotkey, or press the button above"
          ></textarea>
        </div>

      {:else if activeTab === "transcribe"}
        <button class="active-config-bar" onclick={() => (activeTab = "settings")}>
          <div class="active-config-row">
            <span class="active-config-label">Language</span>
            <span class="active-config-value">{languageLabel(settings.language)}</span>
          </div>
          <div class="active-config-row">
            <span class="active-config-label">Model</span>
            <span class="active-config-value">
              {activeModelName()}
              {#if modelStatus()}<span class="active-config-warn"> · {modelStatus()}</span>{/if}
            </span>
          </div>
          <span class="active-config-link">Change</span>
        </button>

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

      {:else if activeTab === "settings"}
        <div class="field">
          <label for="language">Language</label>
          <select id="language" value={settings.language} onchange={onLanguageChange}>
            <option value="en">English</option>
            <option value="sv">Swedish</option>
            <option value="no">Norwegian</option>
            <option value="auto">Auto-detect</option>
          </select>
        </div>

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
                {:else if model.downloaded}
                  <span class="model-badge ready-badge">Ready</span>
                {:else}
                  <span class="model-badge download-badge">Download · {model.size_mb} MB</span>
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

        <div class="field-row" style="margin-top: 20px;">
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
              Sagascript {buildInfo.version} ({buildInfo.git_hash}) - Built {buildInfo.build_date}
            {:else}
              Sagascript 0.1.0
            {/if}
          </div>
        </div>
      {/if}
    </div>
  {:else}
    <div class="loading">Loading settings...</div>
  {/if}

  {#if downloading}
    <div class="download-status-bar">
      <div class="download-status-info">
        <span class="download-status-label">Downloading {downloadingName}...</span>
        <span class="download-status-pct">{Math.round(downloadProgress)}%</span>
      </div>
      <div class="download-status-track">
        <div class="download-status-fill" style="width: {downloadProgress}%"></div>
      </div>
    </div>
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
  }

  .active-config-bar:hover {
    border-color: var(--text-muted);
  }

  .active-config-row {
    display: flex;
    flex-direction: column;
    gap: 1px;
    text-align: left;
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
  }

  .active-config-warn {
    color: var(--danger);
    font-size: 11px;
  }

  .active-config-link {
    font-size: 11px;
    color: var(--accent);
    font-weight: 500;
    margin-left: auto;
    flex-shrink: 0;
  }

  .hotkey-recorder {
    background: var(--bg-secondary);
    border: 2px solid var(--border);
    border-radius: var(--radius);
    padding: 8px 12px;
    font-family: monospace;
    font-size: 13px;
    color: var(--accent);
    cursor: pointer;
    text-align: left;
    width: 100%;
    transition: border-color 0.15s;
  }

  .hotkey-recorder:hover {
    border-color: var(--text-muted);
  }

  .hotkey-recorder.recording {
    border-color: var(--accent);
    animation: pulse-border 1.2s ease-in-out infinite;
    color: var(--text-muted);
  }

  @keyframes pulse-border {
    0%, 100% { border-color: var(--accent); }
    50% { border-color: var(--border); }
  }

  .hotkey-error {
    margin-top: 4px;
    font-size: 11px;
    color: var(--danger);
  }

  .hotkey-hint {
    margin-top: 4px;
    font-size: 11px;
    color: var(--text-secondary, #888);
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

  .ready-badge {
    background: color-mix(in srgb, var(--success, #34c759) 15%, transparent);
    color: var(--success, #34c759);
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

  /* Download status bar (bottom of window, visible on all tabs) */

  .download-status-bar {
    padding: 10px 20px 14px;
    border-top: 1px solid var(--border);
    background: var(--bg-secondary);
    flex-shrink: 0;
  }

  .download-status-info {
    display: flex;
    justify-content: space-between;
    align-items: center;
    margin-bottom: 6px;
  }

  .download-status-label {
    font-size: 12px;
    font-weight: 500;
    color: var(--text);
  }

  .download-status-pct {
    font-size: 12px;
    font-weight: 600;
    color: var(--accent);
    font-variant-numeric: tabular-nums;
  }

  .download-status-track {
    height: 6px;
    background: var(--border);
    border-radius: 3px;
    overflow: hidden;
  }

  .download-status-fill {
    height: 100%;
    background: var(--accent);
    border-radius: 3px;
    transition: width 0.2s;
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

  /* Test dictation section */

  .test-section {
    margin-top: 20px;
    padding-top: 16px;
    border-top: 1px solid var(--border);
  }

  .test-section-label {
    font-size: 12px;
    text-transform: uppercase;
    letter-spacing: 0.5px;
    color: var(--text-muted);
    margin-bottom: 10px;
    font-weight: 600;
  }

  .test-record-btn {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 8px;
    width: 100%;
    padding: 10px 16px;
    background: var(--bg-secondary);
    border: 2px solid var(--border);
    border-radius: var(--radius);
    color: var(--text);
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    transition: border-color 0.15s, background 0.15s;
  }

  .test-record-btn:hover:not(:disabled) {
    border-color: var(--text-muted);
  }

  .test-record-btn.recording {
    border-color: var(--danger);
    background: color-mix(in srgb, var(--danger) 8%, var(--bg-secondary));
    color: var(--danger);
  }

  .test-record-btn:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .recording-dot {
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--danger);
    animation: pulse-dot 1s ease-in-out infinite;
  }

  @keyframes pulse-dot {
    0%, 100% { opacity: 1; }
    50% { opacity: 0.4; }
  }

  .spinner.small {
    width: 14px;
    height: 14px;
    border-width: 2px;
  }

  .test-result {
    width: 100%;
    min-height: 60px;
    max-height: 120px;
    margin-top: 10px;
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

  .test-result:focus {
    border-color: var(--accent);
  }
</style>
