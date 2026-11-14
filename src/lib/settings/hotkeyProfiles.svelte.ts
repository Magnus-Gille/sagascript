// The dictation profiles of the Settings window (SAGA-379): their shortcuts,
// the recorder that captures a new one, whether the saved shortcuts are bound,
// the Accessibility permission they and auto-paste need, and the profile list.
import {
  checkAccessibilityPermission,
  hotkeyStatus,
  retryHotkeyRegistration,
  setAutoPaste,
  setHotkeyProfiles,
  type HotkeyProfile,
  type HotkeyStatus,
  type Language,
} from "../api";
import { isBackendError, toErrorMessage } from "../errors";
import {
  brightnessKeyHotkeyWarning,
  canUseBareHotkey,
  formatHotkeyDisplay as formatShortcutDisplay,
  supportedBareFunctionKeyRange,
  tauriKeyName,
} from "../hotkey";
import { requestAccessibility } from "../permissions";
import { allProfileShortcutValues, profileWithShortcut, suggestedProfileShortcuts } from "../profile-shortcuts";
import type { SettingsWindow } from "./settingsWindow.svelte";
import type { TabError } from "./tabErrors.svelte";

export type ExplicitShortcutSlot = "push_to_talk_shortcut" | "toggle_shortcut";
export const shortcutControls: { slot: ExplicitShortcutSlot; label: string; helper: string }[] = [
  {
    slot: "push_to_talk_shortcut",
    label: "Hold to record",
    helper: "Hold the shortcut while speaking. Release to stop.",
  },
  {
    slot: "toggle_shortcut",
    label: "Press to start/stop",
    helper: "Press the shortcut to start recording. Press it again to stop.",
  },
];

export class HotkeyProfiles {
  readonly #win: SettingsWindow;

  accessibilityGranted: boolean = $state(true); // assume true; checked on mount for macOS
  accessibilityChecking: boolean = $state(false);
  accessibilityRequested: boolean = $state(false);

  // Hotkey recorder state
  recordingProfileId: string | null = $state(null);
  recordingShortcutSlot: ExplicitShortcutSlot | null = $state(null);
  #hotkeyCaptureGeneration = 0;
  draftProfileId: string | null = $state(null);
  hotkeyError: string = $state("");

  // Hotkey registration health — is the saved hotkey actually bound right
  // now? (distinct from hotkeyError above, which is about validating a
  // shortcut the user is in the middle of entering.) Assume healthy until
  // proven otherwise so there's no flash of a warning before the initial
  // fetch resolves.
  statusOk: boolean = $state(true);
  statusError: string = $state("");

  constructor(win: SettingsWindow) {
    this.#win = win;
  }

