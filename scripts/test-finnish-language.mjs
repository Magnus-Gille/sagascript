import assert from "node:assert/strict";
import { readdir, readFile } from "node:fs/promises";
import test from "node:test";

const source = async (path) => readFile(new URL(`../${path}`, import.meta.url), "utf8");

// The CLI's transcribe command is a directory of modules; the negative checks below
// must hold over all of it, wherever the code lives, so they read every file.
const transcribeSources = async () => {
  const dir = "src-tauri/crates/sagascript-cli/src/transcribe";
  const names = (await readdir(new URL(`../${dir}`, import.meta.url))).filter((name) => name.endsWith(".rs")).sort();
  const texts = await Promise.all(names.map((name) => source(`${dir}/${name}`)));
  return texts.map((text) => text.replace(/\r\n/g, "\n")).join("\n");
};

const [transcribe, models, benchmark, benchmarkConfig, latency, config, record, cli] = await Promise.all([
  transcribeSources(),
  source("src-tauri/crates/sagascript-cli/src/models.rs"),
  source("src-tauri/crates/sagascript-cli/src/benchmark_dictation.rs"),
  source("src-tauri/crates/sagascript-cli/src/benchmark_config.rs"),
  source("src-tauri/crates/sagascript-cli/src/latency.rs"),
  // The key table: the language values are listed in its help line (#373).
  source("src-tauri/crates/sagascript-cli/src/config/keys.rs"),
  source("src-tauri/crates/sagascript-cli/src/record.rs"),
  source("src-tauri/crates/sagascript-cli/src/lib.rs"),
]);

// The CLI parses languages and models with the core catalog (#373).
const [coreLanguage, coreWhisper] = await Promise.all([
  source("src-tauri/crates/sagascript-core/src/models/language.rs"),
  source("src-tauri/crates/sagascript-core/src/models/whisper.rs"),
]);

test("CLI Finnish language and model IDs use the core model registry", () => {
  assert.match(coreLanguage, /"finnish" => Ok\(Language::Finnish\)/);
  assert.match(coreWhisper, /WhisperModel::Base => "base"/);
  assert.match(transcribe, /s\.parse::<Language>\(\)/);
  assert.match(transcribe, /WhisperModel::from_id\(s\)/);
  assert.doesNotMatch(transcribe, /"fi-whisper-medium"\s*=>\s*Ok\(WhisperModel::/);
  assert.doesNotMatch(transcribe, /WhisperModel::FinnishWhisperMedium/);
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
  assert.match(coreLanguage, /Language::Auto => "auto"/);
});
