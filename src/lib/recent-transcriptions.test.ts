import { expect, test } from "vitest";
import { rememberTranscription, type RecentTranscription } from "./recent-transcriptions";

const run = (path: string, profileId: string | null = null): RecentTranscription => ({
  path,
  profileId,
  prompt: "",
  diarize: false,
});

test("five recent files retain their own options, deduplicate and move re-runs to the front", () => {
  let history: RecentTranscription[] = [];
  for (let i = 0; i < 6; i++) history = rememberTranscription(history, run(`/audio/${i}.wav`, `p${i}`));
  expect(history.map((r) => r.path)).toStrictEqual([5, 4, 3, 2, 1].map((i) => `/audio/${i}.wav`));
  const old = history;
  history = rememberTranscription(history, { ...run("/audio/2.wav"), prompt: "Names", diarize: true });
  expect(history.length).toBe(5);
  expect(history[0].profileId).toBe(null);
  expect(history[0].prompt).toBe("Names");
  expect(history[0].diarize).toBe(true);
  expect(old[0].path).toBe("/audio/5.wav");
  expect(history[1].profileId).toBe("p5");
});

test("same basenames in different directories remain distinct, paths are not trimmed", () => {
  let history = rememberTranscription([], run("/one/talk.wav"));
  history = rememberTranscription(history, run("/two/talk.wav"));
  history = rememberTranscription(history, run("/two/talk.wav "));
  expect(history.length).toBe(3);
  expect(history[0].path).toBe("/two/talk.wav ");
});
