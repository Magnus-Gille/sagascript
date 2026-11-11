<script lang="ts">
  import FileTranscription from "./FileTranscription.svelte";
  import { fileJobName } from "./transcription-queue";
  import { tick } from "svelte";
  import {
    setLanguage,
    setProfileGlossary,
    setShowOverlay,
    setBeamSize,
    setTemperatureFallback,
    setVadEnabled,
    setEnginePrewarm,
    setEngineIdleUnloadMinutes,
    type EnginePrewarm,
    type Language,
    type HotkeyProfile,
  } from "./api";
  import { isBackendError, toErrorMessage } from "./errors";
  import ErrorRecovery from "./ErrorRecovery.svelte";
  import { isPianissimoId } from "./models.svelte";
  import GlossaryEditor from "./settings/GlossaryEditor.svelte";
  import { languageLabel } from "./settings/languageLabel";
  import { shortcutControls } from "./settings/hotkeyProfiles.svelte";
  import { useSettingsWindow } from "./settings/settingsWindow.svelte";
  import { dictateButtonAction } from "./dictation-ui-state";
  import { supportedBareFunctionKeyRange } from "./hotkey";
  import { displayProfileShortcut } from "./profile-shortcuts";

  // The window's state and logic live in its controllers (settings/settingsWindow.svelte.ts),
  // which register their effects, listeners and mount callbacks here, as the component initializes.
  const win = useSettingsWindow();
  const { session, router, profileModels, profiles, dictation, queue, fileModel, recovery } = win;
  const modelDownloads = win.downloads;

  const settings = $derived(session.settings);
  const engineIdleChoices = [5, 10, 30, 60, 0];
  const downloading = $derived(modelDownloads.id);
  const downloadingName = $derived(modelDownloads.name);
  const downloadProgress = $derived(modelDownloads.progress);

  // Height of the sticky notice, so focus scrolling can leave room for it.
  let unconfirmedNoticeHeight = $state(0);
  let reloadButtonEl: HTMLButtonElement | undefined = $state();
  let windowTitleEl: HTMLHeadingElement | undefined = $state();
  /** A language change a personal dictionary blocks: the dictionary of `profileId`, previewed so that clearing it is a compare-and-swap. */
  let blockedLanguageChange: { language: Language; source: string; profileId: string } | null = $state(null);
  let languageSelectEl: HTMLSelectElement | undefined = $state();
  let languageRecoveryTitle: HTMLHeadingElement | undefined = $state();

  $effect(() => {
    if (blockedLanguageChange && languageRecoveryTitle) languageRecoveryTitle.focus();
  });

  async function restoreLanguageFocus(): Promise<void> {
    await tick();
    languageSelectEl?.focus();
  }

  function cancelLanguageChange(): void {
    blockedLanguageChange = null;
    void restoreLanguageFocus();
  }

  let hotkeyRecorderEl: HTMLButtonElement | undefined = $state();

  $effect(() => {
    if (profiles.recordingProfileId && profiles.recordingShortcutSlot && hotkeyRecorderEl) hotkeyRecorderEl.focus();
  });

  let testRecordButton: HTMLButtonElement | undefined = $state();
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

  // The personal dictionary editor (settings/GlossaryEditor.svelte) owns its
  // drafts and the dirty guard that requestTabChange asks before a tab change.
  let glossaryEditor: GlossaryEditor | undefined = $state();
  const glossarySaving = $derived(glossaryEditor?.isSaving() ?? false);
  router.guard = {
    requestLeave: (leave) => {
      if (glossaryEditor) glossaryEditor.requestLeave(leave);
      else leave();
    },
    isAsking: () => glossaryEditor?.isAsking() ?? false,
  };

  // When the lock engages, a focused control that just became disabled drops the
  // keyboard focus; hand it to Reload settings, the one thing to do next.
  let lockFocusHandled = false;
  $effect(() => {
    if (!session.locked) {
      lockFocusHandled = false;
      return;
    }
    if (lockFocusHandled) return;
    lockFocusHandled = true;
    void tick().then(() => {
      const active = document.activeElement;
      if (!active || active === document.body || active.matches(":disabled")) reloadButtonEl?.focus();
    });
  });

  async function reloadSettings(): Promise<void> {
    if (session.reloading) return;
    session.reloading = true;
    const hadFocus = document.activeElement === reloadButtonEl;
    try {
      await session.sync();
    } finally {
      session.reloading = false;
    }
    // The button leaves with the notice: keep a keyboard user's place in the window.
    if (hadFocus && !session.unconfirmed) {
      await tick();
      windowTitleEl?.focus();
    }
  }

  /**
   * The profile `set_language` changes, and so the one whose dictionary can block
   * the change: the profile called "default", else the first one
   * (`HotkeyProfile::default_index` in sagascript-core). The error carries only a
   * code, and "default" can be removed. A settings shape without profiles has the
   * implicit "default".
   */
  function languageProfileId(profiles: HotkeyProfile[]): string {
    return profiles.find((profile) => profile.id === "default")?.id ?? profiles[0]?.id ?? "default";
  }

  async function onLanguageChange(e: Event) {
    const select = e.currentTarget as HTMLSelectElement;
    const value = select.value as Language;
    if (!settings) return;
    // Native selects change before the async handler. A rejected value must
    // never remain visible merely because Svelte's saved value did not change.
    select.value = settings.language;
    if (router.held || glossarySaving) return;
    blockedLanguageChange = null;
    router.held = true;
    let saved = false;
    try {
      let rejection: unknown = null;
      const ok = await session.apply(() =>
        setLanguage(value).catch((error: unknown) => {
          rejection = error;
          throw error;
        }),
      );
      saved = ok;
      // The backend names a dictionary that blocks the change by its code
      // (src-tauri/crates/sagascript-core/src/settings/error.rs).
      if (!ok && isBackendError(rejection, "dictionary_blocks_language_change")) {
        // Show the actual blocking dictionary, not whichever editor scope is
        // selected. Freeze the preview for the existing compare-and-swap API.
        const current = await session.read();
        const profileId = languageProfileId(current.hotkey_profiles);
        const source = current.profile_glossaries[profileId] ?? "";
        if (source.trim()) blockedLanguageChange = { language: value, source, profileId };
        else session.error = "The blocking dictionary was cleared elsewhere. Select the language again to retry.";
      }
    } catch (error) {
      // The change was rejected, so nothing was saved; only the preview of the blocking dictionary is missing.
      if (session.restampUnconfirmed(false)) {
        // The failed read locked the window and the notice says so; the language select is disabled
        // until Reload settings, which also lifts the rejection message above.
        session.rejectionWithoutPreview = session.error;
      } else {
        session.error += `${session.error ? " " : ""}The window could not read the settings to show the blocking dictionary (${toErrorMessage(error, "unknown error")}). Select the language again to retry.`;
      }
    } finally {
      // A saved language whose read-back failed is shown as chosen: `settings` is
      // the old copy then, and the notice says it may be out of date.
      select.value = saved && session.unconfirmed ? value : settings.language;
      router.held = false;
      if (!blockedLanguageChange) void restoreLanguageFocus();
    }
  }

  // Calls the setters directly, past the unconfirmed hold of saveSetting: the
  // dictionary clear is a compare-and-swap against the preview's `expectedSource`,
  // so an old copy cannot overwrite a changed dictionary, and the language change
  // sends only the language. The language select that opens it and the button that
  // starts it are both disabled while unconfirmed anyway.
  async function clearDictionaryAndSwitchLanguage(): Promise<void> {
    if (!settings || !blockedLanguageChange || router.held || glossarySaving) return;
    const request = blockedLanguageChange;
    const { profileId } = request;
    const scopeDraft = glossaryEditor?.unsavedDraftOf(profileId) ?? null;
    router.held = true;
    session.error = "";
    let cleared = false;
    try {
      await setProfileGlossary(profileId, "", request.source);
      cleared = true;
      // Clear the confirmation immediately: a failed retry must not offer to
      // erase a dictionary that may have been populated again in the meantime.
      blockedLanguageChange = null;
      if (scopeDraft !== null) glossaryEditor?.preserveDraft(profileId, scopeDraft);
      session.settings = { ...settings, profile_glossaries: { ...settings.profile_glossaries, [profileId]: "" } };
      glossaryEditor?.discardChangesOf(profileId);
      const notices = await setLanguage(request.language);
      session.profileModelNotices = Array.isArray(notices) ? notices : [];
    } catch (error) {
      const message = toErrorMessage(error, "Unknown error");
      if (cleared) glossaryEditor?.preserveDraft(profileId, request.source);
      session.error = cleared
        ? `The default dictionary was cleared, but the language change failed: ${message}`
        : `The dictionary could not be cleared; the language was not changed: ${message}`;
      blockedLanguageChange = null;
    } finally {
      try {
        const current = await session.read();
        await profileModels.refresh(current.hotkey_profiles);
        session.restampUnconfirmed(cleared);
      } catch (error) {
        // The notice says what happened and offers Reload settings.
        session.markUnconfirmed(error, cleared);
      }
      router.held = false;
      void restoreLanguageFocus();
    }
  }

  async function onShowOverlayToggle() {
    if (!settings) return;
    const next = !settings.show_overlay;
    await session.applyScalar(() => setShowOverlay(next));
  }

  async function onBeamSizeChange(e: Event) {
    const select = e.currentTarget as HTMLSelectElement;
    const value = Number(select.value);
    const saved = await session.applyScalar(() => setBeamSize(value));
    if (!saved && settings) select.value = String(settings.beam_size);
  }

  async function onTemperatureFallbackToggle() {
    if (!settings) return;
    const next = !settings.temperature_fallback;
    await session.applyScalar(() => setTemperatureFallback(next));
  }

  async function onEnginePrewarmChange(e: Event) {
    const select = e.currentTarget as HTMLSelectElement;
    const value = select.value as EnginePrewarm;
    const saved = await session.applyScalar(() => setEnginePrewarm(value));
    if (!saved && settings) select.value = settings.engine_prewarm;
  }

  async function onEngineIdleChange(e: Event) {
    const select = e.currentTarget as HTMLSelectElement;
    const value = Number(select.value);
    const saved = await session.applyScalar(() => setEngineIdleUnloadMinutes(value));
    if (!saved && settings) select.value = String(settings.engine_idle_unload_minutes);
  }

  async function onVadToggle() {
    if (!settings) return;
    const next = !settings.vad_enabled;
    await session.applyScalar(() => setVadEnabled(next));
  }

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

