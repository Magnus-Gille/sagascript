import { expect, test } from "vitest";
import {
  LOADING_LABEL_DELAY_MS,
  initialOverlayState,
  needsLoadingTimer,
  overlayLabel,
  reduceOverlay,
  type OverlayAction,
  type OverlayState,
} from "./overlay-state";

const run = (actions: OverlayAction[], start: OverlayState = initialOverlayState) =>
  actions.reduce(reduceOverlay, start);
const state = (value: string): OverlayAction => ({ type: "state", value });
const engine = (value: string) => ({ type: "engine", value }) as const;
const elapsed: OverlayAction = { type: "loading-delay-elapsed" };
const label = (s: OverlayState, language = "sv", locale = "en-US") => overlayLabel(s, language, locale);

test("a cold engine goes recording -> transcribing -> loading label -> transcribing -> done", () => {
  let s = run([state("recording")]);
  expect(label(s)).toBe("Recording · Swedish");
  s = run([state("transcribing"), engine("loading")], s);
  // Waiting, but under the threshold: still the normal transcribing label.
  expect(label(s)).toBe("Transcribing…");
  expect(needsLoadingTimer(s)).toBe(true);
  s = run([elapsed], s);
  expect(label(s)).toBe("Loading model…");
  s = run([engine("ready")], s);
  expect(s.loadingVisible).toBe(false);
  expect(label(s)).toBe("Transcribing…");
  s = run([state("idle")], s);
  expect(s.phase).toBe("idle");
});

test("a load that finishes before the threshold never shows the loading label", () => {
  let s = run([state("transcribing"), engine("loading")]);
  expect(needsLoadingTimer(s)).toBe(true);
  s = run([engine("ready")], s);
  expect(needsLoadingTimer(s)).toBe(false);
  // A late timer callback is ignored.
  s = run([elapsed], s);
  expect(s.loadingVisible).toBe(false);
  expect(label(s)).toBe("Transcribing…");
  expect(LOADING_LABEL_DELAY_MS).toBe(150);
});

test("loading while still recording starts no timer and shows no loading label", () => {
  const s = run([state("recording"), engine("loading")]);
  expect(needsLoadingTimer(s)).toBe(false);
  expect(run([elapsed], s).loadingVisible).toBe(false);
  expect(label(s)).toBe("Recording · Swedish");
  // Key-up while the load is still running starts the wait now.
  expect(needsLoadingTimer(run([state("transcribing")], s))).toBe(true);
});

test("a failed load shows the error state and survives the follow-up idle", () => {
  let s = run([state("transcribing"), engine("loading"), elapsed, engine("failed")]);
  expect(s.phase).toBe("error");
  expect(label(s)).toBe("Model failed to load");
  s = run([state("idle")], s);
  expect(s.phase).toBe("error");
  s = run([state("recording")], s);
  expect(s.phase).toBe("recording");
  expect(s.engine).toBe("unknown");
  expect(label(s)).toBe("Recording · Swedish");
});

test("a transcription error state sticks until the next recording", () => {
  const s = run([state("transcribing"), engine("ready"), state("error"), state("idle")]);
  expect(s.phase).toBe("error");
  expect(label(s)).toBe("Transcription failed");
  expect(run([state("recording")], s).phase).toBe("recording");
});

test("a failed pre-warm while idle does not show an error", () => {
  const s = run([state("idle"), engine("failed")]);
  expect(s.phase).toBe("idle");
});

test("Swedish locale shows the Swedish loading label and ignores unknown states", () => {
  const s = run([state("transcribing"), engine("loading"), elapsed]);
  expect(label(s, "sv", "sv-SE")).toBe("Laddar modell…");
  expect(label(s, "sv", "en-GB")).toBe("Loading model…");
  expect(run([state("bogus")], s)).toBe(s);
  expect(label(run([state("recording")]), "sv", "sv-SE")).toBe("Spelar in · svenska");
});

test("a cancelled load resets the engine so later dictations do not show loading", () => {
  let s = run([state("transcribing"), engine("loading"), elapsed]);
  expect(label(s)).toBe("Loading model…");
  s = run([engine("unknown")], s);
  expect(s.engine).toBe("unknown");
  expect(s.loadingVisible).toBe(false);
  expect(label(s)).toBe("Transcribing…");
  s = run([state("idle"), state("recording"), state("transcribing")], s);
  expect(label(s)).toBe("Transcribing…");
  expect(needsLoadingTimer(s)).toBe(false);
});

test("a new recording resets stale engine state", () => {
  let s = run([state("transcribing"), engine("loading"), state("idle")]);
  expect(s.engine).toBe("loading");
  s = run([state("recording")], s);
  expect(s.engine).toBe("unknown");
  s = run([state("transcribing")], s);
  expect(label(s)).toBe("Transcribing…");
});

test("a background warm failure never escalates to the error state", () => {
  let s = run([state("transcribing"), { ...engine("failed"), source: "warm" }]);
  expect(s.phase).toBe("transcribing");
  expect(s.engine).toBe("unknown");
  s = run([state("idle")], s);
  expect(s.phase).toBe("idle");
  // The same failure from the request itself does escalate.
  s = run([state("transcribing"), { ...engine("failed"), source: "request" }]);
  expect(s.phase).toBe("error");
});

test("seeding from the cached backend state shows loading for an app-start warm", () => {
  // The overlay is created mid-load: initial state is recording, seeded as loading.
  let s = run([state("recording"), { ...engine("loading"), source: "warm" }]);
  expect(s.engine).toBe("loading");
  s = run([state("transcribing"), elapsed], s);
  expect(label(s)).toBe("Loading model…");
  // Seeded "unknown" leaves a fresh overlay untouched.
  expect(run([engine("unknown")])).toStrictEqual(initialOverlayState);
});
