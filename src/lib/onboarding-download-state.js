const COMPLETED_DOWNLOAD_STATE = Object.freeze({
  downloading: false,
  downloadComplete: true,
  downloadProgress: 100,
});

/** @typedef {typeof COMPLETED_DOWNLOAD_STATE} DownloadCompletionState */

export function completedDownloadState() {
  return { ...COMPLETED_DOWNLOAD_STATE };
}

/**
 * @param {Promise<unknown>} downloadRequest
 * @param {(state: DownloadCompletionState) => void} onComplete
 */
export async function awaitDownloadCompletion(downloadRequest, onComplete) {
  await downloadRequest;
  const completed = completedDownloadState();
  onComplete(completed);
  return completed;
}

/**
 * @param {() => Promise<() => void>} registerProgress
 * @param {() => Promise<() => void>} registerReady
 * @param {() => boolean} isCancelled
 * @returns {Promise<[() => void, () => void] | null>}
 */
export async function registerDownloadListeners(registerProgress, registerReady, isCancelled) {
  /** @param {() => Promise<() => void>} register */
  const ownRegistration = async (register) => {
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

  return /** @type {[() => void, () => void]} */ (owned);
}

/**
 * @param {[() => void, () => void] | null} listeners
 * @param {() => boolean} isCancelled
 * @param {(progress: () => void, ready: () => void) => void} adopt
 */
export function adoptDownloadListeners(listeners, isCancelled, adopt) {
  if (!listeners) return false;
  if (isCancelled()) {
    listeners.forEach((unlisten) => unlisten());
    return false;
  }
  adopt(...listeners);
  return true;
}
