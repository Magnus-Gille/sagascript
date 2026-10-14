// The macOS Accessibility permission flow, shared by Onboarding and Settings
// (#362). macOS reports a grant only some time after the user ticks the box in
// System Settings, so the flow is: show the system prompt, then poll.
import { checkAccessibilityPermission, requestAccessibilityPermission } from "./api";

export interface GrantWait {
  /** Milliseconds between checks. Default 1000. */
  intervalMs?: number;
  /** Checks before giving up (`Infinity` waits until cancelled). Default 60, a minute. */
  attempts?: number;
  /** Ends the wait without a result: a newer attempt replaced it, or the window closed. */
  isCancelled?: () => boolean;
}

export interface AccessibilityRequest extends GrantWait {
  /**
   * Runs when the grant arrives while waiting, before the request resolves: the
   * registration of a bare F13 to F24 shortcut can only succeed from then on.
   */
  onGranted?: () => Promise<void>;
}

const pause = (ms: number) => new Promise<void>((resolve) => setTimeout(resolve, ms));

/**
 * Check every `intervalMs`, starting one interval from now, until macOS reports
 * Accessibility as granted. Resolves true when it is, false when the attempts
 * ran out or the wait was cancelled (a check answered after cancelling is
 * discarded). Rejects with the first failing check and stops asking.
 */
export async function waitForAccessibility({
  intervalMs = 1000,
  attempts = 60,
  isCancelled = () => false,
}: GrantWait = {}): Promise<boolean> {
  for (let attempt = 0; attempt < attempts; attempt += 1) {
    await pause(intervalMs);
    if (isCancelled()) return false;
    const granted = await checkAccessibilityPermission();
    if (isCancelled()) return false;
    if (granted) return true;
  }
  return false;
}

/**
 * Show macOS's Accessibility prompt and wait for the grant. For a user action
 * that needs the permission after a look found it missing: the prompt is only
 * allowed in response to one. Resolves true on a grant (after `onGranted`).
 */
export async function requestAccessibility({ onGranted, ...wait }: AccessibilityRequest = {}): Promise<boolean> {
  await requestAccessibilityPermission();
  if (wait.isCancelled?.()) return false;
  const granted = await waitForAccessibility(wait);
  if (granted) await onGranted?.();
  return granted;
}
