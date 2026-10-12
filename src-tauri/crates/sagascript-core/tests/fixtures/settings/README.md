# Settings file corpus

One fixture per historical shape of the settings file, used by
`tests/settings_fixtures.rs`. Each fixture directory has:

- `input/`: the files as that release left them, laid out like a user's disk.
  `input/data/<identifier>/` is the per-application data directory
  (`~/Library/Application Support` on macOS) that releases before 1.1.0 used;
  `input/config/sagascript/` is the configuration directory used since 1.1.0
  (`~/.config/sagascript`, `%APPDATA%\sagascript` on Windows), with the
  personal dictionaries `glossary.txt` and `glossaries/<profile>.txt`.
- `expected/config/`: the configuration directory after the startup load.
- `expected/loaded.json`: the settings that startup load returns.
- `expected/load_from.json`: the settings `store::load_from` returns for the
  input settings file placed at the current location. This is the plain load
  that field updates use, without machine state.

The test runs the startup load with fixed machine state: Pianissimo supported
and downloaded; Whisper `base`, `base.en`, `kb-whisper-base` and
`kb-whisper-small` downloaded.

## Derivation

Each input was written by hand from the `Settings` struct, its `Default`
impl, `HotkeyProfile` and the store's write path (`store.rs`) at the release
tag. The values are realistic non-default user choices. Keys are sorted and
pretty-printed without a trailing newline, as the store's
`serde_json::to_string_pretty` of a `serde_json::Map` wrote them. Every
release wrote every field of its `Settings` struct, so each input has exactly
the keys its release serialized.

| Fixture | Mirrors tags | Shape |
|---|---|---|
| `pre-1.0` | none (pre-release, `com.sagascript.app`) | Settings at `c4e2eff` (first shared store) plus `hasCompletedOnboarding`, which the Tauri plugin store wrote into the same file |
| `v1.0.0` | v1.0.0 – v1.0.5 | data directory; global `whisper_model`/`auto_select_model`; embedded `initial_prompt` |
| `v1.0.6` | v1.0.6 – v1.0.9 | adds `hotkey_profiles` (`id`, `name`, `shortcut`, `language`) |
| `v1.1.0` | v1.1.0 – v1.1.3 | moves to the configuration directory; dictionaries leave the file for `glossary.txt` and `glossaries/<id>.txt` |
| `v1.2.0` | v1.2.0 | profile `push_to_talk_shortcut`/`toggle_shortcut`; Finnish |
| `v1.3.0` | v1.3.0 – v1.3.2 | adds `file_transcription_model`; empty `glossary.txt`, which every save since 1.1.0 creates |
| `v1.4.0` | v1.4.0 – v1.4.1 | adds per-profile `profile_models`, `pianissimo_dictation`, engine warm-up, `profile_glossary_migrated`, `model_lineup_version`; old engine defaults `on_key_down`/10 |
| `v1.4.2` | v1.4.2 – v1.4.3 | adds `engine_defaults_version`; the current shape |

How each tag was read: `git show vX.Y.Z:src-tauri/crates/sagascript-core/src/settings/manager.rs`
and `.../store.rs`. Between those groups the `Settings` struct, its default
values and the store's write path are unchanged.

## Uncertain

- `pre-1.0` is reconstructed from source, not from a real file. Pre-release
  plugin-store files may have held other keys; only `hasCompletedOnboarding`
  is known to have been read back.
- No release wrote an embedded `profile_glossaries` map: 1.1.0 added the
  field and moved it to files in the same release. Unit tests in `store.rs`
  cover that shape. Likewise the `presenter` hotkey mode existed only in
  development builds between 1.1.3 and 1.2.0; a unit test covers it.
- The release builds may have enabled serde_json's `preserve_order`, which
  would change key order but not content. The test compares JSON values.
