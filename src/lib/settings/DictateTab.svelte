<script lang="ts">
  import { tick } from "svelte";
  import { dictateButtonAction } from "../dictation-ui-state";
  import ErrorRecovery from "../ErrorRecovery.svelte";
  import { supportedBareFunctionKeyRange } from "../hotkey";
  import { displayProfileShortcut } from "../profile-shortcuts";
  import { languageLabel } from "./languageLabel";
  import { shortcutControls } from "./hotkeyProfiles.svelte";
  import type { SettingsWindow } from "./settingsWindow.svelte";

  let { win }: { win: SettingsWindow; active: boolean } = $props();
  // The window is one object for the life of the panel.
  // svelte-ignore state_referenced_locally
  const { session, profileModels, profiles, dictation, recovery, downloads: modelDownloads } = win;

  const settings = $derived(session.settings!); // the window renders panels only once the settings are loaded
  const downloading = $derived(modelDownloads.id);
  const downloadProgress = $derived(modelDownloads.progress);
  const updatePreparing = $derived(recovery.preparing);

  let hotkeyRecorderEl: HTMLButtonElement | undefined = $state();

  $effect(() => {
    if (profiles.recordingProfileId && profiles.recordingShortcutSlot && hotkeyRecorderEl) hotkeyRecorderEl.focus();
  });

  let testRecordButton: HTMLButtonElement | undefined = $state();

  /**
   * "Try again" under the Dictate tab's error: a new recording, whose button has
   * the focus. The Try again button goes away as the recording starts
   * (`clearTestError()`), so the focus moves first, and not only when
   * `start_recording` answers: it is never on the page in between. The record
   * button is enabled here: the cases that disable it are all part of the
   * busy state that disables Try again.
   */
  async function onDictateRetry(): Promise<void> {
    testRecordButton?.focus();
    await dictation.toggleRecording();
    await tick();
    testRecordButton?.focus();
  }
</script>

