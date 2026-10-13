// Run a command as its own process tree with a time budget, and stop the whole
// tree on timeout, on interruption and when it leaves processes behind (#350).
//
// POSIX: the child is spawned detached, i.e. as the leader of a new process
// group, so `kill(-pgid)` reaches every descendant that did not start its own
// session. Windows: `taskkill /T /F` stops the tree while its root is alive.
import { spawn, spawnSync } from "node:child_process";
import { readdirSync, readFileSync } from "node:fs";
import { constants } from "node:os";

const isWindows = process.platform === "win32";

/** True while `pid` names a live process (zombies count as gone). */
export function isProcessAlive(pid) {
  try {
    process.kill(pid, 0);
  } catch (error) {
    return error.code === "EPERM";
  }
  if (process.platform === "linux") {
    const state = linuxProcessState(pid);
    return state !== null && state !== "Z" && state !== "X";
  }
  return true;
}

function linuxProcessState(pid) {
  try {
    const stat = readFileSync(`/proc/${pid}/stat`, "utf8");
    // Format: "pid (comm) state ppid pgrp ..."; comm may itself contain ") ".
    return stat.slice(stat.lastIndexOf(")") + 2).split(" ")[0];
  } catch {
    return null;
  }
}

// Live (non-zombie) members of a process group. Linux reads /proc so zombies
// left by a non-reaping PID 1 (containers) do not count; elsewhere a signal-0
// probe of the group answers "any member left?".
function groupHasLiveMembers(pgid) {
  if (process.platform === "linux") {
    let entries;
    try {
      entries = readdirSync("/proc");
    } catch {
      entries = null;
    }
    if (entries) {
      for (const entry of entries) {
        if (!/^\d+$/.test(entry)) continue;
        let stat;
        try {
          stat = readFileSync(`/proc/${entry}/stat`, "utf8");
        } catch {
          continue;
        }
        const [state, , pgrp] = stat.slice(stat.lastIndexOf(")") + 2).split(" ");
        if (Number(pgrp) === pgid && state !== "Z" && state !== "X") return true;
      }
      return false;
    }
  }
  try {
    process.kill(-pgid, 0);
    return true;
  } catch (error) {
    return error.code === "EPERM";
  }
}

/** Signal the process tree rooted at `pid` (spawned by `runProcessTree`). */
export function killProcessTree(pid, signal = "SIGTERM") {
  if (!pid) return;
  if (isWindows) {
    // /F: Windows has no graceful equivalent of SIGTERM for console trees.
    spawnSync("taskkill", ["/PID", String(pid), "/T", "/F"], { stdio: "ignore", windowsHide: true });
    return;
  }
  try {
    process.kill(-pid, signal);
  } catch (error) {
    if (error.code !== "ESRCH") throw error;
  }
}

/** Exit status a shell reports for a process ended by `signal` (SIGINT → 130). */
export function signalExitCode(signal) {
  return 128 + (constants.signals[signal] ?? 1);
}

/**
 * Call `onSignal(name)` on SIGINT, SIGTERM and SIGHUP instead of dying, so the
 * caller can stop its children and report. Returns a function that removes
 * the handlers.
 */
export function trapTerminationSignals(onSignal) {
  const handlers = ["SIGINT", "SIGTERM", "SIGHUP"].map((name) => {
    const handler = () => onSignal(name);
    process.on(name, handler);
    return [name, handler];
  });
  return () => {
    for (const [name, handler] of handlers) process.off(name, handler);
  };
}

/**
 * Run `command args` with a time budget.
 *
 * Resolves (never rejects) with { code, signal, error, timedOut, abortedBy,
 * leftovers, stdout, durationMs, pid }. On timeout or when `abortSignal`
 * fires, the tree gets SIGTERM and, `killGraceMs` later, SIGKILL. After the
 * root exits, processes still in its group get `lingerMs` to exit on their
 * own (helpers such as esbuild follow their parent a moment later); the rest
 * are killed and `leftovers` is set, so no child outlives the call (POSIX).
 *
 * With `captureStdout` the call ends when the root exits, after at most
 * `drainMs` for the pipe to close: a descendant that left the group (setsid)
 * may hold the pipe open indefinitely, and its output is not waited for.
 * `onStdout` sees each captured chunk as it arrives (to tee it).
 */
