<script lang="ts">
  import { onMount } from "svelte";
  import {
    getSettings,
    setLanguage,
    setBackend,
    setHotkeyMode,
    setAutoSelectModel,
    setAutoPaste,
    setShowOverlay,
    saveApiKey,
    hasApiKey,
    deleteApiKey,
    getBuildInfo,
    type Settings,
    type BuildInfo,
    type Language,
    type TranscriptionBackend,
    type HotkeyMode,
  } from "./api";

  let settings: Settings | null = $state(null);
  let buildInfo: BuildInfo | null = $state(null);
  let activeTab: "general" | "transcription" | "advanced" = $state("general");
  let apiKeyInput = $state("");
  let hasKey = $state(false);
  let apiKeySaved = $state(false);

  onMount(async () => {
    settings = await getSettings();
    hasKey = await hasApiKey();
    buildInfo = await getBuildInfo();
  });

  async function onLanguageChange(e: Event) {
    const value = (e.target as HTMLSelectElement).value as Language;
    await setLanguage(value);
    settings = await getSettings();
  }

  async function onBackendChange(e: Event) {
    const value = (e.target as HTMLSelectElement).value as TranscriptionBackend;
    await setBackend(value);
    settings = await getSettings();
  }

  async function onHotkeyModeChange(e: Event) {
    const value = (e.target as HTMLSelectElement).value as HotkeyMode;
    await setHotkeyMode(value);
    settings = await getSettings();
  }

  async function onAutoSelectToggle() {
    if (!settings) return;
    await setAutoSelectModel(!settings.auto_select_model);
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

  async function onSaveApiKey() {
    if (apiKeyInput.trim()) {
      const ok = await saveApiKey(apiKeyInput.trim());
      if (ok) {
        hasKey = true;
        apiKeyInput = "";
        apiKeySaved = true;
        setTimeout(() => (apiKeySaved = false), 2000);
      }
    }
  }

  async function onDeleteApiKey() {
    await deleteApiKey();
    hasKey = false;
  }
</script>

<div class="settings-window">
  <div class="titlebar">FlowDictate Settings</div>

  <div class="tabs">
    <button
      class="tab"
      class:active={activeTab === "general"}
      onclick={() => (activeTab = "general")}
    >
      General
    </button>
    <button
      class="tab"
      class:active={activeTab === "transcription"}
      onclick={() => (activeTab = "transcription")}
    >
      Transcription
    </button>
    <button
      class="tab"
      class:active={activeTab === "advanced"}
      onclick={() => (activeTab = "advanced")}
    >
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

      {:else if activeTab === "transcription"}
        <div class="field">
          <label for="backend">Backend</label>
          <select id="backend" value={settings.backend} onchange={onBackendChange}>
            <option value="local">Local (whisper.cpp)</option>
            <option value="remote">Remote (OpenAI)</option>
          </select>
        </div>

        <div class="field-row">
          <label>Auto-select best model</label>
          <div
            class="toggle"
            class:active={settings.auto_select_model}
            onclick={onAutoSelectToggle}
            role="switch"
            tabindex="0"
            aria-checked={settings.auto_select_model}
          ></div>
        </div>

        {#if settings.backend === "remote"}
          <div class="field">
            <label for="api-key">OpenAI API Key</label>
            {#if hasKey}
              <div class="api-key-status">
                <span class="status-ok">Key configured</span>
                <button class="danger" onclick={onDeleteApiKey}>Remove</button>
              </div>
            {:else}
              <div class="api-key-input">
                <input
                  type="password"
                  id="api-key"
                  placeholder="sk-..."
                  bind:value={apiKeyInput}
                />
                <button class="primary" onclick={onSaveApiKey}>Save</button>
              </div>
              {#if apiKeySaved}
                <span class="status-ok">Saved!</span>
              {/if}
            {/if}
          </div>
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

  .api-key-status {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .api-key-input {
    display: flex;
    gap: 8px;
  }

  .api-key-input input {
    flex: 1;
  }

  .status-ok {
    color: var(--accent);
    font-size: 12px;
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
</style>
