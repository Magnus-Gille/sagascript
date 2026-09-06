import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const source = async (path) =>
  readFile(new URL(`../${path}`, import.meta.url), "utf8");

const [
  api,
  settings,
  overlay,
  onboarding,
  transcribe,
  models,
  benchmark,
  benchmarkConfig,
  latency,
  config,
  record,
  cli,
] =
  await Promise.all([
    source("src/lib/api.ts"),
    source("src/lib/Settings.svelte"),
    source("src/lib/Overlay.svelte"),
    source("src/lib/Onboarding.svelte"),
    source("src-tauri/crates/sagascript-cli/src/transcribe.rs"),
    source("src-tauri/crates/sagascript-cli/src/models.rs"),
    source("src-tauri/crates/sagascript-cli/src/benchmark_dictation.rs"),
    source("src-tauri/crates/sagascript-cli/src/benchmark_config.rs"),
    source("src-tauri/crates/sagascript-cli/src/latency.rs"),
    source("src-tauri/crates/sagascript-cli/src/config.rs"),
    source("src-tauri/crates/sagascript-cli/src/record.rs"),
    source("src-tauri/crates/sagascript-cli/src/lib.rs"),
  ]);

test("Finnish is exposed by every production language selector", () => {
  assert.match(api, /export type Language = "en" \| "sv" \| "no" \| "fi" \| "auto";/);
  assert.match(settings, /<option value="fi">Finnish<\/option>/g);
  assert.match(settings, /case "fi": return "Finnish";/);
  assert.match(overlay, /fi: "Finnish"/);
  assert.match(onboarding, /type OnboardingLanguage = "en" \| "sv" \| "no" \| "fi";/);
  assert.match(onboarding, /selectedLanguage === "fi"/);
  assert.match(onboarding, /fi: "142 MB"/);
});

test("CLI Finnish language and model IDs use the core model registry", () => {
  assert.match(transcribe, /"fi" \| "finnish" => Ok\(Language::Finnish\)/);
  assert.match(transcribe, /"base" => Ok\(WhisperModel::Base\)/);
  assert.doesNotMatch(transcribe, /fi-whisper-medium|FinnishWhisperMedium/);
  assert.match(models, /Language::Finnish/);
  assert.match(benchmark, /"en" \| "sv" \| "no" \| "fi"/);
  assert.match(benchmark, /Language::Finnish => "fi"/);
  assert.match(benchmarkConfig, /Language::Finnish/);
  assert.match(benchmarkConfig, /en, sv, no, or fi/);
  assert.match(latency, /\(Language::Finnish, "fi"\)/);
  assert.match(config, /en, sv, no, fi, auto/);
  assert.match(record, /en, sv, no, fi, auto/);
});

test("Finnish remains opt-in and does not alter generic defaults", () => {
  assert.match(cli, /Supported languages: English \(en\), Swedish \(sv\), Norwegian \(no\), Finnish \(fi\), Auto-detect/);
  assert.match(cli, /Finnish uses the generic multilingual Base model/);
  assert.doesNotMatch(cli, /fi-whisper-medium|Finnish-NLP model/);
  assert.match(onboarding, /let selectedLanguage: OnboardingLanguage = \$state\("en"\)/);
  assert.match(onboarding, /fi: "base"/);
  assert.match(transcribe, /"auto" => Ok\(Language::Auto\)/);
});
