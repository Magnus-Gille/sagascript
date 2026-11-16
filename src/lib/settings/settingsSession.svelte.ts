// The persisted settings of the Settings window (SAGA-379), as the window shows
// them: the reads that confirm them and the one way a change is saved. The
// whole window renders from `settings`; a read or save here is what every tab
// goes through.
import { getSettings, type HotkeyProfile, type Settings } from "../api";
import { isBackendError, toErrorMessage } from "../errors";
import { createSettingsTransactionController, type SaveOutcome } from "../settingsTransactionController";
import type { SettingsWindow } from "./settingsWindow.svelte";
import type { TabError } from "./tabErrors.svelte";

/** Why a change was not sent while the window cannot tell what is saved. */
export const UNCONFIRMED_BLOCK = "This change was not sent: the window could not confirm the last saved change. Reload settings first.";

export class SettingsSession {
  readonly #win: SettingsWindow;

  settings: Settings | null = $state(null);
  // Why the initial data fetch failed. A change that fails is reported on its tab (settings/tabErrors.svelte.ts).
  initError: string = $state("");
  // Reads of the persisted settings carry a ticket in the order they are issued
  // (#341). `settingsReadSettled` is the highest ticket that has answered, with a
  // copy or with a failure. An answer older than that is dropped, so answers that
  // arrive out of order cannot put an older copy over a newer one, nor hide a
  // newer failure.
  #settingsReadIssued = 0;
  #settingsReadSettled = 0;
  // Set while `settings` is known to be behind what the backend holds: a change was
  // saved (or the file changed) and the read that should confirm it failed. Until a
  // read succeeds the window says so, disables the controls that save a setting and
  // sends no further change.
  unconfirmed: { afterSave: boolean; detail: string } | null = $state(null);
  reloading = $state(false);
  // The rejection message of a language change whose preview read then failed, and
  // the tab error showing it. No preview follows it, so the read that confirms the
  // settings lifts it too.
  rejectionWithoutPreview: { shownIn: TabError; message: string } | null = null;
  // Non-blocking notices: a profile's model was reset to Auto by a language change.
  profileModelNotices: string[] = $state([]);

  constructor(win: SettingsWindow) {
    this.#win = win;
  }

  get locked(): boolean {
    return this.unconfirmed !== null;
  }

  profiles(source: Settings | null = this.settings): HotkeyProfile[] {
    return source?.hotkey_profiles ?? [];
  }

  profileForId(profileId: string | null, source: Settings | null = this.settings): HotkeyProfile | null {
    if (!profileId) return null;
    return this.profiles(source).find((profile) => profile.id === profileId) ?? null;
  }

  /**
   * Read the persisted settings and show them; showing a copy confirms it.
   * Resolves with the copy now shown. A read that a later-issued read has
   * already answered is dropped, whether it succeeds or fails: that answer
   * stands. Any other failure rejects. When a copy is already shown it also
   * marks that copy unconfirmed, for every caller, because a read failed and
   * the copy may be behind the saved state; `afterSave` says a change of ours
   * was saved before this read, which the notice then states.
   */
  read = async (afterSave = false): Promise<Settings> => {
    const ticket = ++this.#settingsReadIssued;
    let fresh: Settings;
    try {
      fresh = await getSettings();
    } catch (error) {
      const superseded = ticket < this.#settingsReadSettled;
      this.#settingsReadSettled = Math.max(this.#settingsReadSettled, ticket);
      if (superseded && this.settings) return this.settings;
      if (this.settings) this.markUnconfirmed(error, afterSave);
      throw error;
    }
    if (ticket > this.#settingsReadSettled) {
      this.#settingsReadSettled = ticket;
      this.settings = fresh;
      this.#confirmSettings();
    } else if (!this.settings) {
      // A newer read failed before this older one answered. Nothing is shown
      // yet, so nothing can go stale: show this copy rather than none. The
      // newer failure's notice stays, with Reload settings.
      this.settings = fresh;
    }
    return this.settings ?? fresh;
  };

  /** A read showed the saved state: lift the notice and the messages that only said a change was held back. */
  #confirmSettings(): void {
    if (!this.unconfirmed) return;
    this.unconfirmed = null;
    this.#win.errors.dismiss(UNCONFIRMED_BLOCK);
    this.rejectionWithoutPreview?.shownIn.dismiss(this.rejectionWithoutPreview.message);
    this.rejectionWithoutPreview = null;
    if (this.#win.profiles.hotkeyError === UNCONFIRMED_BLOCK) this.#win.profiles.hotkeyError = "";
    this.#win.downloads.dismiss(UNCONFIRMED_BLOCK);
  }

