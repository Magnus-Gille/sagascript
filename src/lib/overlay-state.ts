// Pure state machine for the dictation overlay. The component feeds it backend
// events and one timer callback; everything else (labels, flicker threshold)
// lives here so it can be tested without a DOM.

export type OverlayPhase = "recording" | "transcribing" | "loading_model" | "idle" | "error";
export type EngineLoad = "unknown" | "loading" | "ready" | "failed";

export interface OverlayState {
  phase: OverlayPhase;
  engine: EngineLoad;
  /** True once the engine has been loading, while working, for LOADING_LABEL_DELAY_MS. */
  loadingVisible: boolean;
}

export type OverlayAction =
  | { type: "state"; value: string }
  | { type: "engine"; value: string; source?: string }
  | { type: "loading-delay-elapsed" };

/** Loads faster than this never show the "Loading model" label (no flicker). */
export const LOADING_LABEL_DELAY_MS = 150;
/** How long the backend keeps the overlay up after a failure. */
export const ERROR_VISIBLE_MS = 2500;

export const initialOverlayState: OverlayState = {
  phase: "recording",
  engine: "unknown",
  loadingVisible: false,
};

const PHASES: readonly string[] = ["recording", "transcribing", "loading_model", "idle", "error"];

export function isWorking(phase: OverlayPhase): boolean {
  return phase === "transcribing" || phase === "loading_model";
}

/** The delay timer runs only while the user is waiting on a model load. */
export function needsLoadingTimer(state: OverlayState): boolean {
  return isWorking(state.phase) && state.engine === "loading" && !state.loadingVisible;
}

function settle(state: OverlayState): OverlayState {
  const waiting = isWorking(state.phase) && state.engine === "loading";
  return waiting || !state.loadingVisible ? state : { ...state, loadingVisible: false };
}

export function reduceOverlay(state: OverlayState, action: OverlayAction): OverlayState {
  switch (action.type) {
    case "state": {
      if (!PHASES.includes(action.value)) return state;
      const phase = action.value as OverlayPhase;
      // The error state sticks until the next recording; the backend's
      // follow-up "idle" must not erase it before the user can read it.
      if (state.phase === "error" && phase !== "recording") return state;
      // A new recording starts clean: stale engine state (a failed or
      // cancelled load) must not leak into this dictation. The backend
      // re-sends the live load state when it shows the overlay.
      const engine = phase === "recording" ? "unknown" : state.engine;
      return settle({ ...state, phase, engine });
    }
    case "engine": {
      let engine: EngineLoad =
        action.value === "loading" ||
        action.value === "ready" ||
        action.value === "failed" ||
        action.value === "unknown"
          ? action.value
          : state.engine;
      // Only a failure of the load this dictation is waiting on is an error.
      // A background warm-up failure (wake, profile change) is not.
      const background = engine === "failed" && action.source === "warm";
      if (background) engine = "unknown";
      if (engine === state.engine) return state;
      const phase: OverlayPhase =
        engine === "failed" && isWorking(state.phase) ? "error" : state.phase;
      return settle({ ...state, engine, phase });
    }
    case "loading-delay-elapsed":
      return needsLoadingTimer(state) ? { ...state, loadingVisible: true } : state;
  }
}

type Strings = {
  recording: string;
  transcribing: string;
  loading: string;
  loadFailed: string;
  failed: string;
  languages: Record<string, string>;
};

const STRINGS: Record<"en" | "sv", Strings> = {
  en: {
    recording: "Recording",
    transcribing: "Transcribing…",
    loading: "Loading model…",
    loadFailed: "Model failed to load",
    failed: "Transcription failed",
    languages: { en: "English", sv: "Swedish", no: "Norwegian", fi: "Finnish", auto: "Auto" },
  },
  sv: {
    recording: "Spelar in",
    transcribing: "Transkriberar…",
    loading: "Laddar modell…",
    loadFailed: "Laddning misslyckades",
    failed: "Misslyckades",
    languages: { en: "engelska", sv: "svenska", no: "norska", fi: "finska", auto: "auto" },
  },
};

export function overlayStrings(locale: string | undefined): Strings {
  return /^sv\b/i.test(locale ?? "") ? STRINGS.sv : STRINGS.en;
}

export function overlayLabel(
  state: OverlayState,
  profileLanguage: string | null,
  locale: string | undefined,
): string {
  const text = overlayStrings(locale);
  if (state.phase === "error") return state.engine === "failed" ? text.loadFailed : text.failed;
  if (isWorking(state.phase)) return state.loadingVisible ? text.loading : text.transcribing;
  if (profileLanguage) return `${text.recording} · ${text.languages[profileLanguage] ?? profileLanguage}`;
  return `${text.recording}…`;
}
