<script lang="ts">
  import {
    setBeamSize,
    setTemperatureFallback,
    setVadEnabled,
    setEnginePrewarm,
    setEngineIdleUnloadMinutes,
    type EnginePrewarm,
  } from "../api";
  import type { SettingsWindow } from "./settingsWindow.svelte";

  let { win }: { win: SettingsWindow; active: boolean } = $props();
  // The window is one object for the life of the panel.
  // svelte-ignore state_referenced_locally
  const { session } = win;
  // The section is part of the Settings tab: a change that fails here is reported there.
  // svelte-ignore state_referenced_locally
  const error = win.errors.of("settings");

  const settings = $derived(session.settings!); // the window renders panels only once the settings are loaded
  const engineIdleChoices = [5, 10, 30, 60, 0];

  async function onBeamSizeChange(e: Event) {
    const select = e.currentTarget as HTMLSelectElement;
    const value = Number(select.value);
    const saved = await session.applyScalar(() => setBeamSize(value), error);
    if (!saved && settings) select.value = String(settings.beam_size);
  }

  async function onTemperatureFallbackToggle() {
    if (!settings) return;
    const next = !settings.temperature_fallback;
    await session.applyScalar(() => setTemperatureFallback(next), error);
  }

  async function onEnginePrewarmChange(e: Event) {
    const select = e.currentTarget as HTMLSelectElement;
    const value = select.value as EnginePrewarm;
    const saved = await session.applyScalar(() => setEnginePrewarm(value), error);
    if (!saved && settings) select.value = settings.engine_prewarm;
  }

  async function onEngineIdleChange(e: Event) {
    const select = e.currentTarget as HTMLSelectElement;
    const value = Number(select.value);
    const saved = await session.applyScalar(() => setEngineIdleUnloadMinutes(value), error);
    if (!saved && settings) select.value = String(settings.engine_idle_unload_minutes);
  }

  async function onVadToggle() {
    if (!settings) return;
    const next = !settings.vad_enabled;
    await session.applyScalar(() => setVadEnabled(next), error);
  }
</script>

<details class="advanced-section">
  <summary>Advanced</summary>
  <div class="advanced-content">
    <p class="advanced-intro">
      Sagascript automatically chooses the recommended local model for each language. Change these controls only when you have a specific
      quality or performance need.
    </p>

    <div class="field advanced-field">
      <label for="beam-size">Decoding mode</label>
      <select id="beam-size" value={settings.beam_size} onchange={onBeamSizeChange} disabled={session.locked}>
        <option value={0}>Greedy (fast)</option>
        <option value={5}>Beam search (accurate)</option>
      </select>
    </div>

    <div class="field-row">
      <span class="field-label">Temperature fallback</span>
      <button
        type="button"
        class="toggle"
        class:active={settings.temperature_fallback}
        onclick={onTemperatureFallbackToggle}
        role="switch"
        aria-checked={settings.temperature_fallback}
        aria-label="Temperature fallback"
        disabled={session.locked}
      ></button>
    </div>
    <div class="hotkey-hint">Re-decode hard segments; off is faster but less robust.</div>

    {#if win.engineStatus?.supported}
      <div class="field advanced-field">
        <label for="engine-prewarm">Prepare Pianissimo when</label>
        <select id="engine-prewarm" value={settings.engine_prewarm} onchange={onEnginePrewarmChange} disabled={session.locked}>
          <option value="on_app_start">When Sagascript starts (recommended)</option>
          <option value="on_key_down">When I press the dictation key</option>
          <option value="off">Only when needed</option>
        </select>
      </div>
      <div class="hotkey-hint">Earlier preparation uses more memory but makes the first word faster.</div>

      <div class="field advanced-field">
        <label for="engine-idle">Unload after idle</label>
        <select id="engine-idle" value={settings.engine_idle_unload_minutes} onchange={onEngineIdleChange} disabled={session.locked}>
          {#each engineIdleChoices as minutes (minutes)}
            <option value={minutes}>{minutes === 0 ? "Never" : `${minutes} minutes`}</option>
          {/each}
          {#if !engineIdleChoices.includes(settings.engine_idle_unload_minutes)}
            <option value={settings.engine_idle_unload_minutes}>{settings.engine_idle_unload_minutes} minutes</option>
          {/if}
        </select>
      </div>
      <div class="hotkey-hint">Frees the Pianissimo model's memory after this much inactivity.</div>
    {/if}

    <div class="field-row advanced-toggle">
      <span class="field-label">Voice activity detection</span>
      <button
        type="button"
        class="toggle"
        class:active={settings.vad_enabled}
        onclick={onVadToggle}
        role="switch"
        aria-checked={settings.vad_enabled}
        aria-label="Voice activity detection"
        disabled={session.locked}
      ></button>
    </div>
    <div class="hotkey-hint">Skip silence; downloads a small model on first enable.</div>
  </div>
</details>

<style>
  :global(.content) > * {
    min-width: 0;
    max-width: 100%;
  }

  /* While the settings are unconfirmed, the controls that save a setting are disabled;
     make that visible (Settings.svelte, .settings-locked). Placed before the scoped
     :disabled rules below so those win where they set the same properties. */
  :global(.settings-locked) button:disabled,
  :global(.settings-locked) select:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  :global(.settings-locked) .toggle:disabled.active {
    background: var(--text-muted);
  }

  .hotkey-hint {
    margin-top: 4px;
    font-size: 11px;
    color: var(--text-secondary, #888);
  }

  /* Model picker */

  .advanced-section {
    margin: 22px 0 18px;
    border-top: 1px solid var(--border);
    border-bottom: 1px solid var(--border);
  }

  .advanced-section summary {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 13px 0;
    color: var(--text);
    font-weight: 600;
    cursor: pointer;
    list-style: none;
  }

  .advanced-section summary::-webkit-details-marker {
    display: none;
  }

  .advanced-section summary::after {
    content: "+";
    color: var(--text-muted);
    font-size: 18px;
    font-weight: 400;
  }

  .advanced-section[open] summary::after {
    content: "−";
  }

  .advanced-content {
    padding: 2px 0 18px;
  }

  .advanced-intro {
    margin-bottom: 16px;
    color: var(--text-muted);
    font-size: 12px;
    line-height: 1.55;
  }

  .advanced-field {
    margin-top: 20px;
  }

  .advanced-toggle {
    margin-top: 16px;
  }
</style>
