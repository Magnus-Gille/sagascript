// Model ids and downloads shared by Settings and Onboarding (#362).
import { downloadModel, downloadPianissimoModel } from "./api";
import { useTauriEvent } from "./tauri-events.svelte";

/**
 * The id of the Pianissimo (Swedish) model on the wire: what the backend's
 * `FileModel::PianissimoOriginal` serializes to (sagascript-core
 * `FileModel::PIANISSIMO_ID` in models/file_model.rs, pinned by its tests). `get_file_model_options`,
 * `profile_models` and `file_transcription_model` carry it.
 */
export const PIANISSIMO_ID = "pianissimo-sv";

/**
 * Download one model. Pianissimo has its own command (its own platform check
 * and bundle); every other model goes through `download_model` by id. Both
 * emit `model-download-progress` and `model-ready`.
 */
export async function downloadModelById(id: string): Promise<void> {
  if (id === PIANISSIMO_ID) await downloadPianissimoModel();
  else await downloadModel(id);
}

/** Which model is downloading, its name and how far it is, for one window. */
export class ModelDownloads {
  /** The id of the model being downloaded, or `null` when none is. */
  id = $state<string | null>(null);
  name = $state("");
  /** Percent, 0 to 100. */
  progress = $state(0);

  /** One download at a time: callers refuse to start another while this is true. */
  get busy(): boolean {
    return this.id !== null;
  }

  start(id: string, name: string): void {
    this.id = id;
    this.name = name;
    this.progress = 0;
  }

  /** Call from a `finally`: a failed download clears the state like a finished one. */
  finish(): void {
    this.id = null;
    this.progress = 0;
  }
}

/** A `ModelDownloads` whose progress follows `model-download-progress` for the life of the component. */
export function useModelDownloads(): ModelDownloads {
  const downloads = new ModelDownloads();
  useTauriEvent("model-download-progress", ({ progress }) => {
    downloads.progress = progress;
  });
  return downloads;
}
