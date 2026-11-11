<script lang="ts">
  import { tick } from "svelte";
  import { setLanguage, setProfileGlossary, setShowOverlay, type HotkeyProfile, type Language } from "../api";
  import { isBackendError, toErrorMessage } from "../errors";
  import AdvancedPanel from "./AdvancedPanel.svelte";
  import GlossaryEditor from "./GlossaryEditor.svelte";
  import { languageLabel } from "./languageLabel";
  import type { SettingsWindow } from "./settingsWindow.svelte";

  let { win, active }: { win: SettingsWindow; active: boolean } = $props();
  // The window is one object for the life of the panel.
  // svelte-ignore state_referenced_locally
  const { session, router, profileModels } = win;

  const settings = $derived(session.settings!); // the window renders panels only once the settings are loaded

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
</script>

{#if active}
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
          The <strong>default</strong> profile has the saved dictionary below. Its words belong to its current language, even when another dictionary
          is selected in the editor.
        </p>
        <textarea
          class="initial-prompt-input"
          rows="3"
          aria-label="Saved default profile dictionary"
          value={blockedLanguageChange.source}
          readonly></textarea>
        <p>
          Copy these words before continuing if you want to keep them. Clearing cannot be undone. Unsaved editor drafts will be preserved.
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
  {active}
  languageSaving={router.held}
  bind:settingsError={session.error}
  applySetting={session.apply}
  readSettings={session.read}
/>
{#if active}
  <AdvancedPanel {win} {active} />
{/if}

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
  :global(.settings-locked) textarea:disabled {
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
</style>
