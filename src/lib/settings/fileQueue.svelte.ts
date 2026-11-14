// The Transcribe tab's queue of file jobs (SAGA-379): the files added, the one
// that runs, their results' tabs, and running a file again. `jobs`,
// `savedReviewIds` and `selectedId` are the `FileQueuePort` update recovery
// reads and writes.
import { tick } from "svelte";
import type { DragDropEvent } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import type { HotkeyProfile, Language } from "../api";
import { rememberTranscription, type RecentTranscription } from "../recent-transcriptions";
import { createFileJobs, nextQueuedFile, updateFileJob, type FileJob, type FileJobStatus } from "../transcription-queue";
import type { SettingsWindow } from "./settingsWindow.svelte";

export class FileQueue {
  readonly #win: SettingsWindow;

  supportedFormats: string[] = $state([]);
  dragOver = $state(false);
  prompt = $state("");
  diarize = $state(false);
  profileId: string | null = $state(null);
  jobs = $state<FileJob[]>([]);
  selectedId = $state<string | null>(null);
  busy = $state<Record<string, boolean>>({});
  attention = $state<Record<string, boolean>>({});
  needingRetry = $derived(this.jobs.filter((job) => this.attention[job.id]));
  savedReviewIds = $state<string[]>([]);
  recent = $state<RecentTranscription[]>([]);
  selectedRerunPath = $state("");
  rerunError = $state("");
  transcribing = $derived(
    this.jobs.some((job) => job.status === "queued" || job.status === "running") || Object.values(this.busy).some(Boolean),
  );
  /** Why a "Try again" under a failed file was refused, by the job's id: shown in that file's panel. */
  retryRefusals = $state<Record<string, string>>({});

  constructor(win: SettingsWindow) {
    this.#win = win;
  }

