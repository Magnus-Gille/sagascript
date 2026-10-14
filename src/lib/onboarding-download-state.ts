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
