export type RunStatus = "idle" | "running" | "completed" | "failed" | "cancelled";
export type StageStatus = "pending" | "active" | "completed" | "failed" | "cancelled";
export interface StageState {
  status: RunStatus;
  index: number;
  phase: string;
  percent: number | null;
}
export interface StageRow {
  title: string;
  status: StageStatus;
  detail: string;
  percent: number | null;
}

const titles = ["Read audio", "Prepare audio & model", "Transcribe"];
const phaseStages: Record<string, number> = {
  decoding: 0, resampling: 1, loading: 1, language_detection: 1,
  preparing: 1, encoding: 1, transcribing: 2, finalizing: 2,
};

export function initialStages(): StageState {
  return { status: "idle", index: 0, phase: "decoding", percent: 0 };
}
export function startStages(): StageState {
  return { ...initialStages(), status: "running" };
}

export function acceptRunProgress(state: StageState, runId: string | null, payload: unknown): StageState {
  if (!runId || !payload || typeof payload !== "object") return state;
  const event = payload as { runId?: unknown; phase?: unknown; percent?: unknown };
  if (event.runId !== runId || typeof event.phase !== "string") return state;
  return updateStages(state, event.phase, event.percent);
}

/** Consumes the phase vocabulary emitted by CLI --progress-json and the GUI
 * callbacks. Completion comes only from a successful command result, never a
 * native 100% callback. Preparatory percentages refer to audio conversion only.
 */
export function updateStages(state: StageState, phase: string, reported?: unknown): StageState {
  if (state.status !== "running" || !Object.hasOwn(phaseStages, phase)) return state;
  const index = phaseStages[phase];
  if (index < state.index) return state;
  const measured = phase === "decoding" || phase === "resampling" || phase === "transcribing";
  let percent: number | null = null;
  if (measured) {
    if (typeof reported !== "number" || !Number.isFinite(reported)) return state;
    percent = Math.max(0, Math.min(100, Math.floor(reported)));
    if (phase === "transcribing" && percent === 0) return state;
    if (phase === state.phase) percent = Math.max(state.percent ?? 0, percent);
  }
  if (phase === "decoding" && percent === 100) {
    return { status: "running", index: 1, phase: "preparing", percent: null };
  }
  if (phase === "resampling" && percent === 100) {
    return { status: "running", index: 1, phase: "preparing", percent: null };
  }
  return { status: "running", index, phase, percent };
}

export function finishStages(state: StageState, status: "completed" | "failed" | "cancelled"): StageState {
  if (state.status !== "running") return state;
  return status === "completed"
    ? { status, index: 2, phase: "completed", percent: 100 }
    : { ...state, status };
}

function activeDetail(state: StageState): string {
  switch (state.phase) {
    case "decoding": return "Decoding the file";
    case "resampling": return "Converting to 16 kHz mono";
    case "loading": return "Loading the speech model";
    case "language_detection": return "Checking the language";
    case "preparing": return "Preparing the speech engine";
    case "encoding": return "Analysing sound before the first words";
    case "transcribing": return state.percent === 100 ? "Finishing transcript…" : "Recognising words";
    case "finalizing": return "Finishing transcript…";
    default: return "";
  }
}

export function stageRows(state: StageState): StageRow[] {
  return titles.map((title, index) => {
    let status: StageStatus = "pending";
    if (state.status !== "idle") {
      if (state.status === "completed" || index < state.index) status = "completed";
      else if (index === state.index) status = state.status === "running" ? "active" : state.status;
    }
    const detail = status === "active" ? activeDetail(state)
      : status === "completed" ? "Done"
      : status === "failed" ? "Failed"
      : status === "cancelled" ? "Cancelled" : "Waiting";
    return { title, status, detail, percent: status === "completed" ? 100
      : index === state.index && status !== "pending" ? state.percent : 0 };
  });
}
