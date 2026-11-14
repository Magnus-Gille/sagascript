// The file transcription model of the Transcribe tab (SAGA-379): the lineup
// for the selected profile's language, what Auto resolves to, the one change of
// the model at a time and its download.
import {
  getAutoFileModelInfo,
  getEffectiveModelInfo,
  getFileModelOptions,
  setFileTranscriptionModel,
  type Language,
  type WhisperModel,
} from "../api";
import { toErrorMessage } from "../errors";
import { createModelCatalogController } from "../modelCatalogController";
import { firstPianissimoId, isPianissimoId, pianissimoIdForLanguage } from "../models.svelte";
import type { SettingsWindow } from "./settingsWindow.svelte";

type FileModelCatalogQuery = { language: Language; selected?: string };

export class FileModel {
  readonly #win: SettingsWindow;

  options: WhisperModel[] = $state([]);
  pianissimoSizeMb = $derived(
    this.options.find((model) => model.id === this.#win.session.settings?.file_transcription_model && isPianissimoId(model.id))?.size_mb ??
      0,
  );
  // What Auto will use now (recommended if downloaded, else best downloaded) and what is recommended.
  auto: WhisperModel | null = $state(null);
  recommended: WhisperModel | null = $state(null);
  saving = $state(false);

  // The file model lineup depends on these two values and on what is on disk.
  // Read the first two as derived primitives: the effect below must not run
  // again for every new `settings` object (a Voice activity detection toggle,
  // say). A download that finishes elsewhere in the window bumps the third.
  #language = $derived.by(() => (this.#win.session.settings ? this.#win.queue.language() : null));
  #selection = $derived.by(() => this.#win.session.settings?.file_transcription_model);
  diskRevision = $state(0);

  constructor(win: SettingsWindow) {
    this.#win = win;
  }

  readonly #fileModelCatalogController = createModelCatalogController<
    FileModelCatalogQuery,
    {
      options: WhisperModel[];
      automatic: WhisperModel;
      recommended: WhisperModel;
    }
  >({
    load: async ({ language, selected }) => {
      const [options, automatic, recommended] = await Promise.all([
        getFileModelOptions(language, selected && selected !== "auto" ? selected : undefined),
        getAutoFileModelInfo(language),
        getEffectiveModelInfo(language),
      ]);
      return { options, automatic, recommended };
    },
    publish: ({ options, automatic, recommended }) => {
      this.options = Array.isArray(options) ? options : [];
      this.auto = automatic;
      this.recommended = recommended;
    },
    reject: (error) => {
      this.#win.downloads.fileError = toErrorMessage(error);
    },
  });

  /** Load the lineup for the current language and selection; returns the cancel of that request. */
  requestLineup(): (() => void) | undefined {
    const language = this.#language;
    void this.diskRevision;
    if (language === null) return;
    const selected = this.#selection;
    const request = this.#fileModelCatalogController.request({ language, selected });
    return request.cancel;
  }

  async onChange(event: Event): Promise<void> {
    if (!this.#win.session.settings || this.saving) return;
    // Read before the first await: currentTarget is gone once the handler yields.
    const select = event.currentTarget as HTMLSelectElement;
    const id = select.value;
    this.saving = true;
    this.#win.downloads.fileError = "";
    try {
      const outcome = await this.#win.session.save(() => setFileTranscriptionModel(id));
      if (!outcome.saved) {
        // Nothing was saved. A one-way bound native select keeps the rejected
        // choice unless it is put back by hand.
        if (this.#win.session.settings) select.value = this.#win.session.settings.file_transcription_model;
        this.#win.downloads.fileError = toErrorMessage(outcome.error);
        return;
      }
      // Saved, whether or not the read-back could confirm it.
      if (isPianissimoId(id)) {
        this.#win.queue.diarize = false;
        this.#win.queue.prompt = "";
      }
    } finally {
      this.saving = false;
    }
  }

  async downloadSelected(): Promise<void> {
    const preference = this.#win.session.settings?.file_transcription_model;
    // "Auto" downloads the recommended model (it may currently use an older downloaded one).
    const id = preference === "auto" ? this.recommended?.id : preference;
    if (!id) return;
    const name =
      this.options.find((model) => model.id === id)?.display_name ?? (this.recommended?.id === id ? this.recommended.display_name : id);
    await this.#win.downloads.downloadFileModel(
      id,
      name,
      () =>
        this.#fileModelCatalogController.request({ language: this.#win.queue.language(), selected: id !== "auto" ? id : undefined }).done,
    );
  }

  /** "Download the model again" for the Pianissimo models `ids`; rejects when a download fails. */
  downloadEngineModelAgain = (ids: readonly string[]): Promise<void> => {
    return this.#win.downloads.downloadAgain(ids, (id) => this.options.find((model) => model.id === id)?.display_name ?? "Pianissimo");
  };

  /**
   * The Pianissimo model a file job's engine error is about. A Pianissimo
   * model only serves its own language, so the job's language decides: its
   * profile's, or the default profile's for a job without one.
   */
  pianissimoIdFor(job: { profileId: string | null }): string {
    const profile =
      this.#win.session.profileForId(job.profileId) ?? this.#win.session.settings?.hotkey_profiles.find((p) => p.id === "default") ?? null;
    return firstPianissimoId([pianissimoIdForLanguage(profile?.language), this.#win.session.settings?.file_transcription_model]);
  }

  dispose(): void {
    this.#fileModelCatalogController.dispose();
  }
}