  /**
   * After a read that resolved: a newer read may have failed meanwhile and marked the
   * copy, while this one was dropped. Then the change of ours that this read followed
   * (`afterSave`) was saved but is still unconfirmed. Resolves whether the copy is unconfirmed.
   */
  restampUnconfirmed = (afterSave: boolean): boolean => {
    const unconfirmed = this.unconfirmed;
    if (!unconfirmed) return false;
    this.markUnconfirmed(unconfirmed.detail, afterSave);
    return true;
  };

  markUnconfirmed = (error: unknown, afterSave: boolean): void => {
    this.unconfirmed = {
      afterSave: afterSave || (this.unconfirmed?.afterSave ?? false),
      detail: toErrorMessage(error, "unknown error"),
    };
  };

  /**
   * Read the settings again and refresh what depends on them, for the
   * settings-changed event and the Reload settings button. A failed read keeps
   * the notice, with the newest reason.
   */
  async sync(): Promise<void> {
    try {
      const current = await this.read();
      await this.#win.profileModels.refresh(current.hotkey_profiles);
    } catch (error) {
      this.markUnconfirmed(error, false);
    }
  }

  /**
   * One settings change followed by its read-back, kept apart (#341):
   * - `saved: false`: the backend rejected the change (or the window held it
   *   back, see below). Nothing was saved and nothing was read.
   * - `saved: true`: the change is saved; it is never repeated. `confirmed` says
   *   whether the read-back showed the saved state. When it failed, the window
   *   keeps the old copy, marks it unconfirmed and shows the notice that offers
   *   Reload settings; it does not claim the save failed.
   * While unconfirmed, no change is sent: it could be built from the old copy
   * (a toggle, a whole profile list, a dictionary save) and overwrite what was
   * saved. The controls that save a setting are disabled then (`settingsLocked`),
   * so this hold only catches what stays enabled, such as the dictionary Save.
   * `afterSave` runs as soon as the change is saved, before the read-back.
   */
  readonly #settingsTransactionController = createSettingsTransactionController({
    isUnconfirmed: () => this.unconfirmed !== null,
    unconfirmedError: UNCONFIRMED_BLOCK,
    readBack: () => this.read(true),
    markUnconfirmed: this.markUnconfirmed,
    restamp: this.restampUnconfirmed,
  });

  save<T>(mutate: () => Promise<T>, afterSave?: (result: T) => void): Promise<SaveOutcome<T>> {
    return this.#settingsTransactionController.save(mutate, afterSave);
  }

  /**
   * Shared wrapper for the common "mutate then refresh settings" pattern.
   * Resolves true when the change is saved, even if the read-back that should
   * confirm it failed (then the unconfirmed notice is shown, see saveSetting).
   * `report` is the error of the tab the change was made on: it is cleared when
   * the change starts and holds the message when the change is rejected. A
   * caller that shows the failure itself passes null and reads `errorSink`.
   * When it is rejected: also reassigns `settings` so rendered values come from
   * the last known copy, and resolves false. A native select or input keeps the
   * rejected value on its own: callers put the saved one back by hand. Never
   * re-throws.
   */
  apply = async (
    mutate: () => Promise<void | string[]>,
    report: TabError | null,
    errorSink?: { value: string; code: string | null },
    refreshModels = true,
  ): Promise<boolean> => {
    if (report) report.message = "";
    const outcome = await this.save(mutate, (notices) => {
      this.profileModelNotices = Array.isArray(notices) ? notices : [];
    });
    if (!outcome.saved) {
      const message = toErrorMessage(outcome.error, "Failed to save setting.");
      if (report) report.message = message;
      if (errorSink) {
        errorSink.value = message;
        errorSink.code = isBackendError(outcome.error) ? outcome.error.code : null;
      }
      if (this.settings) this.settings = { ...this.settings };
      return false;
    }
    if (refreshModels && outcome.confirmed && this.settings) await this.#win.profileModels.refresh(this.settings.hotkey_profiles);
    return true;
  };

  /** `applySetting` for a setting no model depends on: the models are not queried again. */
  applyScalar(mutate: () => Promise<void | string[]>, report: TabError): Promise<boolean> {
    return this.apply(mutate, report, undefined, false);
  }
}
