// Start the distributable GUI binary, not its --version CLI path. A valid
// signature alone cannot catch a Tauri plugin-config panic during app setup.
import { spawn } from 'node:child_process';
import { mkdtempSync, realpathSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { basename, join } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

const [appArgument] = process.argv.slice(2);
if (!appArgument || basename(appArgument) !== 'Sagascript.app') {
  throw new Error('Usage: smoke-signed-app-start.mjs /path/to/Sagascript.app');
}
const app = realpathSync(appArgument);
const directory = mkdtempSync(join(process.env.RUNNER_TEMP || tmpdir(), 'sagascript-signed-start-'));
const settingsPath = join(directory, 'settings.json');
writeFileSync(settingsPath, JSON.stringify({ has_completed_onboarding: true, auto_paste: false }), { mode: 0o600 });

const child = spawn(join(app, 'Contents/MacOS/sagascript'), ['--background'], {
  env: {
    PATH: process.env.PATH,
    HOME: process.env.HOME,
    TMPDIR: process.env.TMPDIR,
    USER: process.env.USER,
    LOGNAME: process.env.LOGNAME,
    RUST_LOG: 'sagascript=info',
    SAGASCRIPT_SETTINGS_PATH: settingsPath,
  },
  stdio: ['ignore', 'pipe', 'pipe'],
});
const closed = new Promise((resolve) => child.once('close', resolve));
let logs = '';
let timer;
try {
  await new Promise((resolve, reject) => {
    const append = (chunk) => {
      logs = `${logs}${chunk}`.slice(-4000);
      if (logs.includes('Background launch complete')) resolve();
    };
    child.stdout.on('data', append);
    child.stderr.on('data', append);
    child.once('error', reject);
    child.once('exit', (code, signal) => reject(new Error(`App exited before startup: code=${code}, signal=${signal}`)));
    timer = setTimeout(() => reject(new Error('App did not finish background startup within 30 seconds')), 30_000);
  });
  await delay(2_000);
  if (child.exitCode !== null || child.signalCode !== null) {
    throw new Error('App exited immediately after background startup');
  }
  console.log('Signed Sagascript app completed background startup');
} catch (error) {
  console.error(logs);
  throw error;
} finally {
  clearTimeout(timer);
  if (child.pid && child.exitCode === null && child.signalCode === null) child.kill('SIGTERM');
  await Promise.race([closed, delay(3_000)]);
  if (child.pid && child.exitCode === null && child.signalCode === null) child.kill('SIGKILL');
  rmSync(directory, { recursive: true, force: true });
}
