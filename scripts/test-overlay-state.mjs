import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import ts from "typescript";

const source = await readFile(new URL("../src/lib/overlay-state.ts", import.meta.url), "utf8");
const js = ts.transpileModule(source, {
  compilerOptions: { module: ts.ModuleKind.ESNext, target: ts.ScriptTarget.ES2022 },
}).outputText;
const {
  LOADING_LABEL_DELAY_MS,
  initialOverlayState,
  needsLoadingTimer,
  overlayLabel,
  reduceOverlay,
} = await import(`data:text/javascript;base64,${Buffer.from(js).toString("base64")}`);

const run = (actions, start = initialOverlayState) => actions.reduce(reduceOverlay, start);
const state = (value) => ({ type: "state", value });
const engine = (value) => ({ type: "engine", value });
const elapsed = { type: "loading-delay-elapsed" };
const label = (s, language = "sv", locale = "en-US") => overlayLabel(s, language, locale);

test("a cold engine goes recording -> transcribing -> loading label -> transcribing -> done", () => {
  let s = run([state("recording")]);
  assert.equal(label(s), "Recording · Swedish");
  s = run([state("transcribing"), engine("loading")], s);
  // Waiting, but under the threshold: still the normal transcribing label.
  assert.equal(label(s), "Transcribing…");
  assert.equal(needsLoadingTimer(s), true);
  s = run([elapsed], s);
  assert.equal(label(s), "Loading model…");
  s = run([engine("ready")], s);
  assert.equal(s.loadingVisible, false);
  assert.equal(label(s), "Transcribing…");
  s = run([state("idle")], s);
  assert.equal(s.phase, "idle");
});

test("a load that finishes before the threshold never shows the loading label", () => {
  let s = run([state("transcribing"), engine("loading")]);
  assert.equal(needsLoadingTimer(s), true);
  s = run([engine("ready")], s);
  assert.equal(needsLoadingTimer(s), false);
  // A late timer callback is ignored.
  s = run([elapsed], s);
  assert.equal(s.loadingVisible, false);
  assert.equal(label(s), "Transcribing…");
  assert.equal(LOADING_LABEL_DELAY_MS, 150);
});

test("loading while still recording starts no timer and shows no loading label", () => {
  const s = run([state("recording"), engine("loading")]);
  assert.equal(needsLoadingTimer(s), false);
  assert.equal(run([elapsed], s).loadingVisible, false);
  assert.equal(label(s), "Recording · Swedish");
  // Key-up while the load is still running starts the wait now.
  assert.equal(needsLoadingTimer(run([state("transcribing")], s)), true);
});

test("a failed load shows the error state and survives the follow-up idle", () => {
  let s = run([state("transcribing"), engine("loading"), elapsed, engine("failed")]);
  assert.equal(s.phase, "error");
  assert.equal(label(s), "Model failed to load");
  s = run([state("idle")], s);
  assert.equal(s.phase, "error");
  s = run([state("recording")], s);
  assert.equal(s.phase, "recording");
  assert.equal(s.engine, "unknown");
  assert.equal(label(s), "Recording · Swedish");
});

test("a transcription error state sticks until the next recording", () => {
  let s = run([state("transcribing"), engine("ready"), state("error"), state("idle")]);
  assert.equal(s.phase, "error");
  assert.equal(label(s), "Transcription failed");
  assert.equal(run([state("recording")], s).phase, "recording");
});

test("a failed pre-warm while idle does not show an error", () => {
  const s = run([state("idle"), engine("failed")]);
  assert.equal(s.phase, "idle");
});

test("Swedish locale shows the Swedish loading label and ignores unknown states", () => {
  const s = run([state("transcribing"), engine("loading"), elapsed]);
  assert.equal(label(s, "sv", "sv-SE"), "Laddar modell…");
  assert.equal(label(s, "sv", "en-GB"), "Loading model…");
  assert.equal(run([state("bogus")], s), s);
  assert.equal(label(run([state("recording")]), "sv", "sv-SE"), "Spelar in · svenska");
});

test("a cancelled load resets the engine so later dictations do not show loading", () => {
  let s = run([state("transcribing"), engine("loading"), elapsed]);
  assert.equal(label(s), "Loading model…");
  s = run([engine("unknown")], s);
  assert.equal(s.engine, "unknown");
  assert.equal(s.loadingVisible, false);
  assert.equal(label(s), "Transcribing…");
  s = run([state("idle"), state("recording"), state("transcribing")], s);
  assert.equal(label(s), "Transcribing…");
  assert.equal(needsLoadingTimer(s), false);
});

test("a new recording resets stale engine state", () => {
  let s = run([state("transcribing"), engine("loading"), state("idle")]);
  assert.equal(s.engine, "loading");
  s = run([state("recording")], s);
  assert.equal(s.engine, "unknown");
  s = run([state("transcribing")], s);
  assert.equal(label(s), "Transcribing…");
});

test("a background warm failure never escalates to the error state", () => {
  let s = run([state("transcribing"), { ...engine("failed"), source: "warm" }]);
  assert.equal(s.phase, "transcribing");
  assert.equal(s.engine, "unknown");
  s = run([state("idle")], s);
  assert.equal(s.phase, "idle");
  // The same failure from the request itself does escalate.
  s = run([state("transcribing"), { ...engine("failed"), source: "request" }]);
  assert.equal(s.phase, "error");
});

test("seeding from the cached backend state shows loading for an app-start warm", () => {
  // The overlay is created mid-load: initial state is recording, seeded as loading.
  let s = run([state("recording"), { ...engine("loading"), source: "warm" }]);
  assert.equal(s.engine, "loading");
  s = run([state("transcribing"), elapsed], s);
  assert.equal(label(s), "Loading model…");
  // Seeded "unknown" leaves a fresh overlay untouched.
  assert.deepEqual(run([engine("unknown")]), initialOverlayState);
});
