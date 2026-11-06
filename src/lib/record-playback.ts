// Playback of a finished recording's mic track (SAGA-564).
import { convertFileSrc } from "@tauri-apps/api/core";

/**
 * Playback seam. Recordings are FLAC (lossless) and the WebView plays them
 * through an `<audio>` element, which is UNVERIFIED on WKWebView (macOS) and
 * WebView2 (Windows). While this is `true`, Record.svelte replaces the player
 * with "Playback not verified, use Reveal". The owner flips it to `false` only
 * after playing a lossless and a compact recording in a signed build on both
 * platforms (checklist in docs/recorder-design.md once written).
 */
export const RECORDING_PLAYBACK_FALLBACK = true;

/** The scheme the backend registers for recording audio; TODO(SAGA-564): Rust must serve it (like `meeting-audio`). */
export const RECORDING_AUDIO_SCHEME = "recording-audio";

/**
 * The `<audio src>` for a recording's mic track: the folder name, resolved by
 * the backend under the configured recordings folder (never a filesystem path).
 */
export function recordingAudioUrl(folderName: string): string {
  return convertFileSrc(folderName, RECORDING_AUDIO_SCHEME);
}
