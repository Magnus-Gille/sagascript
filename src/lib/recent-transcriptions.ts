export interface RecentTranscription {
  path: string;
  profileId: string | null;
  prompt: string;
  diarize: boolean;
}

/** Session-only re-run requests, without audio or transcript results. */
export function rememberTranscription(
  history: RecentTranscription[], run: RecentTranscription,
): RecentTranscription[] {
  return [{ ...run }, ...history.filter(item => item.path !== run.path)].slice(0, 5);
}
