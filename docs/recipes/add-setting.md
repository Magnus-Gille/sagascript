# Recipe: add a setting

A global user setting exists in five places that must agree: the `Settings` struct, the
CLI `config` key, a Tauri setter command, the frontend type/wrapper/UI, and tests. The flow
between them is in [ARCHITECTURE.md §2.2](../ARCHITECTURE.md#22-settings-layers-disk--store--appcontroller--watcher--frontend).
The example adds a boolean `example_flag`; mirror `temperature_fallback` (plain bool),
`engine_prewarm` (enum) or `engine_idle_unload_minutes` (integer), which go through every
step below. Per-profile values (language, model, dictionary, shortcuts) live on
`HotkeyProfile` / `profile_models` and are out of scope here.

## 1. Model: `src-tauri/crates/sagascript-core/src/settings/manager.rs`

1. Add the field with a doc comment to `pub struct Settings`, and its default to
   `impl Default for Settings`.
2. The struct is `#[serde(default)]`, so files without the key load the default: an additive
   field needs no migration. You need one only to
   - change the default for existing users: copy the version-marker pattern
     (`engine_defaults_version`, `ENGINE_DEFAULTS_VERSION`, `Settings::migrate_engine_defaults`,
     `migrate_engine_defaults_at`, called from `store::load()` in `settings/store.rs`);
   - rename a key: `#[serde(alias = "old_name")]` (see `has_completed_onboarding`) or
     `canonicalize_legacy_keys` in `store.rs`;
   - add an enum: `#[serde(rename_all = "snake_case")]` plus `id()` / `parse_id()` like
     `EnginePrewarm`. Never remove or rename a variant that may be on disk: an unknown value
     makes `load_from` fall back to `Settings::default()` for the whole file.

Planned: [#365](https://github.com/Magnus-Gille/sagascript/issues/365) replaces the markers
with `schema_version` and ordered migrations in one module.

## 2. CLI key: `src-tauri/crates/sagascript-cli/src/config.rs`

CLI first ([ADR 0001](../adr/0001-cli-first.md)): every user setting gets a `config` key.

1. Add `"example_flag"` to `VALID_KEYS`.
2. Add arms to `apply_other_setting_value` (set:
   `settings.example_flag = parse_bool(value, "example_flag")?;`; enums use `parse_id` or
   `parse_enum_value`, integers `value.parse::<u32>()` mapped to a `SettingsError`),
   `reset_setting_value` and `get_setting_value` (the string `config get` prints), and a row
   to `cmd_list`. `reset_all_settings` picks up the default through `..defaults`.
3. Add the key and its values to the help text: the `long_about` of `ConfigAction::List`,
   `Get` and `Set` and the `key` doc comments of `Get` and `Set`. A warning (like
   `auto_paste`'s) goes in `setting_warning`.
4. The test `valid_keys_count_matches_settings_struct` fails until the key is in
   `VALID_KEYS`. Add the field to its `INTERNAL_FIELDS` list only if it is not user-facing
   (markers such as `engine_defaults_version`).

## 3. App command: `src-tauri/src/commands.rs` and `src-tauri/src/main.rs`

Copy `set_temperature_fallback`:

```rust
#[tauri::command]
pub async fn set_example_flag(
    controller: State<'_, SharedController>,
    enabled: bool,
) -> Result<(), String> {
    let persisted = sagascript_core::settings::store::update(|settings| {
        settings.example_flag = enabled;
    })?;
    controller.lock().unwrap().settings_mut().example_flag = persisted.example_flag;
    info!("Example flag: {enabled}");
    Ok(())
}
```

- Persist first with `store::update` (`store::try_update` when validation can fail), then copy
  the persisted value into the controller. Never `store::save()` an in-memory clone; never hold
  the controller lock across the store call or an `.await`.
- Side effects go after the lock is released (`set_show_overlay`, `set_vad_enabled`). The
  watcher (`start_settings_watcher` in `main.rs`) copies CLI edits into the controller but does
  not replay setter side effects: prefer reading the value at use time from `ctrl.settings()`,
  or add the side effect to the watcher too.
- Register `commands::set_example_flag` in `tauri::generate_handler![...]` in `main()`.
- Setter logic goes in a plain function with a `#[cfg(test)]` test (`apply_language_selection`
  / `language_selection_tests`, `effective_auto_paste` / `auto_paste_tests`).

## 4. Frontend: `src/lib/api.ts`, `src/lib/Settings.svelte`

1. `api.ts`: add `example_flag: boolean;` to `interface Settings` (enum: a string-literal union
   like `EnginePrewarm`) and a wrapper:
   `export async function setExampleFlag(enabled: boolean): Promise<void> { return invoke("set_example_flag", { enabled }); }`
   (Rust parameter names arrive as camelCase keys).
2. `Settings.svelte`: import the wrapper; add a handler next to `onTemperatureFallbackToggle`
   that calls `applySetting(() => setExampleFlag(next))` (it re-reads `getSettings()` and shows
   errors); copy the `.field-row` toggle markup of "Show recording overlay"
   (`role="switch"`, `aria-checked`, `aria-label`) into the `{#if activeTab === "settings"}`
   block, or into the `Advanced` `<details>` for tuning knobs. Use runes, not stores.
3. `scripts/fixtures/transcription-browser-mock.js`: add the field to the object returned by
   `case "get_settings"`. Setter commands need no case (unknown commands return `null`).

## 5. Tests

| Where | What |
|---|---|
| `settings/manager.rs` | Assert the default in `settings_default_values` and the round trip in `settings_serde_roundtrip`. |
| `settings/store.rs` | Assert a file without the key loads the default (`partial_json_fills_defaults`). Migrations: test through the `*_at(path, …)` helpers on a temp file, never the real settings path. |
| `config.rs` | Extend `get_setting_value_returns_serialized_values`; add a set/reset/reject test modeled on `engine_settings_can_be_set_reset_and_reject_bad_values`. |
| `commands.rs` | Unit-test any extracted setter logic. |
| Frontend | New logic in a `.ts` module gets a `scripts/test-*.mjs` test. Do not add regex tests over `Settings.svelte` source ([ADR 0004](../adr/0004-test-strategy.md)). |

## 6. Verify, including CLI/GUI parity

```bash
npm run build                                   # once; needed by the app crate
cd src-tauri
cargo test -p sagascript-core settings
cargo test -p sagascript-cli config
cargo clippy --workspace --all-targets -- -D warnings
export SAGASCRIPT_SETTINGS_PATH="$(mktemp -d)/sagascript-settings.json"   # never your real file
cargo run -q -p sagascript-cli --no-default-features -- config set example_flag true
cargo run -q -p sagascript-cli --no-default-features -- config get example_flag   # true
cargo run -q -p sagascript-cli --no-default-features -- config reset example_flag
grep example_flag "$SAGASCRIPT_SETTINGS_PATH"
cd .. && npx svelte-check --tsconfig ./tsconfig.json && npm run test:frontend
```

Parity: with the same `SAGASCRIPT_SETTINGS_PATH` exported, run `cargo tauri dev`, toggle the
setting in the UI and check the file holds what `config set` writes; then `config set` while
the app runs and check the UI follows (watcher → `settings_reloaded`). Finish with
[CONTRIBUTING.md](../../CONTRIBUTING.md#running-tests). Planned:
[#373](https://github.com/Magnus-Gille/sagascript/issues/373) (automated CLI/app parity table,
table-driven key registry), [#347](https://github.com/Magnus-Gille/sagascript/issues/347)
(Rust/TypeScript `Settings` contract test).
