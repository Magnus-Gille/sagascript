// Exercise the installed app's real updater against a signed archive on loopback.
// The test app lives under RUNNER_TEMP, so the updater never touches /Applications.
import { createServer } from 'node:http';
import { spawn, execFileSync } from 'node:child_process';
import { readFileSync, realpathSync } from 'node:fs';
import { resolve, join, sep } from 'node:path';
import { setTimeout as delay } from 'node:timers/promises';

const [appArgument, archiveArgument, signatureArgument, oldVersion, newVersion] = process.argv.slice(2);
if (!appArgument || !archiveArgument || !signatureArgument || !/^\d+\.\d+\.\d+$/.test(oldVersion) || !/^\d+\.\d+\.\d+$/.test(newVersion)) {
  throw new Error('Usage: smoke-signed-updater.mjs APP ARCHIVE SIGNATURE OLD_VERSION NEW_VERSION');
}
const app = realpathSync(appArgument);
const runnerTemp = realpathSync(process.env.RUNNER_TEMP || '/nonexistent');
if (!app.startsWith(`${runnerTemp}${sep}`) || !app.endsWith(`${sep}Sagascript.app`)) {
  throw new Error('The updater smoke app must be a Sagascript.app inside RUNNER_TEMP');
}
const executable = join(app, 'Contents/MacOS/sagascript');
const plist = join(app, 'Contents/Info.plist');
const archive = readFileSync(resolve(archiveArgument));
const signature = readFileSync(resolve(signatureArgument), 'utf8').trim();
if (!archive.length || !signature) throw new Error('Signed updater archive or signature is empty');

function installedVersion() {
  return execFileSync('/usr/libexec/PlistBuddy', ['-c', 'Print :CFBundleShortVersionString', plist], { encoding: 'utf8' }).trim();
}

function appPids() {
  const output = execFileSync('ps', ['-axo', 'pid=,command='], { encoding: 'utf8' });
  return output.split('\n').flatMap((line) => {
    const match = line.match(/^\s*(\d+)\s+(.+)$/);
    return match && (match[2] === executable || match[2].startsWith(`${executable} `))
      ? [Number(match[1])]
      : [];
  });
}

if (installedVersion() !== oldVersion) {
  throw new Error(`Expected initial app ${oldVersion}, got ${installedVersion()}`);
}
const manifest = JSON.stringify({
  version: newVersion,
  platforms: {
    'darwin-aarch64': {
      url: 'http://127.0.0.1:34827/Sagascript.app.tar.gz',
      signature,
    },
  },
});
const server = createServer((request, response) => {
  if (request.url === '/latest.json') {
    response.writeHead(200, { 'Content-Type': 'application/json', 'Content-Length': Buffer.byteLength(manifest) });
    response.end(manifest);
  } else if (request.url === '/Sagascript.app.tar.gz') {
    response.writeHead(200, { 'Content-Type': 'application/gzip', 'Content-Length': archive.length });
    response.end(archive);
  } else {
    response.writeHead(404);
    response.end();
  }
});

let child;
let logs = '';
try {
  await new Promise((accept, reject) => {
    server.once('error', reject);
    server.listen(34827, '127.0.0.1', accept);
  });
  child = spawn(executable, ['--install-update'], {
    stdio: ['ignore', 'pipe', 'pipe'],
    env: process.env,
  });
  const appendLog = (chunk) => {
    logs = `${logs}${chunk}`.slice(-20000);
  };
  child.stdout.on('data', appendLog);
  child.stderr.on('data', appendLog);
  child.on('error', appendLog);

  const deadline = Date.now() + 180_000;
  let restartedPid;
  while (Date.now() < deadline) {
    if (installedVersion() === newVersion) {
      restartedPid = appPids().find((pid) => pid !== child.pid);
      if (restartedPid) break;
    }
    await delay(500);
  }
  if (!restartedPid) {
    throw new Error(`Signed update did not install and restart: version=${installedVersion()}, initialPid=${child.pid}, runningPids=${appPids().join(',')}`);
  }
  execFileSync('codesign', ['--verify', '--deep', '--strict', app], { stdio: 'inherit' });
  console.log(`Signed updater installed ${oldVersion} → ${newVersion} and restarted PID ${child.pid} → ${restartedPid}`);
} catch (error) {
  console.error(logs);
  throw error;
} finally {
  for (const pid of appPids()) {
    try { process.kill(pid, 'SIGTERM'); } catch { /* already exited */ }
  }
  if (server.listening) await new Promise((accept) => server.close(accept));
}
