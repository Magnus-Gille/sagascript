import type { PlainTranscriptionProgress } from "./events";
import type { Subscription } from "./tauri-events.svelte";

export interface PlainTranscriptionRequest {
  path: string;
  prompt?: string;
  profileId?: string;
  runId: string;
}

export interface PlainTranscriptionRunPorts {
  waitForMeetingActions(): Promise<void>;
  subscribeProgress(handler: (payload: PlainTranscriptionProgress) => void): Subscription;
  transcribeFile(path: string, options: { prompt?: string; diarize: false; profileId?: string; runId: string }): Promise<string>;
  cancelFileTranscription(runId: string): Promise<boolean>;
  onStart(request: PlainTranscriptionRequest): void;
  onProgress(payload: PlainTranscriptionProgress): void;
  onCancelledBeforeSubmission(): void;
  onResult(text: string, cancellationRequested: boolean): void;
  onError(error: unknown, cancellationRequested: boolean, path: string): void;
  onFinished(): void;
  onCancelRequested(): void;
  onCancelError(error: unknown): void;
}

interface ActiveRun {
  request: PlainTranscriptionRequest;
  cancellationRequested: boolean;
  submitted: boolean;
}

/** Own the single plain-file run, including its listener and native IPC lifetime. */
export function createPlainTranscriptionRun(ports: PlainTranscriptionRunPorts) {
  let active: ActiveRun | null = null;

  async function start(request: PlainTranscriptionRequest): Promise<void> {
    if (active) return;

    const run: ActiveRun = { request, cancellationRequested: false, submitted: false };
    active = run;
    let progress: Subscription | undefined;
    try {
      ports.onStart(request);
      await ports.waitForMeetingActions();
      progress = ports.subscribeProgress((payload) => {
        if (active === run && payload.runId === run.request.runId) ports.onProgress(payload);
      });
      await progress.ready;
      if (run.cancellationRequested) {
        ports.onCancelledBeforeSubmission();
        return;
      }

      run.submitted = true;
      const result = await ports.transcribeFile(run.request.path, {
        prompt: run.request.prompt,
        diarize: false,
        profileId: run.request.profileId,
        runId: run.request.runId,
      });
      ports.onResult(result, run.cancellationRequested);
    } catch (error) {
      ports.onError(error, run.cancellationRequested, run.request.path);
    } finally {
      progress?.dispose();
      ports.onFinished();
      if (active === run) active = null;
    }
  }

  async function cancel(): Promise<void> {
    const run = active;
    if (!run || run.cancellationRequested) return;
    run.cancellationRequested = true;
    ports.onCancelRequested();
    if (!run.submitted) return;

    try {
      await ports.cancelFileTranscription(run.request.runId);
    } catch (error) {
      if (active !== run) return;
      run.cancellationRequested = false;
      ports.onCancelError(error);
    }
  }

  return { start, cancel };
}
