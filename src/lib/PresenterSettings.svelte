<script lang="ts">
  import type { PresenterConfig, PresenterFinishAction } from "./api";
  import { canUseBareHotkey, supportedBareFunctionKeyRange, tauriKeyName } from "./hotkey.js";

  type PresenterSettingsProps = {
    config: PresenterConfig;
    profileShortcuts: string[];
    platform: string;
    onSave: (config: PresenterConfig) => Promise<string | null>;
  };

  let {
    config,
    profileShortcuts,
    platform,
    onSave,
  }: PresenterSettingsProps = $props();

  let draft: PresenterConfig = $state({
    finish_shortcut: "",
    cancel_shortcut: null,
    app_actions: {},
  });
  let newAppId = $state("");
  let localError = $state("");
  let saveError = $state("");
  let saving = $state(false);
  let recordingField = $state<"finish" | "cancel" | null>(null);
  let initialized = false;
  let lastConfigJson = "";

  const actionLabels: Record<PresenterFinishAction, string> = {
    insert_only: "Insert only",
    return: "Insert, then Return",
    command_return: "Insert, then Command+Return",
  };

  const supportsAutoSubmit = () => platform === "macos";

  function cloneConfig(source: PresenterConfig): PresenterConfig {
    return {
      finish_shortcut: source.finish_shortcut,
      cancel_shortcut: source.cancel_shortcut,
      app_actions: { ...source.app_actions },
    };
  }

  // A failed backend save can cause the parent to refresh Settings. Keep the
  // unsaved draft when the refreshed config has the same serialized value.
  $effect(() => {
    const incoming = JSON.stringify(config);
    const current = JSON.stringify(draft);
    if (!initialized || (incoming !== lastConfigJson && incoming !== current)) {
      draft = cloneConfig(config);
      localError = "";
      saveError = "";
    }
    lastConfigJson = incoming;
    initialized = true;
  });

  function modifierCanonical(modifier: string): string {
    switch (modifier.toLowerCase()) {
      case "control":
      case "ctrl":
        return "control";
      case "alt":
      case "option":
        return "alt";
      case "super":
      case "command":
      case "cmd":
        return "super";
      case "commandorcontrol":
      case "commandorctrl":
      case "cmdorctrl":
      case "cmdorcontrol":
        return platform === "macos" ? "super" : "control";
      case "shift":
        return "shift";
      default:
        return modifier.toLowerCase();
    }
  }

  function canonicalShortcut(shortcut: string): string {
    const tokens = shortcut.split("+").map((token) => token.trim()).filter(Boolean);
    if (tokens.length === 0) return "";
    const key = tokens[tokens.length - 1].toLowerCase();
    const normalizedKey = key.length === 1 && /^[a-z]$/.test(key) ? `key${key}` : key;
    return [
      ...new Set(tokens.slice(0, -1).map(modifierCanonical)),
    ].sort().concat(normalizedKey).join("+");
  }

  function updateDraft(changes: Partial<PresenterConfig>): void {
    draft = { ...draft, ...changes };
    localError = "";
    saveError = "";
  }

  function modifierNames(): { ctrl: string; alt: string; meta: string } {
    return platform === "macos"
      ? { ctrl: "Control", alt: "Option", meta: "Cmd" }
      : { ctrl: "Ctrl", alt: "Alt", meta: "Win" };
  }

  function formatShortcutDisplay(shortcut: string | null): string {
    if (!shortcut) return "Disabled — click to set";
    const names = modifierNames();
    return shortcut
      .replace(/Control/g, names.ctrl)
      .replace(/Alt/g, names.alt)
      .replace(/Super/g, names.meta)
      .split("+")
      .join(" + ");
  }

  function beginShortcutCapture(field: "finish" | "cancel"): void {
    recordingField = field;
    localError = "";
    saveError = "";
  }

  function onShortcutKeydown(event: KeyboardEvent, field: "finish" | "cancel"): void {
    event.preventDefault();
    event.stopPropagation();
    if (event.key === "Escape") {
      recordingField = null;
      return;
    }
    if (["Control", "Shift", "Alt", "Meta"].includes(event.key)) return;
    const keyName = tauriKeyName(event.key);
    if (!keyName) {
      localError = `"${event.key}" is not a supported key.`;
      return;
    }
    const hasModifier = event.ctrlKey || event.altKey || event.metaKey || event.shiftKey;
    if (!hasModifier && !canUseBareHotkey(keyName, platform)) {
      const range = supportedBareFunctionKeyRange(platform);
      localError = `Shortcut must include a modifier.${range ? ` ${range} may be used alone.` : ""}`;
      return;
    }
    const parts: string[] = [];
    if (event.ctrlKey) parts.push("Control");
    if (event.altKey) parts.push("Alt");
    if (event.metaKey) parts.push("Super");
    if (event.shiftKey) parts.push("Shift");
    parts.push(keyName);
    updateDraft(field === "finish"
      ? { finish_shortcut: parts.join("+") }
      : { cancel_shortcut: parts.join("+") });
    recordingField = null;
  }

  function updateAppAction(appId: string, action: PresenterFinishAction): void {
    draft = {
      ...draft,
      app_actions: { ...draft.app_actions, [appId]: action },
    };
    localError = "";
    saveError = "";
  }

  function removeAppAction(appId: string): void {
    const appActions = { ...draft.app_actions };
    delete appActions[appId];
    draft = { ...draft, app_actions: appActions };
    localError = "";
    saveError = "";
  }

  function validateDraft(): string | null {
    if (!draft.finish_shortcut.trim()) return "Finish shortcut is required.";
    const profileKeys = new Set(profileShortcuts.map(canonicalShortcut));
    const finishKey = canonicalShortcut(draft.finish_shortcut);
    if (profileKeys.has(finishKey)) {
      return "Finish shortcut conflicts with a presenter start shortcut.";
    }
    if (draft.cancel_shortcut) {
      const cancelKey = canonicalShortcut(draft.cancel_shortcut);
      if (cancelKey === finishKey) return "Finish and cancel shortcuts must differ.";
      if (profileKeys.has(cancelKey)) {
        return "Cancel shortcut conflicts with a presenter start shortcut.";
      }
    }
    const appIds = Object.keys(draft.app_actions);
    if (appIds.length > 32) return "Presenter supports at most 32 app actions.";
    for (const appId of appIds) {
      if (!appId || new TextEncoder().encode(appId).length > 512 || [...appId].some((char) => /\p{Cc}/u.test(char))) {
        return "App identifiers must be non-empty, at most 512 bytes, and contain no control characters.";
      }
      if (!supportsAutoSubmit() && draft.app_actions[appId] !== "insert_only") {
        return "Automatic Return actions are currently supported only on macOS.";
      }
    }
    return null;
  }

  async function save(): Promise<void> {
    localError = validateDraft() ?? "";
    if (localError) return;
    saving = true;
    saveError = "";
    try {
      saveError = (await onSave(cloneConfig(draft))) ?? "";
    } catch (error: any) {
      saveError = typeof error === "string" ? error : error?.message || "Failed to save presenter settings.";
    } finally {
      saving = false;
    }
  }

  function addAppAction(): void {
    const appId = newAppId.trim();
    if (!appId) {
      localError = "Enter a stable application identifier first.";
      return;
    }
    if (Object.hasOwn(draft.app_actions, appId)) {
      localError = "That application identifier is already configured.";
      return;
    }
    draft = { ...draft, app_actions: { ...draft.app_actions, [appId]: "insert_only" } };
    newAppId = "";
    localError = "";
    saveError = "";
  }