  /** Start the next queued file when nothing runs. */
  startNextQueued(): void {
    if (this.#win.recovery.preparing) return;
    const next = nextQueuedFile(this.jobs, Object.values(this.busy).some(Boolean));
    if (next) {
      this.jobs = updateFileJob(this.jobs, next.id, "running");
    }
  }

  addFiles(paths: string[], openReview = false): void {
    if (this.#win.recovery.preparing) return;
    const jobs = createFileJobs(
      paths,
      {
        diarize: this.diarize,
        prompt: this.prompt.trim() || null,
        profileId: this.selectedProfile()?.id ?? null,
      },
      () => crypto.randomUUID(),
    );
    if (!jobs.length) return;
    if (openReview) this.savedReviewIds = [...this.savedReviewIds, ...jobs.map((job) => job.id)];
    else
      for (const job of jobs) {
        this.recent = rememberTranscription(this.recent, {
          path: job.path,
          profileId: job.profileId,
          prompt: job.prompt ?? "",
          diarize: job.diarize,
        });
        this.selectedRerunPath = job.path;
      }
    const keepSelection = this.transcribing && this.selectedId !== null;
    this.jobs = [...this.jobs, ...jobs];
    if (!keepSelection) this.selectedId = jobs[0].id;
  }

  complete = (id: string, status: FileJobStatus): void => {
    this.jobs = updateFileJob(this.jobs, id, status);
  };

  rerunSelected = (): void => {
    const run = this.recent.find((run) => run.path === this.selectedRerunPath);
    if (!run) return;
    const refusal = this.rerunRefusal(run);
    this.rerunError = refusal ?? "";
    if (refusal === null) this.queueRerun(run);
  };

  /** Why a file cannot run again, or null: the profile it ran with is gone. */
  rerunRefusal(run: RecentTranscription): string | null {
    return run.profileId && !this.#win.session.profileForId(run.profileId)
      ? "The profile for this file is no longer available. Choose a profile and open the file again."
      : null;
  }

  /**
   * Run a file again as a new job, and select it: what Re-run does, and what
   * "Try again" does for a run that failed in a way the backend says can be
   * repeated (ADR 0006). Both keep the file's profile, dictionary prompt and
   * diarization choice. Returns the new job's id.
   */
  queueRerun(run: RecentTranscription): string {
    this.recent = rememberTranscription(this.recent, run);
    // A fresh queued job preserves every previous result and review. Only the
    // serial scheduler can start it; model/decoder settings apply at execution.
    const jobs = createFileJobs([run.path], { profileId: run.profileId, prompt: run.prompt || null, diarize: run.diarize }, () =>
      crypto.randomUUID(),
    );
    this.jobs = [...this.jobs, ...jobs];
    this.selectedId = jobs[0].id;
    return jobs[0].id;
  }

  /**
   * "Try again" under a failed file: the same file, profile and prompt, as a new
   * job, whose tab then has the focus (the button that was pressed goes away with
   * the panel it was in). False when nothing was queued; a refusal is shown in the
   * file's own panel, next to the button, and not above the tab strip.
   */
  retryJob = (job: FileJob): boolean => {
    if (this.#win.recovery.preparing) return false;
    const run = { path: job.path, profileId: job.profileId, prompt: job.prompt ?? "", diarize: job.diarize };
    const refusal = this.rerunRefusal(run);
    if (refusal !== null) {
      this.retryRefusals = { ...this.retryRefusals, [job.id]: refusal };
      return false;
    }
    if (job.id in this.retryRefusals) {
      const others = { ...this.retryRefusals };
      delete others[job.id];
      this.retryRefusals = others;
    }
    const id = this.queueRerun(run);
    void tick().then(() => document.getElementById(`file-tab-${id}`)?.focus());
    return true;
  };

  setBusy = (id: string, busy: boolean): void => {
    if (this.busy[id] !== busy) this.busy = { ...this.busy, [id]: busy };
  };

  forgetMissing = (path: string): void => {
    this.recent = this.recent.filter((run) => run.path !== path);
    if (this.selectedRerunPath === path) this.selectedRerunPath = this.recent[0]?.path ?? "";
  };

  setAttention = (id: string, needsRetry: boolean): void => {
    if (this.attention[id] !== needsRetry) this.attention = { ...this.attention, [id]: needsRetry };
  };

  showNeedingRetry(id: string): void {
    this.#win.router.request("transcribe", () => {
      this.selectedId = id;
      void tick().then(() => document.getElementById(`file-tab-${id}`)?.focus());
    });
  }

  onResultTabKeydown(event: KeyboardEvent, index: number): void {
    let next: number;
    if (event.key === "ArrowRight") next = (index + 1) % this.jobs.length;
    else if (event.key === "ArrowLeft") next = (index + this.jobs.length - 1) % this.jobs.length;
    else if (event.key === "Home") next = 0;
    else if (event.key === "End") next = this.jobs.length - 1;
    else return;
    event.preventDefault();
    this.selectedId = this.jobs[next].id;
    document.getElementById(`file-tab-${this.selectedId}`)?.focus();
  }

  selectedProfile(): HotkeyProfile | null {
    return (
      this.#win.session.profileForId(this.profileId) ??
      this.#win.session.settings?.hotkey_profiles.find((profile) => profile.id === "default") ??
      this.#win.session.settings?.hotkey_profiles[0] ??
      null
    );
  }

  language(): Language {
    return this.selectedProfile()?.language ?? "auto";
  }

  // Settings can be reloaded after hotkey/profile changes. Never leave a
  // removed profile selected for file transcription.
  reconcileProfile(): void {
    const currentSettings = this.#win.session.settings;
    const currentTranscribeProfile = this.profileId;
    if (!currentSettings) return;
    if (!currentTranscribeProfile || !this.#win.session.profileForId(currentTranscribeProfile, currentSettings)) {
      this.profileId =
        currentSettings.hotkey_profiles.find((profile) => profile.id === "default")?.id ?? currentSettings.hotkey_profiles[0]?.id ?? null;
    }
  }

  onProfileChange = (e: Event) => {
    const nextProfileId = (e.target as HTMLSelectElement).value;
    this.profileId = this.#win.session.profileForId(nextProfileId)?.id ?? this.selectedProfile()?.id ?? null;
  };

  pickFiles = async () => {
    if (this.#win.recovery.preparing) return;
    const exts = this.supportedFormats.length > 0 ? this.supportedFormats : ["wav", "mp3", "m4a", "mp4", "ogg", "flac"];
    const file = await open({
      multiple: true,
      filters: [
        {
          name: "Audio/Video",
          extensions: exts,
        },
      ],
    });
    if (file && !this.#win.recovery.preparing) {
      this.addFiles(file);
    }
  };

  /** A file dragged over or dropped on the window. */
  onDragDrop(payload: DragDropEvent): void {
    if (payload.type === "over") {
      this.dragOver = true;
    } else if (payload.type === "drop") {
      this.dragOver = false;
      const paths = payload.paths;
      if (paths.length > 0 && !this.#win.recovery.preparing) {
        this.#win.router.request("transcribe", () => this.addFiles(paths));
      }
    } else {
      this.dragOver = false;
    }
  }
}