  /** The Dictate tab's error: what a change made here that fails is reported in. */
  get #error(): TabError {
    return this.#win.errors.of("dictate");
  }

  setStatus(status: HotkeyStatus): void {
    this.statusOk = status.ok;
    this.statusError = status.error ?? "";
  }

  onAutoPasteToggle = async () => {
    if (!this.#win.session.settings) return;
    const enabling = !this.#win.session.settings.auto_paste;
    if (!enabling || this.#win.platform !== "macos") {
      await this.#win.session.applyScalar(() => setAutoPaste(enabling), this.#error);
      return;
    }

    // Keep the preference off until TCC actually reports approval. This is an
    // explicit user action, so it is the one place where prompting is allowed.
    this.accessibilityChecking = true;
    this.accessibilityRequested = true;
    this.#error.message = "";
    try {
      this.accessibilityGranted = await checkAccessibilityPermission();
      if (!this.accessibilityGranted) {
        this.accessibilityGranted = await requestAccessibility({ onGranted: this.refreshRegistration });
      }

      if (this.accessibilityGranted) {
        await this.#win.session.applyScalar(() => setAutoPaste(true), this.#error);
      } else {
        await this.#win.session.applyScalar(() => setAutoPaste(false), this.#error);
        this.#error.message = "Accessibility permission was not granted. Auto-paste remains off.";
      }
    } catch (e) {
      await this.#win.session.applyScalar(() => setAutoPaste(false), this.#error);
      this.#error.message = toErrorMessage(e, "Failed to check Accessibility permission.");
    } finally {
      this.accessibilityChecking = false;
    }
  };

  // Non-blocking hint when a configured shortcut is bare F14/F15 on macOS.
  brightnessKeyWarning = $derived.by(() => {
    if (!this.#win.session.settings) return null;
    const shortcuts = [
      this.#win.session.settings.hotkey,
      ...allProfileShortcutValues(this.#win.session.settings.hotkey_profiles, this.#win.session.settings.hotkey_mode),
    ];
    for (const shortcut of shortcuts) {
      const warning = brightnessKeyHotkeyWarning(shortcut, this.#win.platform);
      if (warning) return warning;
    }
    return null;
  });

  configuredShortcutsUseBareHotkey(): boolean {
    // Only bare F13–F24 registrations depend on the macOS Accessibility
    // grant, so only those benefit from an automatic retry on Settings
    // open. Retrying other failures (e.g. shortcut-in-use) would just churn
    // unregister/register without helping. Gated on shortcut content, not
    // on backend error text, to avoid fragile string coupling.
    if (!this.#win.session.settings) return false;
    const shortcuts = [
      this.#win.session.settings.hotkey,
      ...allProfileShortcutValues(this.#win.session.settings.hotkey_profiles, this.#win.session.settings.hotkey_mode),
    ];
    return shortcuts.some((shortcut) => canUseBareHotkey(shortcut, this.#win.platform));
  }

  refreshRegistration = async (): Promise<void> => {
    let retryError: unknown = null;
    try {
      await retryHotkeyRegistration();
      const current = await this.#win.session.read();
      await this.#win.profileModels.refresh(current.hotkey_profiles);
    } catch (error) {
      // The registration-health response below contains the backend's full
      // diagnostic and keeps the failure visible in Settings.
      console.warn("Failed to retry hotkey registration", error);
      retryError = error;
    }
    const status = await hotkeyStatus();
    this.statusOk = status.ok;
    this.statusError = status.error ?? "";
    // A retry that was refused as busy (the macOS event monitor is still
    // starting, or a dictation is active) attempted nothing, so the health
    // above has no diagnostic for it and only says the old failure: show why
    // the retry did not happen, and that trying again will work.
    if (isBackendError(retryError, "busy")) this.statusError = toErrorMessage(retryError);
  };

  /** Platform-correct modifier display names */
  modifierNames(): { ctrl: string; alt: string; meta: string } {
    const mac = this.#win.platform === "macos";
    return {
      ctrl: mac ? "Control" : "Ctrl",
      alt: mac ? "Option" : "Alt",
      meta: mac ? "Cmd" : "Win",
    };
  }

  /** Format a shortcut string for display (e.g. "Control+Shift+Space" → "Ctrl + Shift + Space") */
  formatHotkeyDisplay(shortcut: string): string {
    return formatShortcutDisplay(shortcut, this.#win.platform);
  }

  beginCapture(profileId: string, slot: ExplicitShortcutSlot) {
    this.#hotkeyCaptureGeneration += 1;
    this.recordingProfileId = profileId;
    this.recordingShortcutSlot = slot;
    this.hotkeyError = "";
  }

  /** The recorder lost the focus: the capture ends without a change. */
  abandonCapture(): void {
    this.recordingProfileId = null;
    this.recordingShortcutSlot = null;
    this.hotkeyError = "";
  }

  async onKeydown(e: KeyboardEvent, profileId: string, slot: ExplicitShortcutSlot) {
    const captureGeneration = this.#hotkeyCaptureGeneration;
    e.preventDefault();
    e.stopPropagation();

    // Escape cancels recording
    if (e.key === "Escape") {
      this.#hotkeyCaptureGeneration += 1;
      this.recordingProfileId = null;
      this.recordingShortcutSlot = null;
      this.hotkeyError = "";
      return;
    }

    // Ignore bare modifier presses — wait for a non-modifier key
    if (["Control", "Shift", "Alt", "Meta"].includes(e.key)) return;

    const keyName = tauriKeyName(e.key, e.code, this.#win.platform);
    if (!keyName) {
      this.hotkeyError =
        e.code === "IntlBackslash"
          ? "The ISO section key is supported only on macOS and Windows. Choose another key."
          : `"${e.key}" is not a supported key. Use A–Z, 0–9, F1–F24, Space, Arrow keys, or Tab/Enter/Delete.`;
      return;
    }

    // Ordinary keys require a modifier. Extended function keys are reserved
    // for programmable buttons and may be used directly through the native
    // macOS monitor or the Windows global-shortcut backend.
    const hasModifier = e.ctrlKey || e.altKey || e.metaKey || e.shiftKey;
    if (this.#win.platform === "macos" && hasModifier && /^F2[1-4]$/.test(keyName)) {
      this.hotkeyError = `${keyName} is supported without modifiers on macOS, but its modified forms cannot be registered reliably.`;
      return;
    }
    if (!hasModifier && !canUseBareHotkey(keyName, this.#win.platform)) {
      const m = this.modifierNames();
      const bareRange = supportedBareFunctionKeyRange(this.#win.platform);
      this.hotkeyError = `Shortcut must include a modifier (${m.ctrl}, ${m.alt}, ${m.meta}, or Shift).${bareRange ? ` ${bareRange} may be used alone.` : ""}`;
      return;
    }

    if (this.#win.platform === "macos" && !hasModifier && /^F(?:1[3-9]|2[0-4])$/.test(keyName)) {
      this.accessibilityChecking = true;
      try {
        this.accessibilityGranted = await checkAccessibilityPermission();
        if (!this.accessibilityGranted) {
          this.accessibilityGranted = await requestAccessibility({ onGranted: this.refreshRegistration });
        }
      } catch (error) {
        this.hotkeyError = toErrorMessage(error, "Failed to check Accessibility permission.");
        return;
      } finally {
        this.accessibilityChecking = false;
      }
      if (!this.accessibilityGranted) {
        this.hotkeyError = "F13–F24 requires Accessibility permission on macOS. Permission was not granted.";
        return;
      }
      if (captureGeneration !== this.#hotkeyCaptureGeneration) return;
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
    this.hotkeyError = "";

    const currentSettings = this.#win.session.settings;
    if (!currentSettings) return;
    const profiles = currentSettings.hotkey_profiles.map((profile) =>
      profile.id === profileId ? profileWithShortcut(profile, slot, shortcut, currentSettings.hotkey_mode) : profile,
    );
    const outcome = await this.#win.session.save(
      () => setHotkeyProfiles(profiles),
      () => {
        this.recordingProfileId = null;
        this.recordingShortcutSlot = null;
        this.draftProfileId = null;
      },
    );
    if (!outcome.saved) this.hotkeyError = toErrorMessage(outcome.error, "Failed to set hotkey");
    else if (outcome.confirmed && this.#win.session.settings)
      await this.#win.profileModels.refresh(this.#win.session.settings.hotkey_profiles);
  }

  async clearShortcut(profileId: string, slot: ExplicitShortcutSlot) {
    const currentSettings = this.#win.session.settings;
    if (!currentSettings) return;
    const profile = currentSettings.hotkey_profiles.find((candidate) => candidate.id === profileId);
    if (!profile || !profile.push_to_talk_shortcut || !profile.toggle_shortcut) return;
    const profiles = currentSettings.hotkey_profiles.map((candidate) =>
      candidate.id === profileId ? profileWithShortcut(candidate, slot, null, currentSettings.hotkey_mode) : candidate,
    );
    if (profileId === this.draftProfileId) {
      this.#win.session.settings = { ...currentSettings, hotkey_profiles: profiles };
      await this.#win.profileModels.refresh(profiles);
      return;
    }
    const saved = await this.#win.session.apply(() => setHotkeyProfiles(profiles), this.#error);
    if (!saved && this.#error.message) this.hotkeyError = this.#error.message;
  }

  /** Resolves true when the change is kept (saved, or applied to a profile that is still a draft). */
  async update(profileId: string, changes: Partial<HotkeyProfile>): Promise<boolean> {
    if (!this.#win.session.settings) return false;
    const profiles = this.#win.session.settings.hotkey_profiles.map((profile) =>
      profile.id === profileId ? { ...profile, ...changes } : profile,
    );
    if (profileId === this.draftProfileId) {
      this.#win.session.settings = { ...this.#win.session.settings, hotkey_profiles: profiles };
      await this.#win.profileModels.refresh(profiles);
      return true;
    }
    return this.#win.session.apply(() => setHotkeyProfiles(profiles), this.#error);
  }

  async onLanguageChange(event: Event, profile: HotkeyProfile) {
    const select = event.currentTarget as HTMLSelectElement;
    const saved = await this.update(profile.id, { language: select.value as Language });
    if (!saved) select.value = this.#win.session.profileForId(profile.id)?.language ?? profile.language;
  }

  async onNameBlur(event: Event, profile: HotkeyProfile) {
    const input = event.currentTarget as HTMLInputElement;
    // Leaving an untouched field sends nothing while the window cannot confirm what is saved.
    if (this.#win.session.unconfirmed && input.value === profile.name) return;
    const saved = await this.update(profile.id, { name: input.value });
    if (!saved) input.value = this.#win.session.profileForId(profile.id)?.name ?? profile.name;
  }

  add = () => {
    if (!this.#win.session.settings) return;
    let suffix = this.#win.session.settings.hotkey_profiles.length + 1;
    while (
      this.#win.session.settings.hotkey_profiles.some((profile) => profile.id === `profile-${suffix}`) ||
      Object.hasOwn(this.#win.session.settings.profile_glossaries, `profile-${suffix}`)
    )
      suffix += 1;
    const suggested = suggestedProfileShortcuts(
      this.#win.session.settings.hotkey_profiles,
      this.#win.session.settings.hotkey,
      this.#win.platform,
    );
    const profile: HotkeyProfile = {
      id: `profile-${suffix}`,
      name: `Profile ${suffix}`,
      ...suggested,
      language: this.#win.session.settings.language === "sv" ? "en" : "sv",
    };
    this.#win.session.settings = {
      ...this.#win.session.settings,
      hotkey_profiles: [...this.#win.session.settings.hotkey_profiles, profile],
    };
    this.draftProfileId = profile.id;
    this.beginCapture(profile.id, "push_to_talk_shortcut");
    void this.#win.profileModels.refresh(this.#win.session.settings.hotkey_profiles);
  };

  async remove(profileId: string) {
    if (!this.#win.session.settings || this.#win.session.settings.hotkey_profiles.length <= 1) return;
    if (profileId === this.draftProfileId) {
      this.#win.session.settings = {
        ...this.#win.session.settings,
        hotkey_profiles: this.#win.session.settings.hotkey_profiles.filter((profile) => profile.id !== profileId),
      };
      this.draftProfileId = null;
      this.recordingProfileId = null;
      this.recordingShortcutSlot = null;
      return;
    }
    await this.#win.session.apply(
      () => setHotkeyProfiles(this.#win.session.settings!.hotkey_profiles.filter((profile) => profile.id !== profileId)),
      this.#error,
    );
  }
}