</script>

<section class="presenter-settings" aria-labelledby="presenter-settings-title">
  <div class="presenter-heading">
    <div>
      <h3 id="presenter-settings-title">Presenter mode</h3>
      <p class="presenter-description">
        Profile shortcuts start dictation; Finish ends it. These controls configure behavior only.
        Application identifiers are stable IDs you enter yourself — Sagascript does not detect titles,
        sites, or the foreground app here.
      </p>
    </div>
    <span class="presenter-platform">{platform === "macos" ? "macOS support" : "Manual copy on this platform"}</span>
  </div>

  <div class="presenter-fields">
    <label for="presenter-finish">Finish shortcut</label>
    {#if recordingField === "finish"}
      <button
        id="presenter-finish"
        class="presenter-hotkey-recorder recording"
        onkeydown={(event) => onShortcutKeydown(event, "finish")}
        onblur={() => { recordingField = null; }}
      >Press shortcut…</button>
    {:else}
      <button
        id="presenter-finish"
        class="presenter-hotkey-recorder"
        onclick={() => beginShortcutCapture("finish")}
      >{formatShortcutDisplay(draft.finish_shortcut)}</button>
    {/if}
    <span id="presenter-finish-help" class="presenter-hint">Global shortcut used to finish the current presenter dictation.</span>

    <label for="presenter-cancel">Cancel shortcut <span class="optional">(optional)</span></label>
    {#if recordingField === "cancel"}
      <button
        id="presenter-cancel"
        class="presenter-hotkey-recorder recording"
        onkeydown={(event) => onShortcutKeydown(event, "cancel")}
        onblur={() => { recordingField = null; }}
      >Press shortcut…</button>
    {:else}
      <button
        id="presenter-cancel"
        class="presenter-hotkey-recorder"
        onclick={() => beginShortcutCapture("cancel")}
      >{formatShortcutDisplay(draft.cancel_shortcut)}</button>
    {/if}
    {#if draft.cancel_shortcut}
      <button type="button" class="link-btn secondary presenter-disable-cancel" onclick={() => updateDraft({ cancel_shortcut: null })}>Disable</button>
    {/if}
    <span id="presenter-cancel-help" class="presenter-hint">Leave empty to disable cancel.</span>
  </div>

  <div class="presenter-actions">
    <div class="presenter-actions-heading">
      <div>
        <strong>Application actions</strong>
        <div class="presenter-hint">Default: Insert only. Automatic Return actions require the supported macOS path; each action requires Accessibility permission and a verifiable focused text field.</div>
      </div>
      <span class="presenter-count">{Object.keys(draft.app_actions).length}/32</span>
    </div>
    {#each Object.entries(draft.app_actions) as [appId, action] (appId)}
      <div class="presenter-action-row">
        <code title={appId}>{appId}</code>
        <select
          aria-label={`Action for ${appId}`}
          value={action}
          onchange={(event) => updateAppAction(appId, (event.target as HTMLSelectElement).value as PresenterFinishAction)}
        >
          <option value="insert_only">{actionLabels.insert_only}</option>
          <option value="return" disabled={!supportsAutoSubmit()}>{actionLabels.return}</option>
          <option value="command_return" disabled={!supportsAutoSubmit()}>{actionLabels.command_return}</option>
        </select>
        <button type="button" class="presenter-remove" onclick={() => removeAppAction(appId)}>Remove</button>
      </div>
    {/each}
    <div class="presenter-add-row">
      <input
        type="text"
        aria-label="New application identifier"
        placeholder="com.example.editor"
        value={newAppId}
        oninput={(event) => { newAppId = (event.target as HTMLInputElement).value; }}
        onkeydown={(event) => { if (event.key === "Enter") addAppAction(); }}
      />
      <button type="button" class="link-btn secondary" onclick={addAppAction}>Add app</button>
    </div>
  </div>

  {#if platform !== "macos"}
    <div class="presenter-hint presenter-platform-warning">
      Automatic Return and Command+Return actions are disabled here; recognized text remains a draft for manual copying.
    </div>
  {/if}
  {#if localError}
    <div class="presenter-error" role="alert">{localError}</div>
  {/if}
  {#if saveError}
    <div class="presenter-error" role="alert">{saveError}</div>
  {/if}
  <button type="button" class="presenter-save" onclick={save} disabled={saving}>
    {saving ? "Saving…" : "Save presenter settings"}
  </button>
</section>

<style>
  .presenter-settings {
    margin: 18px 0;
    padding: 14px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg-secondary);
  }

  .presenter-heading,
  .presenter-actions-heading,
  .presenter-action-row,
  .presenter-add-row {
    display: flex;
    align-items: center;
    gap: 10px;
  }

  .presenter-heading,
  .presenter-actions-heading {
    justify-content: space-between;
  }

  h3 {
    margin: 0;
    font-size: 14px;
  }

  .presenter-description {
    max-width: 620px;
    margin: 4px 0 0;
    color: var(--text-secondary, #888);
    font-size: 11px;
    line-height: 1.45;
  }

  .presenter-platform,
  .presenter-count,
  .optional {
    color: var(--text-muted);
    font-size: 10px;
  }

  .presenter-platform {
    min-width: max-content;
    flex-shrink: 0;
    white-space: nowrap;
  }

  .presenter-fields {
    display: grid;
    grid-template-columns: minmax(130px, 0.35fr) minmax(180px, 1fr);
    align-items: center;
    gap: 6px 10px;
    margin-top: 14px;
  }

  .presenter-fields label {
    font-size: 12px;
    font-weight: 600;
  }

  .presenter-hotkey-recorder,
  .presenter-add-row input,
  .presenter-action-row select {
    min-width: 0;
    box-sizing: border-box;
    width: 100%;
  }

  .presenter-fields .presenter-hint {
    grid-column: 2;
  }

  .presenter-hotkey-recorder {
    padding: 7px 10px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg);
    color: var(--accent);
    cursor: pointer;
    font-family: monospace;
    text-align: left;
  }

  .presenter-hotkey-recorder.recording {
    border-color: var(--accent);
    color: var(--text-muted);
  }

  .presenter-disable-cancel {
    grid-column: 2;
    justify-self: end;
  }

  .presenter-actions {
    margin-top: 16px;
    padding-top: 12px;
    border-top: 1px solid var(--border);
  }

  .presenter-actions-heading {
    align-items: flex-start;
  }

  .presenter-action-row {
    margin-top: 8px;
  }

  .presenter-action-row code {
    flex: 1;
    min-width: 0;
    overflow: hidden;
    color: var(--text);
    font-size: 11px;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .presenter-action-row select {
    flex: 0 1 230px;
  }

  .presenter-remove {
    border: none;
    background: none;
    color: var(--danger);
    cursor: pointer;
    font-size: 11px;
  }

  .presenter-add-row {
    margin-top: 10px;
  }

  .presenter-add-row input {
    flex: 1;
  }

  .presenter-add-row .link-btn {
    flex: 0 0 auto;
  }

  .presenter-hint {
    color: var(--text-secondary, #888);
    font-size: 11px;
    line-height: 1.4;
  }

  .presenter-platform-warning,
  .presenter-error {
    margin-top: 10px;
  }

  .presenter-error {
    color: var(--danger);
    font-size: 11px;
  }

  .presenter-save {
    margin-top: 14px;
    padding: 7px 12px;
    border: 1px solid var(--accent);
    border-radius: var(--radius);
    background: var(--accent);
    color: var(--bg);
    cursor: pointer;
    font-weight: 600;
  }

  .presenter-save:disabled {
    cursor: wait;
    opacity: 0.65;
  }

  @media (max-width: 520px) {
    .presenter-action-row {
      display: grid;
      grid-template-columns: minmax(0, 1fr) auto;
      align-items: center;
    }

    .presenter-action-row code {
      grid-column: 1 / -1;
      width: 100%;
      overflow-wrap: anywhere;
      white-space: normal;
    }

    .presenter-action-row select {
      grid-column: 1;
      width: 100%;
      flex: none;
    }

    .presenter-action-row .presenter-remove {
      grid-column: 2;
      grid-row: 2;
    }
  }
</style>
