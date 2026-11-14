<script lang="ts">
  // The personal dictionary editor of the Settings tab (SAGA-379): one
  // dictionary per dictation profile, edited as a local draft and saved
  // explicitly. The editor owns the draft, its baseline for the compare-and-swap
  // save, the drafts it preserved for manual recovery, and the dirty guard: while
  // changes are unsaved it asks (Save, Discard, Stay) before another profile's
  // dictionary is shown and before the window leaves the Settings tab
  // (`requestLeave`).
  //
  // Settings keeps it mounted while another tab shows (`active` is false then and
  // only the question can render), so the selected profile and the preserved
  // drafts survive a tab change.
  import { setProfileGlossary, type HotkeyProfile, type Settings } from "../api";
  import { languageLabel } from "./languageLabel";

  let {
    settings,
    active,
    languageSaving,
    settingsError = $bindable(),
    applySetting,
    readSettings,
  }: {
    settings: Settings;
    /** The Settings tab is showing. */
    active: boolean;
    /** A language change is in flight: nothing here saves, discards or navigates meanwhile. */
    languageSaving: boolean;
    /** The window's error banner. A dictionary conflict is "shown" while it still holds that rejection's message. */
    settingsError: string;
    /** The window's save-then-read-back (Settings.svelte `applySetting`); resolves true when the change is saved. */
    applySetting: (
      mutate: () => Promise<void | string[]>,
      errorSink?: { value: string; code: string | null },
      reportError?: boolean,
    ) => Promise<boolean>;
    /** Read the persisted settings and show them. */
    readSettings: () => Promise<Settings>;
  } = $props();

  // Every editable dictionary belongs to a named dictation profile.
  let glossaryScopeId: string = $state("default");
  let glossaryDraft: string = $state("");
  let glossaryDraftInitialized = false;
  let glossaryScopeGeneration = $state(0);
  let glossaryDraftGeneration = $state(0);
  let lastStoredGlossarySources: Record<string, string> = {};
  let glossaryEditBaseline: { scopeId: string; source: string; generation: number } | null = $state(null);
  let glossarySaving: boolean = $state(false);
  let glossarySaveInFlight: Promise<boolean> | null = null;
  type PendingGlossaryNavigation = { kind: "scope"; scopeId: string } | { kind: "leave"; leave: () => void };
  let pendingGlossaryNavigation: PendingGlossaryNavigation | null = $state(null);
  let glossaryDialogEl: HTMLDivElement | undefined = $state();
  let glossaryReturnFocusEl: HTMLElement | null = null;
  type RecoveredGlossaryDraft = { scopeId: string; draft: string; conflicted: boolean };
  type GlossarySaveRequest = {
    scopeId: string;
    generation: number;
    draftGeneration: number;
    value: string;
  };
  let recoveredGlossaryDrafts: RecoveredGlossaryDraft[] = $state([]);
  let glossaryConflictScopeId: string | null = $state(null);

  // The backend rejects a dictionary save whose baseline went stale with the
  // code below (ADR 0006); the code decides, never the message text. The
  // conflict is "shown" while `settingsError` still holds the message of that
  // rejection; a different error replaces it.
  const dictionaryConflictCode = "dictionary_changed_elsewhere";
  let dictionaryConflictMessage: string | null = $state(null);

  function isDictionaryConflictShown(): boolean {
    return dictionaryConflictMessage !== null && settingsError === dictionaryConflictMessage;
  }

  // The dictionary editor is intentionally local state. Settings reloads may
  // update the saved source, but must never replace a draft that differs from
  // the source we last saw for this scope.
  function glossaryHasUnsavedChanges(): boolean {
    if (glossarySaving) return true;
    const baseline = glossaryEditBaseline;
    const draftChanged = baseline?.scopeId === glossaryScopeId && glossaryDraft !== baseline.source;
    const conflict = glossaryConflictScopeId === glossaryScopeId && isDictionaryConflictShown();
    return Boolean(draftChanged || conflict);
  }

  $effect(() => {
    if (!pendingGlossaryNavigation || !glossaryDialogEl) return;
    queueMicrotask(() => {
      if (glossarySaving) {
        glossaryDialogEl?.focus();
      } else {
        glossaryDialogEl?.querySelector<HTMLButtonElement>("[data-dialog-stay]")?.focus();
      }
    });
  });

  function profileForId(profileId: string, source: Settings = settings): HotkeyProfile | null {
    if (!profileId) return null;
    return source.hotkey_profiles.find((profile) => profile.id === profileId) ?? null;
  }

  function glossarySourceForScope(scopeId: string, source: Settings = settings): string {
    return source.profile_glossaries[scopeId] ?? "";
  }

  function isValidGlossaryScope(scopeId: string, source: Settings = settings): boolean {
    return profileForId(scopeId, source) !== null;
  }

  function glossaryScopeLabel(scopeId: string, source: Settings = settings): string {
    return profileForId(scopeId, source)?.name ?? scopeId;
  }

  function rememberGlossaryRecovery(scopeId: string, draft: string, conflicted = false): void {
    const existing = recoveredGlossaryDrafts.find((recovery) => recovery.scopeId === scopeId && recovery.draft === draft);
    if (existing) {
      if (conflicted && !existing.conflicted) {
        recoveredGlossaryDrafts = recoveredGlossaryDrafts.map((recovery) =>
          recovery === existing ? { ...recovery, conflicted: true } : recovery,
        );
      }
      return;
    }
    recoveredGlossaryDrafts = [...recoveredGlossaryDrafts, { scopeId, draft, conflicted }];
  }

  function removeGlossaryRecovery(scopeId: string, draft: string): void {
    recoveredGlossaryDrafts = recoveredGlossaryDrafts.filter((recovery) => recovery.scopeId !== scopeId || recovery.draft !== draft);
  }

  function isCurrentGlossaryRequest(request: GlossarySaveRequest): boolean {
    return (
      request.generation === glossaryScopeGeneration &&
      request.scopeId === glossaryScopeId &&
      request.draftGeneration === glossaryDraftGeneration
    );
  }

  // Settings can be reloaded after hotkey/profile changes. Never leave a
  // removed profile selected, and never show another scope's text after that
  // reconciliation.
  $effect(() => {
    const currentSettings = settings;
    const currentScope = glossaryScopeId;

    const currentStored = glossarySourceForScope(currentScope, currentSettings);
    const previousStored = lastStoredGlossarySources[currentScope];
    if (!glossaryDraftInitialized || glossaryDraft === previousStored) {
      glossaryDraft = currentStored;
      glossaryDraftInitialized = true;
    }
    lastStoredGlossarySources[currentScope] = currentStored;
    if (!isValidGlossaryScope(currentScope, currentSettings)) {
      const removedProfileHasDraft =
        glossaryEditBaseline?.scopeId === currentScope || (glossaryDraftInitialized && glossaryDraft !== (previousStored ?? currentStored));
      if (removedProfileHasDraft) {
        rememberGlossaryRecovery(currentScope, glossaryDraft, glossaryConflictScopeId === currentScope && isDictionaryConflictShown());
      }
      glossaryScopeGeneration += 1;
      glossaryScopeId = currentSettings.hotkey_profiles[0]?.id ?? "default";
      glossaryDraft = glossarySourceForScope(glossaryScopeId, currentSettings);
      glossaryDraftGeneration += 1;
      glossaryEditBaseline = null;
      glossaryConflictScopeId = null;
    }
  });

  async function refreshDictionaryAfterConflict(primaryError: string, request: GlossarySaveRequest) {
    try {
      await readSettings();
    } catch (error) {
      console.warn("Could not refresh the dictionary after a concurrent change", error);
    }
    // A stale request still refreshes the source of truth, but cannot replace
    // a newer scope's error or draft. The recovery item carries its context.
    if (isCurrentGlossaryRequest(request)) {
      settingsError = primaryError;
      glossaryConflictScopeId = request.scopeId;
    } else {
      rememberGlossaryRecovery(request.scopeId, request.value, true);
    }
  }

  async function saveGlossary(): Promise<boolean> {
    if (languageSaving) return false;
    if (glossarySaveInFlight) return glossarySaveInFlight;

    const request: GlossarySaveRequest = {
      scopeId: glossaryScopeId,
      generation: glossaryScopeGeneration,
      draftGeneration: glossaryDraftGeneration,
      value: glossaryDraft,
    };
    const scopeId = request.scopeId;
    const draftGeneration = request.draftGeneration;
    const value = request.value;
    const editBaseline = glossaryEditBaseline;
    const expectedSource =
      editBaseline?.scopeId === scopeId && editBaseline.generation <= draftGeneration
        ? editBaseline.source
        : (lastStoredGlossarySources[scopeId] ?? glossarySourceForScope(scopeId));

    if (!isValidGlossaryScope(scopeId)) return false;
    if (!glossaryHasUnsavedChanges()) {
      if (glossaryEditBaseline?.scopeId === scopeId) glossaryEditBaseline = null;
      return true;
    }

    const saveError: { value: string; code: string | null } = { value: "", code: null };
    const operation = (async (): Promise<boolean> => {
      glossarySaving = true;
      settingsError = "";
      try {
        const saved = await applySetting(() => setProfileGlossary(scopeId, value, expectedSource), saveError, false);
        const conflict = saveError.code === dictionaryConflictCode;
        let requestIsCurrent = isCurrentGlossaryRequest(request);
        if (conflict) {
          dictionaryConflictMessage = saveError.value;
          await refreshDictionaryAfterConflict(saveError.value, request);
          requestIsCurrent = isCurrentGlossaryRequest(request);
        } else if (!saved && !requestIsCurrent) {
          rememberGlossaryRecovery(scopeId, value);
        }

        if (requestIsCurrent) {
          settingsError = saved ? "" : saveError.value;
          if (saved) glossaryConflictScopeId = null;
        }
        if (saved) removeGlossaryRecovery(scopeId, value);

        // If our own save won the CAS race while the user kept typing in the
        // same edit lineage, advance only that lineage's baseline to our
        // value. A stale request never clears a newer draft.
        if (saved && editBaseline && glossaryEditBaseline === editBaseline && editBaseline.scopeId === scopeId) {
          if (draftGeneration === glossaryDraftGeneration) {
            glossaryEditBaseline = null;
          } else {
            glossaryEditBaseline = { ...editBaseline, source: value };
          }
        }

        // A scope removal/reload while the invoke was pending owns the
        // textarea now; never navigate based on a stale request.
        return saved && requestIsCurrent;
      } finally {
        glossarySaving = false;
      }
    })();
    glossarySaveInFlight = operation;
    try {
      return await operation;
    } finally {
      if (glossarySaveInFlight === operation) glossarySaveInFlight = null;
    }
  }

  function onGlossaryInput(e: Event) {
    if (!glossaryEditBaseline || glossaryEditBaseline.scopeId !== glossaryScopeId) {
      glossaryEditBaseline = {
        scopeId: glossaryScopeId,
        source: lastStoredGlossarySources[glossaryScopeId] ?? glossarySourceForScope(glossaryScopeId),
        generation: glossaryDraftGeneration + 1,
      };
    }
    glossaryDraftGeneration += 1;
    glossaryDraft = (e.target as HTMLTextAreaElement).value;
  }

  function discardGlossaryChanges(): void {
    if (glossarySaving) return;
    const currentSource = glossarySourceForScope(glossaryScopeId, settings);
    glossaryDraftGeneration += 1;
    glossaryDraft = currentSource;
    glossaryDraftInitialized = true;
    lastStoredGlossarySources[glossaryScopeId] = currentSource;
    glossaryEditBaseline = null;
    if (glossaryConflictScopeId === glossaryScopeId) {
      glossaryConflictScopeId = null;
      if (isDictionaryConflictShown()) settingsError = "";
    }
  }

  function commitGlossaryScopeChange(nextScope: string): void {
    if (!isValidGlossaryScope(nextScope)) return;
    glossaryScopeGeneration += 1;
    glossaryDraftGeneration += 1;
    glossaryEditBaseline = null;
    glossaryConflictScopeId = null;
    settingsError = isDictionaryConflictShown() ? "" : settingsError;
    glossaryScopeId = nextScope;
    glossaryDraft = glossarySourceForScope(nextScope, settings);
    glossaryDraftInitialized = true;
    lastStoredGlossarySources[nextScope] = glossaryDraft;
  }

  function onGlossaryScopeChange(e: Event) {
    const nextScope = (e.target as HTMLSelectElement).value;
    if (!isValidGlossaryScope(nextScope)) return;
    if (nextScope === glossaryScopeId) return;
    if (glossaryHasUnsavedChanges()) {
      // The browser changes a select's displayed value before onchange fires.
      // Restore the current scope until the user makes an explicit decision.
      (e.currentTarget as HTMLSelectElement).value = glossaryScopeId;
      promptGlossaryNavigation({ kind: "scope", scopeId: nextScope }, e.currentTarget as HTMLElement);
      return;
    }
    commitGlossaryScopeChange(nextScope);
  }

  function promptGlossaryNavigation(pending: PendingGlossaryNavigation, returnFocusEl?: HTMLElement): void {
    const activeElement = returnFocusEl ?? (typeof document === "undefined" ? null : document.activeElement);
    glossaryReturnFocusEl = activeElement instanceof HTMLElement ? activeElement : null;
    pendingGlossaryNavigation = pending;
  }

  function restoreGlossaryNavigationFocus(): void {
    const returnFocusEl = glossaryReturnFocusEl;
    glossaryReturnFocusEl = null;
    queueMicrotask(() => {
      if (returnFocusEl?.isConnected && !returnFocusEl.matches(":disabled")) {
        returnFocusEl.focus();
      }
    });
  }

  function onGlossaryDialogKeydown(event: KeyboardEvent): void {
    if (event.key === "Escape") {
      event.preventDefault();
      event.stopPropagation();
      if (!glossarySaving) stayOnGlossaryDraft();
      return;
    }
    if (event.key !== "Tab") return;

    const dialog = glossaryDialogEl;
    if (!dialog) return;
    const focusable = Array.from(
      dialog.querySelectorAll<HTMLElement>(
        "button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex='-1'])",
      ),
    ).filter((element) => !element.matches(":disabled"));
    event.preventDefault();
    if (focusable.length === 0) {
      dialog.focus();
      return;
    }

    const activeElement = typeof document === "undefined" ? null : document.activeElement;
    const activeIndex = activeElement ? focusable.indexOf(activeElement as HTMLElement) : -1;
    const nextIndex =
      activeIndex < 0
        ? event.shiftKey
          ? focusable.length - 1
          : 0
        : (activeIndex + (event.shiftKey ? -1 : 1) + focusable.length) % focusable.length;
    focusable[nextIndex]?.focus();
  }

  function finishPendingGlossaryNavigation(pending: PendingGlossaryNavigation): void {
    pendingGlossaryNavigation = null;
    restoreGlossaryNavigationFocus();
    if (pending.kind === "scope") {
      commitGlossaryScopeChange(pending.scopeId);
    } else {
      pending.leave();
    }
  }

  async function saveAndFinishGlossaryNavigation(): Promise<void> {
    const pending = pendingGlossaryNavigation;
    if (!pending || glossarySaving || languageSaving) return;
    if ((await saveGlossary()) && pendingGlossaryNavigation === pending) {
      finishPendingGlossaryNavigation(pending);
    }
  }

  function discardAndFinishGlossaryNavigation(): void {
    const pending = pendingGlossaryNavigation;
    if (!pending || glossarySaving || languageSaving) return;
    discardGlossaryChanges();
    finishPendingGlossaryNavigation(pending);
  }

  function stayOnGlossaryDraft(): void {
    pendingGlossaryNavigation = null;
    restoreGlossaryNavigationFocus();
  }

  // What the window needs from the editor (`bind:this` in Settings.svelte).

  /**
   * The dirty guard for the tab router. With nothing unsaved `leave` runs at
   * once. With unsaved changes the editor asks first, and `leave` runs only
   * after Save succeeded or after Discard; Stay drops it. While a save is in
   * flight nothing happens.
   */
  export function requestLeave(leave: () => void): void {
    if (glossarySaving) return;
    if (glossaryHasUnsavedChanges()) {
      promptGlossaryNavigation({ kind: "leave", leave });
      return;
    }
    leave();
  }

  export function hasUnsavedChanges(): boolean {
    return glossaryHasUnsavedChanges();
  }

  export function isSaving(): boolean {
    return glossarySaving;
  }

  /** The Save, Discard or Stay question is open. */
  export function isAsking(): boolean {
    return pendingGlossaryNavigation !== null;
  }

  /** The unsaved draft of `profileId`'s dictionary, when that is the one being edited. */
  export function unsavedDraftOf(profileId: string): string | null {
    return glossaryScopeId === profileId && glossaryHasUnsavedChanges() ? glossaryDraft : null;
  }

  /** Keep `draft` on show for manual recovery: its dictionary was changed without it. */
  export function preserveDraft(scopeId: string, draft: string): void {
    rememberGlossaryRecovery(scopeId, draft);
  }

  /** Show the saved dictionary of `profileId` again, when that is the one being edited. */
  export function discardChangesOf(profileId: string): void {
    if (glossaryScopeId === profileId) discardGlossaryChanges();
  }
