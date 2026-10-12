const COMPLETED_DOWNLOAD_STATE = Object.freeze({
  downloading: false,
  downloadComplete: true,
  downloadProgress: 100,
});

export interface DownloadCompletionState {
  downloading: boolean;
  downloadComplete: boolean;
  downloadProgress: number;
}

/** Stops one event listener (what `listen()` resolves with). */
export type Unlisten = () => void;
/** The progress and ready listeners, owned together. */
export type DownloadListeners = [progress: Unlisten, ready: Unlisten];

export function completedDownloadState(): DownloadCompletionState {
  return { ...COMPLETED_DOWNLOAD_STATE };
}

export async function awaitDownloadCompletion(
  downloadRequest: Promise<unknown>,
  onComplete: (state: DownloadCompletionState) => void,
): Promise<DownloadCompletionState> {
  await downloadRequest;
  const completed = completedDownloadState();
  onComplete(completed);
  return completed;
}

export async function registerDownloadListeners(
  registerProgress: () => Promise<Unlisten>,
  registerReady: () => Promise<Unlisten>,
  isCancelled: () => boolean,
): Promise<DownloadListeners | null> {
  const ownRegistration = async (register: () => Promise<Unlisten>): Promise<Unlisten | null> => {
    const unlisten = await register();
    if (isCancelled()) {
      unlisten();
      return null;
    }
    return unlisten;
  };

  const results = await Promise.allSettled([
    ownRegistration(registerProgress),
    ownRegistration(registerReady),
  ]);
  const owned = results.flatMap((result) =>
    result.status === "fulfilled" && result.value ? [result.value] : [],
  );
  const failure = results.find((result) => result.status === "rejected");

  if (isCancelled() || failure || owned.length !== 2) {
    owned.forEach((unlisten) => unlisten());
    if (failure) throw failure.reason;
    return null;
  }

  return [owned[0], owned[1]];
}

export function adoptDownloadListeners(
  listeners: DownloadListeners | null,
  isCancelled: () => boolean,
  adopt: (progress: Unlisten, ready: Unlisten) => void,
): boolean {
  if (!listeners) return false;
  if (isCancelled()) {
    listeners.forEach((unlisten) => unlisten());
    return false;
  }
  adopt(...listeners);
  return true;
}
