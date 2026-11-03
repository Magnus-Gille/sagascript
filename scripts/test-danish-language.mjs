import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";

const source = async (path) => readFile(new URL(`../${path}`, import.meta.url), "utf8");

const [coreLanguage, coreWhisper, corePostprocess, models, config, keys, record, latency, cli, transcribeArgs] = await Promise.all([
  source("src-tauri/crates/sagascript-core/src/models/language.rs"),
  source("src-tauri/crates/sagascript-core/src/models/whisper.rs"),
  source("src-tauri/crates/sagascript-core/src/transcription/postprocess.rs"),
  source("src-tauri/crates/sagascript-cli/src/models.rs"),
  source("src-tauri/crates/sagascript-cli/src/config.rs"),
  source("src-tauri/crates/sagascript-cli/src/config/keys.rs"),
  source("src-tauri/crates/sagascript-cli/src/record.rs"),
  source("src-tauri/crates/sagascript-cli/src/latency.rs"),
  source("src-tauri/crates/sagascript-cli/src/lib.rs"),
  source("src-tauri/crates/sagascript-cli/src/transcribe/args.rs"),
]);

test("Danish is a core language with the id, name and Whisper code da", () => {
  assert.match(coreLanguage, /#\[serde\(rename = "da"\)\]\s+Danish,/);
  assert.match(coreLanguage, /Language::Danish => "da"/);
  assert.match(coreLanguage, /Language::Danish => "Danish"/);
  assert.match(coreLanguage, /Language::Danish => Some\("da"\)/);
  assert.match(coreLanguage, /"danish" => Ok\(Language::Danish\)/);
  assert.match(coreLanguage, /pub const ALL: \[Language; 6\]/);
  // Auto-detect stays last so every list keeps its established ending.
  assert.match(coreLanguage, /Language::Danish,\s+Language::Auto,\s+\];/);
});

test("Danish uses only the multilingual Whisper models and recommends Small", () => {
  assert.match(coreWhisper, /Language::Danish => WhisperModel::Small/);
  assert.match(coreWhisper, /Language::Danish \| Language::Auto => \{\s+!self\.is_english_only\(\) && !self\.is_language_optimized\(\)/);
  assert.match(coreWhisper, /Language::Danish \| Language::Auto => &\[\s+WhisperModel::Tiny,/);
  // No Danish-specific fine-tune is registered.
  assert.doesNotMatch(coreWhisper, /DanishWhisper|DaWhisper|da-whisper/i);
  assert.match(corePostprocess, /Language::Swedish \| Language::Danish => "\[MUSIK\]"/);
});

test("CLI help, config and listings name Danish", () => {
  assert.match(transcribeArgs, /en, sv, no, fi, da, auto/);
  assert.match(record, /en, sv, no, fi, da, auto/);
  assert.match(config, /en, sv, no, fi, da, auto/);
  assert.match(keys, /en, sv, no, fi, da, auto/);
  assert.match(models, /en, sv, no, fi, da, auto/);
  assert.match(models, /Language::Danish/);
  assert.match(latency, /\(Language::Danish, "da"\)/);
  assert.match(
    cli,
    /Supported languages: English \(en\), Swedish \(sv\), Norwegian \(no\), Finnish \(fi\), Danish \(da\), Auto-detect \(auto\)\./,
  );
  assert.match(cli, /Danish uses the generic multilingual Small model/);
  assert.match(cli, /sagascript list-models --language da/);
});
