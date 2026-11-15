// Model downloads of the Settings window (SAGA-379): the one download that
// runs, how far it is, and why a profile's model or the file model could not
// be downloaded. The Dictate and Transcribe tabs both start downloads and both
// show the same progress, so this state belongs to the window, not to a tab.
//
// Built on the window-neutral pieces in models.svelte.ts (#362):
// `ModelDownloads` holds the running download and its progress, and
// `downloadModelById` picks the command. Nothing of those is repeated here.
// A download cannot be cancelled: the backend has no command for it.
import type { WhisperModel } from "../api";
import { toErrorMessage } from "../errors";
import { downloadModelById, useModelDownloads, type ModelDownloads } from "../models.svelte";

export interface SettingsModelDownloadsOptions {
  /** The running download and its progress; `useModelDownloads()` in a component. */
  downloads: ModelDownloads;
  /** A model was written to disk: what the Transcribe tab offers depends on it. */
  onDiskChanged: () => void;
  /** Read every profile's model state again. */
  refreshProfileModels: () => Promise<void>;
  /** Download one model by its id. `downloadModelById` unless a test replaces it. */
  download?: (id: string) => Promise<void>;
}

export class SettingsModelDownloads {
  /**
   * The message under each profile's speech model, by profile id: why its
   * download failed, or why its model could not be changed. One slot per
   * profile, so the latest of them shows.
   */
  profileErrors = $state<Record<string, string>>({});
  /** The message under the file transcription model: a failed download, a failed change or a failed lineup read. */
  fileError = $state("");
  /**
   * True from the press of "Download the model again" to the end of the model
   * list's refresh after it, longer than `busy`, which ends with the download
   * itself: in between the button must not start a second one.
   */
  repairing = $state(false);

  readonly #downloads: ModelDownloads;
  readonly #onDiskChanged: () => void;
  readonly #refreshProfileModels: () => Promise<void>;
  readonly #download: (id: string) => Promise<void>;

  constructor(options: SettingsModelDownloadsOptions) {
    this.#downloads = options.downloads;
    this.#onDiskChanged = options.onDiskChanged;
    this.#refreshProfileModels = options.refreshProfileModels;
    this.#download = options.download ?? downloadModelById;
  }

  /** The id of the model being downloaded, or `null` when none is. */
  get id(): string | null {
    return this.#downloads.id;
  }

  get name(): string {
    return this.#downloads.name;
  }

  /** Percent, 0 to 100. */
  get progress(): number {
    return this.#downloads.progress;
  }

  /** One download at a time: nothing here starts another while this is true. */
  get busy(): boolean {
    return this.#downloads.busy;
  }

  setProfileError(profileId: string, message: string): void {
    this.profileErrors = { ...this.profileErrors, [profileId]: message };
  }

  /** Remove `message` wherever it shows: what it said no longer holds. */
  dismiss(message: string): void {
    if (this.fileError === message) this.fileError = "";
    this.profileErrors = Object.fromEntries(Object.entries(this.profileErrors).map(([id, shown]) => [id, shown === message ? "" : shown]));
  }

  /** Download the speech engine `model` a profile needs. A failure shows under that profile. */
  async downloadProfileModel(profileId: string, model: WhisperModel | undefined): Promise<void> {
    if (!model || model.downloaded || this.busy) return;
    this.#downloads.start(model.id, model.display_name);
    this.setProfileError(profileId, "");
    try {
      await this.#download(model.id);
      this.#onDiskChanged();
      await this.#refreshProfileModels();
    } catch (error) {
      this.setProfileError(profileId, toErrorMessage(error, "Speech engine download failed."));
    } finally {
      this.#downloads.finish();
    }
  }

  /**
   * Download the file transcription model `id`, then `refreshLineup` so the
   * picker shows it as ready. A failure of either shows under the picker.
   */
  async downloadFileModel(id: string, name: string, refreshLineup: () => Promise<unknown>): Promise<void> {
    if (this.busy) return;
    this.#downloads.start(id, name);
    this.fileError = "";
    try {
      await this.#download(id);
      await refreshLineup();
    } catch (error) {
      this.fileError = toErrorMessage(error);
    } finally {
      this.#downloads.finish();
    }
  }

  /**
   * Download the speech engine's models again: what "Download the model again"
   * does for `engine_model_missing` and `engine_model_load_failed`, the codes
   * of the Pianissimo engine. The download first checks an installed model and
   * fetches it again when it is missing or damaged. Progress shows like any
   * other download; rejects when a download fails.
   */
  async downloadAgain(ids: readonly string[], nameOf: (id: string) => string): Promise<void> {
    // Reserved before the first await, so a second press cannot start another.
    this.repairing = true;
    try {
      // One at a time: an installed, intact model only has its files checked.
      for (const id of ids) {
        this.#downloads.start(id, nameOf(id));
        try {
          await this.#download(id);
        } finally {
          this.#downloads.finish();
        }
        // Per model: a later failure must not hide what this one changed on disk.
        this.#onDiskChanged();
      }
      await this.#refreshProfileModels();
    } finally {
      this.repairing = false;
    }
  }

  /**
   * Switch a profile to `model`, downloading it first when it is not on disk:
   * the old model keeps dictating until the new one is ready. `switchModel`
   * saves the choice, and the download counts as running until that has
   * settled. A failure of either shows under the profile.
   */
  async downloadThenSwitch(profileId: string, model: WhisperModel, switchModel: () => Promise<void>): Promise<void> {
    if (this.busy) return;
    this.setProfileError(profileId, "");
    let downloaded = false;
    try {
      if (!model.downloaded) {
        this.#downloads.start(model.id, model.display_name);
        await this.#download(model.id);
        downloaded = true;
      }
      await switchModel();
    } catch (error) {
      this.setProfileError(profileId, toErrorMessage(error, `Could not switch to ${model.display_name}.`));
    } finally {
      this.#downloads.finish();
      // The Transcribe tab's lineup follows what is on disk, also after a failed switch.
      if (downloaded) this.#onDiskChanged();
    }
  }
}

/** A `SettingsModelDownloads` whose progress follows the backend's events for the life of the component. */
export function useSettingsModelDownloads(options: Omit<SettingsModelDownloadsOptions, "downloads">): SettingsModelDownloads {
  return new SettingsModelDownloads({ ...options, downloads: useModelDownloads() });
}