export function runProcessTree({
  command,
  args = [],
  cwd,
  env,
  timeoutMs,
  killGraceMs = 5000,
  lingerMs = 500,
  drainMs = 500,
  abortSignal,
  captureStdout = false,
  onStdout,
  shell = false,
}) {
  return new Promise((resolveRun) => {
    const started = Date.now();
    const chunks = [];
    let child;
    let timedOut = false;
    let abortedBy = null;
    let timer;
    let killTimer;
    let drainTimer;
    let terminating = false;
    let settled = false;

    const destroyPipes = () => child?.stdout?.destroy();
    const terminate = () => {
      if (terminating || !child?.pid) return;
      terminating = true;
      killProcessTree(child.pid, "SIGTERM");
      killTimer = setTimeout(() => {
        killProcessTree(child.pid, "SIGKILL");
        destroyPipes();
      }, killGraceMs);
    };
    const onAbort = () => {
      abortedBy = abortSignal.reason ?? "aborted";
      terminate();
    };
    const finish = (outcome) => {
      if (settled) return;
      settled = true;
      clearTimeout(timer);
      clearTimeout(killTimer);
      clearTimeout(drainTimer);
      abortSignal?.removeEventListener("abort", onAbort);
      const durationMs = Date.now() - started;
      const done = (leftovers) =>
        resolveRun({
          code: null,
          signal: null,
          error: null,
          ...outcome,
          timedOut,
          abortedBy,
          leftovers,
          stdout: captureStdout ? Buffer.concat(chunks).toString("utf8") : undefined,
          durationMs,
          pid: child?.pid,
        });
      if (isWindows || !child?.pid) {
        done(false);
        return;
      }
      const pgid = child.pid;
      const lingerUntil = Date.now() + lingerMs;
      const sweep = () => {
        if (!groupHasLiveMembers(pgid)) {
          done(false);
        } else if (Date.now() >= lingerUntil) {
          killProcessTree(pgid, "SIGKILL");
          done(true);
        } else {
          setTimeout(sweep, 25);
        }
      };
      sweep();
    };

    try {
      child = spawn(command, args, {
        cwd,
        env,
        shell,
        stdio: ["ignore", captureStdout ? "pipe" : "inherit", "inherit"],
        detached: !isWindows,
        windowsHide: true,
      });
    } catch (error) {
      finish({ error });
      return;
    }
    child.stdout?.on("data", (chunk) => {
      chunks.push(chunk);
      onStdout?.(chunk);
    });
    child.stdout?.on("error", () => {});
    child.once("error", (error) => {
      // A failed spawn emits only "error"; a running child may also "close".
      if (!child.pid) finish({ error });
    });
    let exitOutcome = null;
    child.once("exit", (code, signal) => {
      exitOutcome = { code, signal };
      if (!child.stdout) {
        finish(exitOutcome);
        return;
      }
      // Give the pipe a moment to deliver what the root wrote, then stop
      // waiting for descendants that still hold it.
      drainTimer = setTimeout(() => {
        destroyPipes();
        finish(exitOutcome);
      }, drainMs);
    });
    child.once("close", (code, signal) => finish(exitOutcome ?? { code, signal }));

    if (timeoutMs !== undefined && timeoutMs !== null) {
      timer = setTimeout(() => {
        timedOut = true;
        terminate();
      }, Math.max(0, timeoutMs));
    }
    if (abortSignal) {
      if (abortSignal.aborted) onAbort();
      else abortSignal.addEventListener("abort", onAbort, { once: true });
    }
  });
}

/** "1.5s", "4m05s", "1h02m". */
export function formatDuration(ms) {
  if (ms < 60_000) return `${(ms / 1000).toFixed(1)}s`;
  const totalSeconds = Math.round(ms / 1000);
  const hours = Math.floor(totalSeconds / 3600);
  const minutes = Math.floor((totalSeconds % 3600) / 60);
  const seconds = totalSeconds % 60;
  if (hours > 0) return `${hours}h${String(minutes).padStart(2, "0")}m`;
  return `${minutes}m${String(seconds).padStart(2, "0")}s`;
}

/** A budget for humans: "45m", "90s", "1.5s". */
export function formatBudget(ms) {
  if (ms >= 60_000 && ms % 60_000 === 0) return `${ms / 60_000}m`;
  if (ms >= 1000 && ms % 1000 === 0) return `${ms / 1000}s`;
  return `${(ms / 1000).toFixed(1)}s`;
}
