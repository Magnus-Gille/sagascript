const COMPLETED_DOWNLOAD_STATE = Object.freeze({
  downloading: false,
  downloadComplete: true,
  downloadProgress: 100,
});

export function completedDownloadState() {
  return { ...COMPLETED_DOWNLOAD_STATE };
}

/** @param {Promise<unknown>} downloadRequest */
export async function awaitDownloadCompletion(downloadRequest) {
  await downloadRequest;
  return completedDownloadState();
}
