// The host OS, asked once per window (#362). It cannot change while the app
// runs, so Onboarding, Settings and whatever follows share one lookup instead
// of each calling get_platform.
import { getPlatform } from "./api";

let lookup: Promise<string> | null = null;

/**
 * "macos", "windows" or "linux". Callers that ask while a lookup is in flight
 * share it, later callers get its answer. A failed lookup is not remembered:
 * the next call asks again.
 */
export function loadPlatform(): Promise<string> {
  lookup ??= getPlatform().catch((error) => {
    lookup = null;
    throw error;
  });
  return lookup;
}

/** Forget the answer, so the next `loadPlatform()` asks again. Tests start from their own IPC mock. */
export function forgetPlatform(): void {
  lookup = null;
}
