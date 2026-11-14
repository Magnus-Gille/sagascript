// The speech model of each dictation profile in the Settings window (SAGA-379):
// what each profile dictates with, what it could use instead, and the one
// change of a profile's model at a time.
import {
  getEffectiveModelInfo,
  getFileModelOptions,
  getProfileModelInfo,
  setProfileModel,
  type HotkeyProfile,
  type WhisperModel,
} from "../api";
import { toErrorMessage } from "../errors";
import { createModelCatalogController } from "../modelCatalogController";
import { pianissimoIdsOf } from "../models.svelte";
import type { SettingsWindow } from "./settingsWindow.svelte";

// Swedish models the app no longer offers. A profile still on one keeps
// working; Settings suggests the recommended model instead of switching silently.
const retiredSwedishModelIds = ["kb-whisper-tiny", "kb-whisper-base", "kb-whisper-small"];

export class ProfileModels {
  readonly #win: SettingsWindow;

  models: Record<string, WhisperModel> = $state({});
  options: Record<string, WhisperModel[]> = $state({});
  // What "Recommended" resolves to per profile (Pianissimo, or KB-Whisper Medium
  // where Pianissimo is unsupported).
  recommended: Record<string, WhisperModel> = $state({});
  saving = $state(false);
  // The receipt of the message of the last check of the speech engines that failed,
  // while the Dictate tab's error may still show it: the next check that succeeds
  // retracts it, and leaves whatever was written there since.
  #checkFailure: number | null = null;

  constructor(win: SettingsWindow) {
    this.#win = win;
  }

  readonly #profileModelCatalogController = createModelCatalogController<
    HotkeyProfile[],
    {
      models: Record<string, WhisperModel>;
      options: Record<string, WhisperModel[]>;
      recommended: Record<string, WhisperModel>;
    }
  >({
    load: async (profiles) => {
      const entries = await Promise.all(
        profiles.map(async (profile) => {
          const model =
            profile.id === this.#win.profiles.draftProfileId
              ? await getEffectiveModelInfo(profile.language)
              : await getProfileModelInfo(profile.id);
          // Keep this lookup after the model-info await: a settings read that
          // settles while that invoke is pending decides the selected model.
          const selected = this.#win.session.settings?.profile_models?.[profile.id];
          const options = await getFileModelOptions(profile.language, selected && selected !== "auto" ? selected : undefined);
          const recommended = await getEffectiveModelInfo(profile.language);
          return [profile.id, model, options, recommended] as const;
        }),
      );
      return {
        models: Object.fromEntries(entries.map(([id, model]) => [id, model])),
        options: Object.fromEntries(entries.map(([id, , options]) => [id, options])),
        recommended: Object.fromEntries(entries.map(([id, , , recommended]) => [id, recommended])),
      };
    },
    publish: ({ models, options, recommended }) => {
      this.models = models;
      this.options = options;
      this.recommended = recommended;
      if (this.#checkFailure !== null) this.#win.errors.of("dictate").retract(this.#checkFailure);
      this.#checkFailure = null;
    },
    // The profiles' models are Dictate's, whichever tab or event asked for the check.
    reject: (error) => {
      this.#checkFailure = this.#win.errors.of("dictate").show(toErrorMessage(error, "Failed to check speech engines."));
    },
  });

  refresh = async (profiles: HotkeyProfile[]): Promise<void> => {
    await this.#profileModelCatalogController.request(profiles).done;
  };

  /**
   * The Pianissimo models a dictation engine error can be about: every one a
   * profile dictates with (Auto resolved). The error does not say which
   * profile failed, so all of them are checked; an intact one costs only a
   * verification.
   */
  dictationPianissimoIds(): string[] {
    return pianissimoIdsOf((this.#win.session.settings?.hotkey_profiles ?? []).map((profile) => this.models[profile.id]?.id));
  }

  /** The recommended model to suggest for a Swedish profile still on a retired model. */
  retiredSuggestion(profile: HotkeyProfile): WhisperModel | null {
    const current = this.models[profile.id];
    const recommended = this.recommended[profile.id];
    if (profile.language !== "sv" || !current || !recommended) return null;
    if (!retiredSwedishModelIds.includes(current.id) || recommended.id === current.id) return null;
    return recommended;
  }

  async switchToRecommended(profile: HotkeyProfile) {
    const target = this.retiredSuggestion(profile);
    if (!target || this.#win.downloads.busy || this.saving) return;
    this.saving = true;
    try {
      await this.#win.downloads.downloadThenSwitch(profile.id, target, async () => {
        const outcome = await this.#win.session.save(() => setProfileModel(profile.id, target.id));
        if (!outcome.saved) throw outcome.error;
        if (outcome.confirmed && this.#win.session.settings) await this.refresh(this.#win.session.settings.hotkey_profiles);
      });
    } finally {
      this.saving = false;
    }
  }

  async change(profile: HotkeyProfile, event: Event) {
    if (!this.#win.session.settings || this.saving) return;
    const select = event.target as HTMLSelectElement;
    const previous = this.#win.session.settings.profile_models[profile.id] ?? "auto";
    this.saving = true;
    this.#win.downloads.setProfileError(profile.id, "");
    try {
      const outcome = await this.#win.session.save(() => setProfileModel(profile.id, select.value));
      if (!outcome.saved) {
        select.value = previous;
        this.#win.downloads.setProfileError(profile.id, toErrorMessage(outcome.error, "Could not change profile model."));
      } else if (outcome.confirmed && this.#win.session.settings) {
        await this.refresh(this.#win.session.settings.hotkey_profiles);
      }
    } finally {
      this.saving = false;
    }
  }

  dispose(): void {
    this.#profileModelCatalogController.dispose();
  }
}
