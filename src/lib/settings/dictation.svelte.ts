// The Dictate tab's "Try it out" recorder and its result editor (SAGA-379).
// `text`, `recoveryPending`, `edited` and `transcribing` are the
// `DictationEditorPort` update recovery reads and writes.
import {
  copyTranscriptionText,
  getLastError,
  getLastTranscription,
  saveTranscriptionText,
  startRecording,
  stopAndTranscribe,
} from "../api";
import { dictateButtonAction, retainTestRecordingOwnership, type BackendDictationState } from "../dictation-ui-state";
import { backendErrorRecovery, downloadFailureRecovery, toErrorMessage, type BackendError, type BackendErrorRecovery } from "../errors";
import type { UiState } from "../events";
import type { SettingsWindow } from "./settingsWindow.svelte";

export class Dictation {
  readonly #win: SettingsWindow;

  recording: boolean = $state(false);
  transcribing: boolean = $state(false);
  backendState: BackendDictationState = $state("idle");
  ownsRecording: boolean = $state(false);
  text: string = $state("");
  error: string = $state("");
  /** The way out of `error`, decided by the error's code (`backendErrorRecovery`); null for plain text. */
  errorRecovery: BackendErrorRecovery | null = $state(null);
  actionMessage: string = $state("");
  recoveryPending: boolean = $state(false);
  edited: boolean = $state(false);
  eventRevision = 0;

  constructor(win: SettingsWindow) {
    this.#win = win;
  }

  showError(failure: unknown, fallback: string): void {
    this.error = toErrorMessage(failure, fallback);
    this.errorRecovery = backendErrorRecovery(failure);
  }

  clearError(): void {
    this.error = "";
    this.errorRecovery = null;
  }

  async copyResult(): Promise<void> {
    if (!this.text.trim()) return;
    try {
      const text = this.text;
      const delivery = this.#win.recovery.captureDelivery(text);
      await copyTranscriptionText(text);
      const nextResultShown = await this.#win.recovery.markDelivered(delivery);
      this.actionMessage = nextResultShown ? "Copied result. Another unsaved dictation is now shown." : "Copied to clipboard.";
    } catch (error) {
      this.actionMessage = toErrorMessage(error);
    }
  }

  async saveResult(): Promise<void> {
    if (!this.text.trim()) return;
    try {
      const text = this.text;
      const delivery = this.#win.recovery.captureDelivery(text);
      const saved = await saveTranscriptionText(text, "dictation.txt", null);
      if (saved) {
        const nextResultShown = await this.#win.recovery.markDelivered(delivery);
        this.actionMessage = nextResultShown ? "Saved result. Another unsaved dictation is now shown." : "Saved.";
      }
      if (!saved) this.actionMessage = "Save cancelled — nothing was written.";
    } catch (error) {
      this.actionMessage = toErrorMessage(error);
    }
  }

  onError(failure: BackendError): void {
    this.eventRevision++;
    this.showError(failure, "Transcription failed");
    this.#win.router.request("dictate");
  }

  onResult(text: string): void {
    this.eventRevision++;
    this.#win.recovery.showNativeResult(text);
    this.clearError();
  }

  /** Recover the persisted-in-memory result without racing newer events. */
  async restoreLast(isDisposed: () => boolean): Promise<void> {
    const initialRevision = this.eventRevision;
    await this.#win.recovery.restored.catch(() => undefined);
    const [error, text] = await Promise.all([getLastError(), getLastTranscription()]);
    if (!isDisposed() && this.eventRevision === initialRevision) {
      // The last error is plain text: it has no code to decide a recovery by.
      this.error = error ?? "";
      this.errorRecovery = null;
      this.#win.recovery.nativeResultSettled(text);
    }
  }

  async onStateChanged(nextState: UiState): Promise<void> {
    if (nextState === "recording") {
      // A new dictation supersedes the restored result and error, and
      // outdates the restore that waits for the listeners above.
      this.eventRevision++;
      this.#win.recovery.dictationStarted();
      this.clearError();
    }

    if (nextState === "settings_reloaded") {
      await this.#win.session.sync();
      return;
    }

    if (!["idle", "recording", "loading_model", "transcribing"].includes(nextState)) return;
    const acceptedState = nextState as BackendDictationState;
    this.ownsRecording = retainTestRecordingOwnership(acceptedState, this.ownsRecording);
    this.backendState = acceptedState;
    this.recording = nextState === "recording";
    this.transcribing = nextState === "loading_model" || nextState === "transcribing";
  }

  /** "Download the model again" under the Dictate tab's error. */
  async downloadModelAgain(): Promise<void> {
    if (this.#win.recovery.preparing || this.#win.downloads.busy || this.#win.downloads.repairing) return;
    const revision = this.eventRevision;
    try {
      await this.#win.fileModel.downloadEngineModelAgain(this.#win.profileModels.dictationPianissimoIds());
      // The model is there: the action can be repeated. Unless another event
      // replaced the error meanwhile (even one with the same text): that one
      // decides its own recovery.
      if (this.eventRevision === revision) this.errorRecovery = "retry";
    } catch (failure) {
      this.showError(failure, "Speech engine download failed.");
      // The download again, only where it can pass; a refusal offers nothing.
      this.errorRecovery = downloadFailureRecovery(failure);
    }
  }

  toggleRecording = async () => {
    if (this.#win.recovery.preparing || this.#win.profileModels.saving || this.#win.downloads.busy) return;
    const action = dictateButtonAction(this.backendState, this.ownsRecording);
    if (action === "blocked") return;

    if (action === "stop") {
      // Stop and transcribe
      this.ownsRecording = false;
      this.recording = false;
      this.transcribing = true;
      this.backendState = "transcribing";
      this.clearError();
      try {
        const text = await stopAndTranscribe();
        this.#win.recovery.appendTestRecording(text);
      } catch (e) {
        this.showError(e, "Transcription failed");
      } finally {
        this.transcribing = false;
        this.backendState = "idle";
      }
    } else {
      // Start recording
      this.clearError();
      try {
        await startRecording();
        this.ownsRecording = true;
        this.recording = true;
        this.backendState = "recording";
      } catch (e) {
        this.ownsRecording = false;
        this.backendState = "idle";
        this.showError(e, "Failed to start recording");
      }
    }
  };
}