</script>

{#if active}
  <div class="field">
    <div class="dictionary-heading">
      <label for="initial-prompt">Personal dictionary</label>
      {#if glossaryHasUnsavedChanges()}
        <span class="unsaved-indicator" role="status">Unsaved changes</span>
      {/if}
    </div>
    <select id="dictionary-scope" value={glossaryScopeId} onchange={onGlossaryScopeChange} disabled={glossarySaving || languageSaving}>
      {#each settings.hotkey_profiles as profile (profile.id)}
        <option value={profile.id}>{profile.name} · {languageLabel(profile.language)}</option>
      {/each}
    </select>
    <textarea
      id="initial-prompt"
      class="initial-prompt-input"
      rows="5"
      value={glossaryDraft}
      oninput={onGlossaryInput}
      disabled={glossarySaving || languageSaving}
      placeholder="OpenRouter = open router | open vrouter&#10;merge = merch&#10;Cloudflare = cloud flare"></textarea>
    <div class="dictionary-actions">
      <button
        type="button"
        class="primary"
        onclick={() => void saveGlossary()}
        disabled={!glossaryHasUnsavedChanges() || glossarySaving || languageSaving}>{glossarySaving ? "Saving…" : "Save changes"}</button
      >
      <button
        type="button"
        class="secondary"
        onclick={discardGlossaryChanges}
        disabled={!glossaryHasUnsavedChanges() || glossarySaving || languageSaving}>Discard changes</button
      >
    </div>
    <div class="hotkey-hint glossary-migration">
      This profile owns its dictionary. Save changes explicitly; switching profiles never moves entries to another dictionary.
    </div>
    {#if glossaryConflictScopeId === glossaryScopeId && isDictionaryConflictShown()}
      <div class="hotkey-hint glossary-migration">
        This dictionary changed elsewhere. Your draft is preserved; copy it if needed, then switch scopes and reselect this scope to reload
        the saved value. If it still shows the old text, close and reopen Settings.
      </div>
    {/if}
    {#if recoveredGlossaryDrafts.length > 0}
      <div class="hotkey-hint glossary-migration">
        Unsaved drafts are preserved below for manual recovery. They are never saved or copied automatically into another dictionary.
      </div>
      {#each recoveredGlossaryDrafts as recovery (recovery.scopeId + "\u0000" + recovery.draft)}
        <div class="glossary-recovery">
          <div class="hotkey-hint">
            <strong>Unsaved draft</strong> for <code>{glossaryScopeLabel(recovery.scopeId)}</code>
            {#if recovery.conflicted}
              — the saved dictionary changed elsewhere.{/if}
          </div>
          <textarea
            class="initial-prompt-input"
            rows="3"
            aria-label={`Unsaved draft for ${glossaryScopeLabel(recovery.scopeId)}`}
            value={recovery.draft}
            readonly></textarea>
        </div>
      {/each}
    {/if}
    <div class="hotkey-hint">
      One preferred spelling per line. Add exact mishearings after <code>=</code>, separated by <code>|</code>. Plain terms still guide
      Whisper. Save explicitly to use changes for live dictation and batch jobs.
    </div>
  </div>
{/if}

{#if pendingGlossaryNavigation}
  <div class="dialog-backdrop">
    <div
      class="unsaved-dialog"
      bind:this={glossaryDialogEl}
      role="dialog"
      aria-modal="true"
      aria-labelledby="unsaved-dialog-title"
      aria-describedby="unsaved-dialog-description"
      tabindex="-1"
      onkeydown={onGlossaryDialogKeydown}
    >
      <h2 id="unsaved-dialog-title">Unsaved dictionary changes</h2>
      <p id="unsaved-dialog-description">
        {pendingGlossaryNavigation.kind === "scope"
          ? `Save changes before switching to ${glossaryScopeLabel(pendingGlossaryNavigation.scopeId)}?`
          : "Save changes before leaving Settings?"}
      </p>
      {#if settingsError}
        <div class="hotkey-error" role="alert">{settingsError}</div>
      {/if}
      <div class="dialog-actions">
        <button
          type="button"
          class="primary"
          onclick={() => void saveAndFinishGlossaryNavigation()}
          disabled={glossarySaving || languageSaving}>{glossarySaving ? "Saving…" : "Save"}</button
        >
        <button type="button" class="danger" onclick={discardAndFinishGlossaryNavigation} disabled={glossarySaving || languageSaving}
          >Discard</button
        >
        <button type="button" class="secondary" data-dialog-stay onclick={stayOnGlossaryDraft} disabled={glossarySaving}>Stay</button>
      </div>
    </div>
  </div>
{/if}

<style>
  /* Settings.svelte gives every direct child of its content area these two. */
  .field {
    min-width: 0;
    max-width: 100%;
  }

  /* While the settings are unconfirmed the window dims its disabled controls
     (Settings.svelte, .settings-locked). The dictionary's buttons keep the look
     of the :disabled rule below, as they did there. */
  :global(.settings-locked) select:disabled,
  :global(.settings-locked) textarea:disabled {
    opacity: 0.5;
    cursor: not-allowed;
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

  .dictionary-heading {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 8px;
  }

  .unsaved-indicator {
    color: var(--accent);
    font-size: 11px;
    font-weight: 600;
  }

  .dictionary-actions,
  .dialog-actions {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    margin-top: 8px;
  }

  .dictionary-actions button:disabled,
  .dialog-actions button:disabled {
    cursor: default;
    opacity: 0.6;
  }

  .dialog-backdrop {
    position: fixed;
    inset: 0;
    z-index: 20;
    display: flex;
    align-items: center;
    justify-content: center;
    padding: 20px;
    background: color-mix(in srgb, var(--bg) 78%, transparent);
  }

  .unsaved-dialog {
    width: min(100%, 420px);
    padding: 20px;
    background: var(--bg-secondary);
    border: 1px solid var(--border);
    border-radius: var(--radius);
    box-shadow: 0 12px 36px rgba(0, 0, 0, 0.35);
  }

  .unsaved-dialog h2 {
    margin-bottom: 8px;
    color: var(--text);
    font-size: 15px;
  }

  .unsaved-dialog p {
    color: var(--text-muted);
    font-size: 13px;
  }
</style>