<div class="settings-window" class:settings-locked={session.locked}>
  <header class="window-header">
    <h1 class="window-title" bind:this={windowTitleEl} tabindex="-1">Sagascript</h1>
    <div class="build-info" aria-label="Build information">
      {#if win.buildInfo}
        Version {win.buildInfo.version} · Build {win.buildInfo.git_hash} · {win.buildInfo.build_date}{win.engineHostIdentity
          ? ` · ${win.engineHostIdentity}`
          : ""}
      {:else}
        Version information unavailable
      {/if}
    </div>
  </header>

  <div class="tabs">
    <button class="tab" class:active={router.active === "dictate"} onclick={() => router.request("dictate")} disabled={router.held}>
      Dictate
    </button>
    <button class="tab" class:active={router.active === "transcribe"} onclick={() => router.request("transcribe")} disabled={router.held}>
      Transcribe
    </button>
    <button class="tab" class:active={router.active === "settings"} onclick={() => router.request("settings")} disabled={router.held}>
      Settings
    </button>
  </div>

  {#if settings}
    <div class="content" style:--notice-clearance={session.unconfirmed ? `${unconfirmedNoticeHeight + 12}px` : null}>
      {#if session.initError}
        <div class="transcribe-error">{session.initError}</div>
      {/if}
      {#if recovery.readError}
        <div class="transcribe-error" role="alert">{recovery.readError}</div>
      {/if}
      {#if session.error}
        <div class="transcribe-error" role={router.asking ? undefined : "alert"}>{session.error}</div>
      {/if}
      {#if session.unconfirmed}
        <section
          class="recovery-notice settings-unconfirmed"
          role="alert"
          aria-label="Settings not confirmed"
          bind:offsetHeight={unconfirmedNoticeHeight}
        >
          <span>
            {session.unconfirmed.afterSave
              ? "Your change was saved, but could not be read back to confirm it"
              : "The settings could not be re-read"}
            ({session.unconfirmed.detail}). Values shown may be out of date; reload before changing anything.
          </span>
          <button
            class="secondary"
            type="button"
            bind:this={reloadButtonEl}
            aria-disabled={session.reloading ? "true" : "false"}
            onclick={() => void reloadSettings()}>Reload settings</button
          >
        </section>
      {/if}
      {#if recovery.draftsNotice}
        <section class="recovery-notice" role="status" aria-label="Recovered drafts">
          <strong>Recovered drafts</strong>
          <span>Unsaved dictation and transcription results were restored after the update.</span>
          <button class="secondary" type="button" disabled={updatePreparing} onclick={() => void recovery.discardDrafts()}
            >Discard recovered drafts</button
          >
        </section>
      {/if}
      <p
        class="queue-summary"
        class:queue-empty={queue.jobs.length === 0}
        role="status"
        aria-label="File transcription status"
        aria-atomic="true"
      >
        {#if queue.jobs.length}
          {queue.jobs.filter((job) => job.status === "completed" && !queue.busy[job.id]).length} completed ·
          {queue.jobs.filter((job) => job.status === "failed").length} failed ·
          {queue.jobs.filter((job) => job.status === "cancelled").length} cancelled ·
          {queue.jobs.filter((job) => job.status === "queued").length} queued ·
          {queue.jobs.filter((job) => job.status === "running" || queue.busy[job.id]).length} running ·
          {queue.needingRetry.length} needs retry
        {/if}
      </p>
      {#if queue.needingRetry.length}
        <div class="queue-attention">
          {#each queue.needingRetry as job (job.id)}
            <div>
              <span
                >{fileJobName(job.path)} needs a status check.{#if queue.jobs.some((file) => file.status === "queued")}
                  The queue is paused.{/if}</span
              >
              <button class="secondary" onclick={() => queue.showNeedingRetry(job.id)}>Show {fileJobName(job.path)}</button>
            </div>
          {/each}
        </div>
      {/if}
      {#if router.active === "dictate"}
        <div class="field profile-field">
          <div class="profile-heading">
            <span class="field-label">Dictation profiles · shortcut, language, model</span>
            <button class="link-btn" onclick={profiles.add} disabled={session.locked}>+ Add profile</button>
          </div>
          {#if session.profileModelNotices.length}
            <div class="profile-model-notice" role="status">
              <span>{session.profileModelNotices.join(" ")}</span>
              <button class="link-btn" type="button" aria-label="Dismiss notice" onclick={() => (session.profileModelNotices = [])}
                >Dismiss</button
              >
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
                  disabled={profile.id === profiles.draftProfileId ||
                    profileModels.saving ||
                    dictation.backendState !== "idle" ||
                    session.locked}
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
              )} by itself{/if}. {#if win.platform === "macos"}Bare F13–F24 requires Accessibility permission: macOS sends keyboard events
              to Sagascript, which immediately ignores everything except bare F13–F24 and never stores or sends them.{/if}
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
      {/if}
      <section hidden={router.active !== "transcribe"} aria-label="File transcription">
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
                <option value={settings.file_transcription_model}
                  >Current choice is incompatible with {languageLabel(queue.language())}</option
                >
              {/if}
              {#each fileModel.options as model (model.id)}
                <option value={model.id}
                  >{model.display_name} · {model.size_mb} MB{model.downloaded ? " · ready" : " · download needed"}</option
                >
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
              <button
                class="secondary"
                onclick={() => void fileModel.downloadSelected()}
                disabled={downloading !== null || queue.transcribing}
              >
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
              active={router.active === "transcribe" && queue.selectedId === job.id}
            />
          </div>
        {/each}
      </section>

      {#if router.active === "settings"}
        <div class="field">
          <label for="language">Default profile language</label>
          <div class="hotkey-hint">This edits the default profile in Dictate. Other profiles keep their own language and model.</div>
          <select
            id="language"
            bind:this={languageSelectEl}
            value={settings.language}
            onchange={onLanguageChange}
            disabled={router.held || glossarySaving || session.locked}
          >
            <option value="en">English</option>
            <option value="sv">Swedish</option>
            <option value="no">Norwegian</option>
            <option value="fi">Finnish</option>
            <option value="da">Danish</option>
            <option value="auto">Auto-detect</option>
          </select>
          {#if blockedLanguageChange}
            <section class="language-recovery" aria-labelledby="language-recovery-title">
              <h3 id="language-recovery-title" bind:this={languageRecoveryTitle} tabindex="-1">
                Switch to {languageLabel(blockedLanguageChange.language)}?
              </h3>
              <p>
                The <strong>default</strong> profile has the saved dictionary below. Its words belong to its current language, even when another
                dictionary is selected in the editor.
              </p>
              <textarea
                class="initial-prompt-input"
                rows="3"
                aria-label="Saved default profile dictionary"
                value={blockedLanguageChange.source}
                readonly></textarea>
              <p>
                Copy these words before continuing if you want to keep them. Clearing cannot be undone. Unsaved editor drafts will be
                preserved.
              </p>
              <p>To keep this dictionary, cancel and use or add a language profile in the Dictate tab.</p>
              <div class="dictionary-actions">
                <button type="button" class="secondary" onclick={cancelLanguageChange} disabled={router.held}>Cancel</button>
                <button
                  type="button"
                  class="danger"
                  onclick={() => void clearDictionaryAndSwitchLanguage()}
                  disabled={router.held || glossarySaving || session.locked}
                >
                  {router.held ? "Switching…" : `Clear default dictionary and switch to ${languageLabel(blockedLanguageChange.language)}`}
                </button>
              </div>
            </section>
          {/if}
        </div>

        <div class="field-row">
          <span class="field-label">Show recording overlay</span>
          <button
            type="button"
            class="toggle"
            class:active={settings.show_overlay}
            onclick={onShowOverlayToggle}
            role="switch"
            aria-checked={settings.show_overlay}
            aria-label="Show recording overlay"
            disabled={session.locked}
          ></button>
        </div>
      {/if}
      <GlossaryEditor
        bind:this={glossaryEditor}
        {settings}
        active={router.active === "settings"}
        languageSaving={router.held}
        bind:settingsError={session.error}
        applySetting={session.apply}
        readSettings={session.read}
      />
      {#if router.active === "settings"}
        <details class="advanced-section">
          <summary>Advanced</summary>
          <div class="advanced-content">
            <p class="advanced-intro">
              Sagascript automatically chooses the recommended local model for each language. Change these controls only when you have a
              specific quality or performance need.
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
                <select
                  id="engine-idle"
                  value={settings.engine_idle_unload_minutes}
                  onchange={onEngineIdleChange}
                  disabled={session.locked}
                >
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
      {/if}
    </div>
  {:else}
    <div class="loading">
      {#if session.initError}
        <div class="transcribe-error">{session.initError}</div>
      {:else}
        Loading settings...
      {/if}
    </div>
  {/if}

  {#if downloading}
    <div class="download-status-bar">
      <div class="download-status-info">
        <span class="download-status-label">Downloading {downloadingName}...</span>
        <span class="download-status-pct">{Math.round(downloadProgress)}%</span>
      </div>
      <div class="download-status-track">
        <div class="download-status-fill" style:width="{downloadProgress}%"></div>
      </div>
    </div>
  {/if}
</div>

<style>
  .settings-window {
    display: flex;
    flex-direction: column;
    height: 100vh;
    min-width: 0;
  }

  /* While the settings are unconfirmed, the controls that save a setting are disabled;
     make that visible. Only then: other disabled states keep their look. Placed before the
     scoped :disabled rules below so those win where they set the same properties. */
  .settings-locked button:disabled,
  .settings-locked select:disabled,
  .settings-locked input:disabled,
  .settings-locked textarea:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .settings-locked .toggle:disabled.active {
    background: var(--text-muted);
  }

  .tabs {
    display: flex;
    padding: 12px 20px 0;
    gap: 4px;
    border-bottom: 1px solid var(--border);
  }

  .window-header {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 16px;
    flex-wrap: wrap;
    padding: 16px 20px 12px;
    border-bottom: 1px solid var(--border);
    min-width: 0;
  }

  /* Programmatic focus (after Reload settings) rings the title only for keyboard users. */
  .window-title:focus:not(:focus-visible) {
    outline: none;
  }

  .window-title {
    min-width: 0;
    font-size: 18px;
    line-height: 1.2;
    color: var(--text);
  }

  .build-info {
    min-width: 0;
    flex: 1 1 auto;
    max-width: 100%;
    overflow-wrap: anywhere;
    color: var(--text-muted);
    font-size: 11px;
    font-variant-numeric: tabular-nums;
    text-align: right;
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
    /* One value for the padding and for the sticky notice's offset inside it. */
    --content-pad: 20px;
    padding: var(--content-pad);
    /* While the notice is shown, focus scrolling keeps a control clear of it. */
    scroll-padding-top: var(--notice-clearance, 0px);
    flex: 1;
    overflow-y: auto;
    min-width: 0;
    max-width: 100%;
  }

  .window-header > *,
  .tabs > *,
  .content > * {
    min-width: 0;
    max-width: 100%;
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

  .initial-prompt-input {
    width: 100%;
    padding: 8px 10px;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    color: var(--text);
    font-family: inherit;
    font-size: 13px;
    line-height: 1.5;
    resize: vertical;
    outline: none;
    user-select: text;
    -webkit-user-select: text;
  }

  .initial-prompt-input:focus {
    border-color: var(--accent);
  }

  .dictionary-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    margin-top: 8px;
  }

  .dictionary-actions button:disabled {
    cursor: default;
    opacity: 0.6;
  }

  .loading {
    padding: 40px 20px;
    text-align: center;
    color: var(--text-muted);
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

  .recovery-notice {
    display: flex;
    align-items: center;
    gap: 10px;
    margin-bottom: 12px;
    padding: 10px 12px;
    border: 1px solid var(--accent);
    border-radius: var(--radius);
    font-size: 13px;
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

  /* The notice must stay in view while the user scrolls to the control they changed.
     Sticky offsets are measured inside the scroller's padding, so minus that padding
     sticks the notice flush with the scroller's top edge. The first shadow paints over
     the 12px margin below it, so scrolled content does not show in that gap. */
  .settings-unconfirmed {
    position: sticky;
    top: calc(-1 * var(--content-pad));
    z-index: 5;
    background: var(--bg);
    box-shadow:
      0 12px 0 0 var(--bg),
      0 18px 10px -6px rgba(0, 0, 0, 0.6);
    padding: 8px 12px;
    font-size: 12px;
    line-height: 1.4;
  }

  .recovery-notice span {
    flex: 1;
    color: var(--text-muted);
  }
  .recovery-notice button {
    flex-shrink: 0;
  }

  .queue-summary {
    font-size: 12px;
    color: var(--text-muted);
    margin-bottom: 12px;
    line-height: 1.6;
  }

  .queue-summary.queue-empty {
    margin: 0;
  }

  .queue-attention {
    border: 1px solid var(--accent);
    border-radius: var(--radius);
    padding: 10px 12px;
    margin-bottom: 12px;
    font-size: 13px;
  }

  .queue-attention > div {
    display: flex;
    align-items: center;
    gap: 12px;
    justify-content: space-between;
  }
  .queue-attention button {
    flex-shrink: 0;
  }

  .result-tabs {
    display: flex;
    gap: 6px;
    overflow-x: auto;
    margin: 18px 0 12px;
    padding-bottom: 4px;
  }
  .language-recovery {
    margin-top: 12px;
    padding: 14px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
  }

  .language-recovery h3 {
    margin: 0 0 8px;
    font-size: 14px;
  }

  .language-recovery p {
    margin: 8px 0;
    font-size: 12px;
    line-height: 1.5;
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
