// Model ids and downloads shared by Settings and Onboarding (#362).
import { downloadModel, downloadPianissimoModel } from "./api";
import { useTauriEvent } from "./tauri-events.svelte";

/**
 * The ids of the Pianissimo models on the wire, one per language: what the
 * backend's `PianissimoModel::id` returns (sagascript-core models/pianissimo.rs,
 * pinned by its tests). `get_file_model_options`, `profile_models` and
 * `file_transcription_model` carry them. `PIANISSIMO_ID` is the Swedish one.
 */
export const PIANISSIMO_ID = "pianissimo-sv";
export const PIANISSIMO_DA_ID = "pianissimo-da";
const PIANISSIMO_IDS: readonly string[] = [PIANISSIMO_ID, PIANISSIMO_DA_ID];

/** Whether `id` names one of the Pianissimo models. */
export function isPianissimoId(id: string | null | undefined): boolean {
  return id != null && PIANISSIMO_IDS.includes(id);
}

/** The Pianissimo model of `language`, if it has one: each serves only its own language. */
export function pianissimoIdForLanguage(language: string | null | undefined): string | null {
  if (language === "sv") return PIANISSIMO_ID;
  if (language === "da") return PIANISSIMO_DA_ID;
  return null;
}

/** The distinct Pianissimo ids among `ids`, in order; the Swedish one when there are none. */
export function pianissimoIdsOf(ids: readonly (string | null | undefined)[]): string[] {
  const pianissimo = ids.filter(isPianissimoId) as string[];
  const found = pianissimo.filter((id, index) => pianissimo.indexOf(id) === index);
  return found.length > 0 ? found : [PIANISSIMO_ID];
}

/** The first Pianissimo id among `ids` (most relevant first), else the Swedish one. */
export function firstPianissimoId(ids: readonly (string | null | undefined)[]): string {
  return (ids.find(isPianissimoId) as string | undefined) ?? PIANISSIMO_ID;
}

/**
 * Download one model. Pianissimo has its own command (its own platform check
 * and bundle); every other model goes through `download_model` by id. Both
 * emit `model-download-progress` and `model-ready`.
 */
export async function downloadModelById(id: string): Promise<void> {
  // The command's default is the Swedish model, so its call stays as it was.
  if (isPianissimoId(id)) await downloadPianissimoModel(id === PIANISSIMO_ID ? undefined : id);
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
