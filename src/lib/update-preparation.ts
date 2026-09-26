/** Drain frontend result delivery before a native updater can restart the app. */
export async function drainUpdateWork({
  busy, settle, failure = () => null, timeoutMs = 20_000, pollMs = 25,
}: {
  busy: () => boolean;
  settle: () => Promise<unknown>;
  failure?: () => string | null;
  timeoutMs?: number;
  pollMs?: number;
}): Promise<void> {
  let expired = false;
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<never>((_, reject) => {
    timer = setTimeout(() => {
      expired = true;
      reject(new Error('Could not finish preparing transcription results. Retry the update after the current work finishes.'));
    }, timeoutMs);
  });
  const drain = async () => {
    while (!expired) {
      const error = failure();
      if (error) throw new Error(error);
      await settle();
      if (expired) return;
      const settledError = failure();
      if (settledError) throw new Error(settledError);
      if (!busy()) return;
      await new Promise(resolve => setTimeout(resolve, pollMs));
    }
  };
  try {
    await Promise.race([drain(), timeout]);
  } finally {
    expired = true;
    clearTimeout(timer);
  }
}