<div class="field profile-field">
  <div class="profile-heading">
    <span class="field-label">Dictation profiles · shortcut, language, model</span>
    <button class="link-btn" onclick={profiles.add} disabled={session.locked}>+ Add profile</button>
  </div>
  {#if session.profileModelNotices.length}
    <div class="profile-model-notice" role="status">
      <span>{session.profileModelNotices.join(" ")}</span>
      <button class="link-btn" type="button" aria-label="Dismiss notice" onclick={() => (session.profileModelNotices = [])}>Dismiss</button>
    </div>
  {/if}
  {#each settings.hotkey_profiles as profile (profile.id)}
    <div class="profile-card">
      <div class="profile-row">
        <input
          type="text"
          class="profile-name"
          aria-label="Profile name"
          value={profile.name}
          onblur={(event) => profiles.onNameBlur(event, profile)}
          disabled={session.locked}
        />
        <select
          aria-label={`${profile.name} language`}
          value={profile.language}
          onchange={(event) => profiles.onLanguageChange(event, profile)}
          disabled={session.locked}
        >
          <option value="en">English</option>
          <option value="sv">Swedish</option>
          <option value="no">Norwegian</option>
          <option value="fi">Finnish</option>
          <option value="da">Danish</option>
          <option value="auto">Auto-detect</option>
        </select>
      </div>
      <label class="profile-model-field">
        Speech model for {profile.name}
        <select
          aria-label={`Model for ${profile.name}`}
          value={settings.profile_models[profile.id] ?? "auto"}
          onchange={(event) => profileModels.change(profile, event)}
          disabled={profile.id === profiles.draftProfileId || profileModels.saving || dictation.backendState !== "idle" || session.locked}
        >
          <option value="auto">Recommended for {languageLabel(profile.language)}</option>
          {#each profileModels.options[profile.id] ?? [] as option (option.id)}
            <option value={option.id}>{option.display_name}</option>
          {/each}
        </select>
      </label>
      <div class="shortcut-grid">
        {#each shortcutControls as shortcutControl (shortcutControl.slot)}
          <div class="shortcut-control">
            <span class="shortcut-label">{shortcutControl.label}</span>
            {#if profiles.recordingProfileId === profile.id && profiles.recordingShortcutSlot === shortcutControl.slot}
              <button
                class="hotkey-recorder recording"
                bind:this={hotkeyRecorderEl}
                aria-label={`${profile.name} ${shortcutControl.label} shortcut`}
                onkeydown={(event) => profiles.onKeydown(event, profile.id, shortcutControl.slot)}
                onblur={() => profiles.abandonCapture()}>Press shortcut...</button
              >
            {:else}
              <button
                class="hotkey-recorder"
                aria-label={`${profile.name} ${shortcutControl.label} shortcut`}
                onclick={() => profiles.beginCapture(profile.id, shortcutControl.slot)}
                disabled={session.locked}
                >{profiles.formatHotkeyDisplay(
                  displayProfileShortcut(profile, shortcutControl.slot, settings.hotkey_mode) || "Not set",
                )}</button
              >
            {/if}
            <div class="shortcut-helper">{shortcutControl.helper}</div>
            {#if profile.push_to_talk_shortcut && profile.toggle_shortcut}
              <button
                type="button"
                class="shortcut-clear"
                aria-label={`Clear ${shortcutControl.label} shortcut for ${profile.name}`}
                onclick={() => profiles.clearShortcut(profile.id, shortcutControl.slot)}
                disabled={session.locked}>Clear</button
              >
            {/if}
          </div>
        {/each}
      </div>
      {#if settings.hotkey_profiles.length > 1}
        <button
          class="profile-remove"
          aria-label={`Remove ${profile.name}`}
          onclick={() => profiles.remove(profile.id)}
          disabled={session.locked}>Remove</button
        >
      {/if}
      {#if profileModels.models[profile.id]}
        <div class="profile-engine" class:missing={!profileModels.models[profile.id].downloaded}>
          <span>
            {profileModels.models[profile.id].downloaded
              ? `${profileModels.models[profile.id].display_name} · Ready`
              : `${profileModels.models[profile.id].display_name} required${profileModels.models[profile.id].size_mb > 0 ? ` · ${profileModels.models[profile.id].size_mb} MB` : ""}`}
          </span>
          {#if !profileModels.models[profile.id].downloaded}
            <button
              class="link-btn profile-engine-action"
              onclick={() => modelDownloads.downloadProfileModel(profile.id, profileModels.models[profile.id])}
              disabled={downloading !== null}
            >
              {downloading === profileModels.models[profile.id].id
                ? `Downloading ${Math.round(downloadProgress)}%`
                : "Download speech engine"}
            </button>
          {/if}
        </div>
        {#if profileModels.retiredSuggestion(profile)}
          {@const suggested = profileModels.retiredSuggestion(profile)!}
          <div class="profile-engine">
            <span>{suggested.display_name} is faster and more accurate for Swedish.</span>
            <button
              class="link-btn profile-engine-action"
              onclick={() => profileModels.switchToRecommended(profile)}
              disabled={downloading !== null || profileModels.saving || session.locked}
            >
              {downloading === suggested.id
                ? `Downloading ${Math.round(downloadProgress)}%`
                : `Switch to ${suggested.display_name}${suggested.downloaded || suggested.size_mb <= 0 ? "" : ` (download ${suggested.size_mb} MB)`}`}
            </button>
          </div>
        {/if}
        {#if modelDownloads.profileErrors[profile.id]}
          <div class="hotkey-error">{modelDownloads.profileErrors[profile.id]}</div>
        {/if}
      {/if}
    </div>
  {/each}
  {#if profiles.hotkeyError}
    <div class="hotkey-error">{profiles.hotkeyError}</div>
  {:else if !profiles.statusOk}
    <div class="hotkey-error">
      ⚠ Not registered{profiles.statusError ? `: ${profiles.statusError}` : ""}{#if win.platform === "macos"}
        — check Accessibility permission or whether another app uses this shortcut.{:else}
        — this shortcut may already be in use by another app. Try a different combination.{/if}
    </div>
  {/if}
  {#if profiles.brightnessKeyWarning}
    <div class="hotkey-error" role="status">⚠ {profiles.brightnessKeyWarning}</div>
  {/if}
  <div class="hotkey-hint">
    Each shortcut above is independent and either or both may be configured. Use a modifier ({profiles.modifierNames().meta}, {profiles.modifierNames()
      .ctrl}, {profiles.modifierNames().alt}, Shift) + key{#if supportedBareFunctionKeyRange(win.platform)}, or {supportedBareFunctionKeyRange(
        win.platform,
      )} by itself{/if}. {#if win.platform === "macos"}Bare F13–F24 requires Accessibility permission: macOS sends keyboard events to
      Sagascript, which immediately ignores everything except bare F13–F24 and never stores or sends them.{/if}
  </div>
</div>

<div class="field-row">
  <span class="field-label">Auto-paste transcription</span>
  <button
    type="button"
    class="toggle"
    class:active={settings.auto_paste}
    onclick={profiles.onAutoPasteToggle}
    role="switch"
    aria-checked={settings.auto_paste}
    aria-label="Auto-paste transcription"
    disabled={profiles.accessibilityChecking || session.locked}
  ></button>
</div>
<div class="hotkey-hint">Automatically paste dictated text into the active app when transcription finishes.</div>
{#if win.platform === "macos" && !profiles.accessibilityGranted && (settings.auto_paste || profiles.accessibilityRequested)}
  <div class="hotkey-error">
    Requires Accessibility permission. Auto-paste remains off until approved. <button
      class="link-btn"
      onclick={profiles.onAutoPasteToggle}
      disabled={profiles.accessibilityChecking || session.locked}
      >{profiles.accessibilityChecking ? "Checking…" : "Open System Settings"}</button
    >
  </div>
{/if}

<div class="test-section">
  <div class="test-section-label">Try it out</div>
  <button
    class="test-record-btn"
    bind:this={testRecordButton}
    class:recording={dictation.recording}
    class:transcribing={dictation.transcribing}
    onclick={dictation.toggleRecording}
    disabled={updatePreparing ||
      dictateButtonAction(dictation.backendState, dictation.ownsRecording) === "blocked" ||
      downloading !== null ||
      profileModels.saving}
  >
    {#if dictation.transcribing}
      <div class="spinner small"></div>
      {dictation.backendState === "loading_model" ? "Preparing speech engine..." : "Transcribing..."}
    {:else if downloading !== null}
      <div class="spinner small"></div>
      Downloading speech engine...
    {:else if dictation.recording}
      <div class="recording-dot"></div>
      {dictation.ownsRecording ? "Stop recording" : "Recording via hotkey..."}
    {:else}
      Start recording
    {/if}
  </button>
  {#if dictation.error}
    <div class="transcribe-error" role="alert">{dictation.error}</div>
    <ErrorRecovery
      recovery={dictation.errorRecovery}
      busy={updatePreparing ||
        modelDownloads.busy ||
        modelDownloads.repairing ||
        profileModels.saving ||
        dictateButtonAction(dictation.backendState, dictation.ownsRecording) !== "start"}
      onRetry={() => void onDictateRetry()}
      onDownloadModel={() => void dictation.downloadModelAgain()}
    />
  {/if}
  <textarea
    class="test-result"
    bind:value={dictation.text}
    oninput={() => {
      dictation.recoveryPending = true;
      dictation.edited = true;
    }}
    disabled={updatePreparing}
    placeholder="Click here and use your hotkey, or press the button above"></textarea>
  {#if dictation.text.trim()}
    <div class="result-actions">
      <button class="secondary" onclick={() => void dictation.copyResult()} disabled={updatePreparing}>Copy result</button>
      <button class="secondary" onclick={() => void dictation.saveResult()} disabled={updatePreparing}>Save result…</button>
      {#if dictation.actionMessage}<span role="status">{dictation.actionMessage}</span>{/if}
    </div>
  {/if}
</div>

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

  :global(.settings-locked) .toggle:disabled.active {
    background: var(--text-muted);
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

  .profile-heading,
  .profile-row {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .profile-heading {
    justify-content: space-between;
    margin-bottom: 8px;
  }

  .profile-card {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 10px;
    margin-bottom: 8px;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }

  .profile-name {
    min-width: 0;
    width: auto;
    flex: 1 1 52%;
    font-weight: 600;
  }

  .profile-row > select {
    min-width: 0;
    width: auto;
    flex: 1 1 48%;
  }

  .profile-model-field {
    display: flex;
    flex-direction: column;
    gap: 4px;
    color: var(--text-muted);
    font-size: 11px;
    font-weight: 600;
  }

  .profile-model-field select {
    width: 100%;
    color: var(--text);
  }

  .shortcut-grid {
    display: grid;
    grid-template-columns: repeat(2, minmax(0, 1fr));
    gap: 8px;
    align-items: end;
  }

  .shortcut-control {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .shortcut-label {
    color: var(--text-muted);
    font-size: 11px;
    font-weight: 600;
  }

  .shortcut-helper {
    color: var(--text-muted);
    font-size: 11px;
    line-height: 1.35;
  }

  .shortcut-clear {
    align-self: flex-start;
    background: transparent;
    border: none;
    cursor: pointer;
    color: var(--text-muted);
    font-size: 11px;
    padding: 0;
  }

  .shortcut-clear:hover {
    color: var(--text);
  }

  @media (max-width: 520px) {
    .shortcut-grid {
      grid-template-columns: 1fr;
    }
  }

  .profile-engine {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding-top: 3px;
    color: var(--text-muted);
    font-size: 11px;
  }

  .profile-engine.missing {
    color: var(--danger);
  }

  .profile-engine .link-btn:disabled {
    cursor: wait;
    opacity: 0.7;
  }

  .profile-engine-action {
    display: inline-block;
    flex: 0 0 152px;
    width: 152px;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    text-align: right;
  }

  .profile-remove {
    border: none;
    background: none;
    color: var(--danger);
    cursor: pointer;
    font-size: 11px;
  }

  @keyframes pulse-border {
    0%,
    100% {
      border-color: var(--accent);
    }
    50% {
      border-color: var(--border);
    }
  }

  .hotkey-error {
    margin-top: 4px;
    font-size: 11px;
    color: var(--danger);
  }

  .link-btn {
    background: none;
    border: none;
    color: var(--accent);
    font-size: inherit;
    padding: 0;
    cursor: pointer;
    text-decoration: underline;
  }

  .hotkey-hint {
    margin-top: 4px;
    font-size: 11px;
    color: var(--text-secondary, #888);
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
    to {
      transform: rotate(360deg);
    }
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

  .profile-model-notice {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 8px;
    padding: 8px 10px;
    border: 1px solid var(--accent);
    border-radius: var(--radius);
    font-size: 12px;
  }

  .profile-model-notice span {
    flex: 1;
    color: var(--text-muted);
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
    transition:
      border-color 0.15s,
      background 0.15s;
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
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.4;
    }
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

  .result-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    margin-top: 8px;
    font-size: 12px;
  }
</style>
