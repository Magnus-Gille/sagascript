import { mkdtempSync, writeFileSync } from 'node:fs';
import { join } from 'node:path';

// An upgrade smoke represents an existing onboarded installation. A fresh
// runner otherwise opens Onboarding, which has no recovery acknowledgement.
export function prepareUpdaterSmokeEnvironment(runnerTemp, baseEnvironment = process.env) {
  const directory = mkdtempSync(join(runnerTemp, 'sagascript-updater-settings-'));
  const settingsPath = join(directory, 'settings.json');
  writeFileSync(settingsPath, JSON.stringify({ has_completed_onboarding: true, auto_paste: false }), { mode: 0o600 });
  return { ...baseEnvironment, SAGASCRIPT_SETTINGS_PATH: settingsPath };
}
