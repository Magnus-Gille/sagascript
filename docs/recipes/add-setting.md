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
   - change the default for existing users: append a step to `settings/migrations.rs`, guarded
     by a version marker of its own like step 6 (`engine_defaults_version`,
     `ENGINE_DEFAULTS_VERSION`, `Settings::migrate_engine_defaults`); see "Adding a migration"
     in [ADR 0007](../adr/0007-settings-file-evolution.md);
   - rename a key: `#[serde(alias = "old_name")]` (see `has_completed_onboarding`) or a
     migration step;
   - add an enum: `#[serde(rename_all = "snake_case")]` plus `id()` / `parse_id()` like
     `EnginePrewarm`. Never remove or rename a variant that may be on disk: an unknown value
     resets that field to its default, and the next save replaces it (after saving a `.bak`
     copy of the file).

## 2. CLI key: `src-tauri/crates/sagascript-cli/src/config/keys.rs`

CLI first ([ADR 0001](../adr/0001-cli-first.md)): every user setting gets a `config` key, and a
key is one entry in the `KEYS` table (#373): `VALID_KEYS`, `config list`, the key lists and the
value table of the `--help` texts and the `[possible values]` of the key argument are generated
from it.

1. Add a `Key { name, note, help, get, set, reset, warning }` to `KEYS`, in the order `config list`
   should print it. A `bool` or `u32` field with nothing to apply beside it is one line:
   `bool_key!("example_flag", example_flag, &["true, false"])` (`count_key!` for an integer). Any
   other key spells out `get` (the string `config get` prints), `set` (parse the text, then apply the
   operation; a language is parsed by `parse_value::<Language>`, a model by
   `parse_value::<WhisperModel>`, an enum by its `from_id` / `parse_id`, a bool by `parse_bool`),
   `reset` (back to the value in `defaults`) and a `warning` if `config set` should print one (like
   `auto_paste`'s). `help` is the line of `config set --help`: the accepted values.
2. If the setting is more than a field (it moves other fields, or the app must do the same), the rule
   is a `Settings` method in `sagascript-core/src/settings/ops.rs` and the key calls it: see the next
   section. `reset_all_settings` picks up a plain field's default through `..defaults`.
3. The test `valid_keys_count_matches_settings_struct` fails until the key is in `KEYS`. Add the
   field to its `INTERNAL_FIELDS` list only if it is not user-facing (markers such as
   `engine_defaults_version`).

## 3. App command: `src-tauri/src/commands/settings.rs` and `src-tauri/src/main.rs`

Copy `set_temperature_fallback` (`commands/settings.rs`):

```rust
#[tauri::command]
pub async fn set_example_flag(
    controller: State<'_, SharedController>,
    enabled: bool,
) -> Result<(), String> {
    persist_and_sync(&controller, |settings| {
        settings.example_flag = enabled;
    })
    .map_err(|error| error.to_string())?;
    info!("Example flag: {enabled}");
    Ok(())
}
```

`persist_and_sync` (`commands/persist.rs`) is the only way a command writes settings: it
applies the change to the latest file under the settings lock and then the same change to
the controller's copy, one writer at a time, so a setter cannot forget the second half. It
returns the written `Settings`. A new setter uses this form: **the controller takes over only
the field you set**. That matters: after a hot reload that could not register the file's
shortcuts, the controller deliberately stays on the registered ones, and a setter that
replaced the whole copy would install shortcuts the OS never registered.

- `try_persist_and_mirror(ctrl, change, |memory, written| …)` when the change can be refused
  (validation runs under the settings lock; a typed `CommandError` stays typed) and the
  controller should copy the written fields (`apply_training_glossary`).
  `try_persist_and_replace` replaces the controller's whole copy; only the setters that
  applied the settings they just wrote use it (`set_language`, `set_profile_model`,
  `set_profile_glossary`, `persist_hotkey_change`), and it also returns the copy it replaced.
  Never `store::save()` an in-memory clone, never write `ctrl.settings_mut()` (it exists in
  tests only), never hold the controller lock across the store call or an `.await`. The
  setter's error type stays `String` until
  [#360](https://github.com/Magnus-Gille/sagascript/issues/360) part 2: map with
  `.map_err(|error| error.to_string())`.
- Add a row to the table in `commands/persist_tests.rs` and say how the setter syncs
  (`Mirror::Field` with its fields, or `Mirror::Whole`): the test calls every setter, changes
  the file behind the controller, and checks that only those fields followed. It also fails
  for a command that writes settings and has no row. The writers are found from the sources
  (`src-tauri/src/contract/writers.rs`), not from the name: a `#[tauri::command]` that reaches
  `persist_and_sync` or a sibling counts, through any chain of functions, methods and other
  commands in any file under `src/` (tests excepted), through a `use … as` rename, and from
  inside a macro such as `dbg!`. What it cannot see: functions and methods are matched by
  name across files (a same-named function can ask for a row you do not need), glob imports
  (`use x::*`), `macro_rules!`-generated code, a turbofish call inside a macro, and calls
  through a function pointer or closure kept in a variable. A second test fails when a
  function other than the permission gate (`load_settings_with_permission_gate` in
  `launch/settings.rs`) calls `store::update`, `try_update`,
  `update_with`, `try_update_with`, `save` or `machine::{update, try_update}` itself.
- A setting that is more than a field is a `Settings` operation in `settings/ops.rs` (take the
  Pianissimo gate or the Accessibility answer as an argument, never probe the machine), the setter
  calls it inside `persist_and_sync` / `try_persist_and_replace` and so does the CLI key (#373).
  Add a row for it to `core::test_support::settings_ops_table` and a translation to the CLI and to the
  app side of the table (`sagascript-cli/tests/settings_parity.rs`, `commands/persist_parity_tests.rs`):
  the table fails when a surface persists something else than the operation does. Decide a
  difference on purpose and write it in `Case::differs_in` and in
  [ARCHITECTURE.md §2.2](../ARCHITECTURE.md#22-settings-layers-disk--store--appcontroller--watcher--frontend).
- Side effects go after the persist call (`set_show_overlay`, `set_vad_enabled`). A setter that
  needs the `AppHandle` (the profiles menu, the overlay window) splits its persistence half into
  a plain function, as `persist_language` and `persist_show_overlay` do, so the table test can
  call it. The
  watcher (`start_settings_watcher` in `settings_watcher.rs`) reloads the **whole** `Settings` from disk
  when the CLI edits the file, so a plain value read at use time from `ctrl.settings()` needs
  nothing there. It does not replay setter side effects: if your setter has one (recreate a
  window, download a model), add it to the watcher too.
- Register `commands::set_example_flag` in `tauri::generate_handler![...]` in `build_app()` (`main.rs`), then
  regenerate the shared contract: `SAGASCRIPT_BLESS_CONTRACTS=1 cargo test -p sagascript
  contract` in `src-tauri/` adds the command to the generated `CommandMap` and the new field
  to `contracts/transport/get_settings.json` ([contracts/README.md](../../contracts/README.md)).
  Set a non-default value in `customised()` in `commands/settings_tests.rs` so the fixture
  shows it.
- Setter logic goes in a plain function with a test in `commands/settings_tests.rs`
  (`apply_language_selection` / `language_selection_tests`, `auto_paste_tests`; the rule itself is
  tested in `settings/ops_tests.rs`).

## 4. Frontend: `src/lib/api.ts`, `src/lib/Settings.svelte`

1. `api.ts`: add `example_flag: boolean;` to `interface Settings` (enum: a string-literal union
   like `EnginePrewarm`) and the same field to `settings` in `src/test/transport-schemas.ts`
   (svelte-check fails while they differ; Vitest fails while the fixture has a field the
   type lacks), and a wrapper:
   `export async function setExampleFlag(enabled: boolean): Promise<void> { return invoke("set_example_flag", { enabled }); }`
   (Rust parameter names arrive as camelCase keys).
2. `Settings.svelte`: import the wrapper; add a handler next to `onTemperatureFallbackToggle`
   that calls `applyScalarSetting(() => setExampleFlag(next))`. It writes, re-reads the
   settings and shows errors, and does not query the models again. Use plain `applySetting`
   only when the value feeds `dictation_model_for_profile` or the file-model resolution
   (language, the profile list, a profile's or the file model): it also re-queries every
   profile's models. Both resolve `true` once the change is saved, even if the re-read
   failed, which the window reports separately; for a native `<select>` or `<input>` put the
   saved value back when it resolves `false`, as `onBeamSizeChange` does. Add the new control
   to the `changes` table in `src/lib/Settings.saving.test.ts`, which pins one write and one
   read-back with no model queries. Copy the `.field-row` toggle markup of "Show recording overlay"
   (`role="switch"`, `aria-checked`, `aria-label`, and the `hotkey-hint` line under it). Put it
   next to related settings: dictation and paste behaviour lives in the Dictate tab (next to
   the auto-paste toggle), general settings in `{#if activeTab === "settings"}`, tuning knobs
   in the `Advanced` `<details>`. Use runes, not stores.
3. `scripts/fixtures/transcription-browser-mock.js`: add the field to the object returned by
   `case "get_settings"`. Setter commands need no case (unknown commands return `null`).

## 5. Tests

| Where | What |
|---|---|
| `settings/manager_tests.rs` | Assert the default in `settings_default_values` and the round trip in `settings_serde_roundtrip`. |
| `settings/store_tests.rs` | Assert a file without the key loads the default (`partial_json_fills_defaults`). Migrations: test through the `*_at(path, …)` helpers on a `fake_settings()` file (`with_temp_settings`), never the real settings path. |
| `config.rs` | Extend `get_setting_value_returns_serialized_values`; add a set/reset/reject test modeled on `engine_settings_can_be_set_reset_and_reject_bad_values`. |
| `settings/ops_tests.rs`, `test_support/settings_ops_table.rs` | The rule of an operation, and its row in the CLI/app parity table (run by `tests/settings_parity.rs` and `commands/persist_parity_tests.rs`). |
| `commands/settings_tests.rs`, `commands/persist_tests.rs` | Unit-test any extracted setter logic; add the setter's row to the `persist_and_sync` table. |
| Frontend | New logic in a `.ts` module gets a Vitest test in `src/lib/<module>.test.ts`; what Settings shows or invokes gets a component test (`src/lib/Settings.test.ts`, [docs/testing/frontend.md](../testing/frontend.md)). Do not add regex tests over `Settings.svelte` source ([ADR 0004](../adr/0004-test-strategy.md)). |

## 6. Verify, including CLI/GUI parity

Run this as one shell session (the exported variable must reach every command):

```bash
npm ci && npm run build                         # fresh worktree: deps, then dist/ for the app crate
cd src-tauri
cargo fmt --all                                 # CI rejects unformatted code
cargo test -p sagascript-core settings
cargo test -p sagascript-cli config
cargo test -p sagascript-cli config -- --list | grep example_flag   # your new test really ran
cargo clippy --workspace --all-targets -- -D warnings
cargo clippy -p sagascript-cli --all-targets --no-default-features -- -D warnings
cargo test -p sagascript-cli --no-default-features
export SAGASCRIPT_SETTINGS_PATH="$(mktemp -d)/sagascript-settings.json"   # never your real file
cli() { cargo run -q -p sagascript-cli --no-default-features -- "$@"; }
cli config get example_flag                     # false (the default)
cli config set example_flag true && cli config get example_flag   # true
cli config set example_flag maybe; echo "exit $?"                 # rejected: exit 1
cli config list | grep example_flag             # the row `config list` prints from the table
cli config set --help | grep example_flag       # the help text
cli config reset example_flag && grep example_flag "$SAGASCRIPT_SETTINGS_PATH"  # key stays, now false
cd ..
grep -n set_example_flag src-tauri/src/commands/settings.rs src-tauri/src/main.rs src/lib/api.ts  # all three
npx svelte-check --tsconfig ./tsconfig.json && npm run test:unit && npm run test:frontend
npx playwright install chromium && npm run test:browser   # you touched Settings.svelte and the mock
```

The app crate's tests fail on a `#[tauri::command]` missing from `generate_handler!`, and
`src/lib/api.test.ts` type-checks the wrapper's command name and argument keys against the
generated `CommandMap` (add its row there); the `grep` is a quick look at all three.

Parity: with the same `SAGASCRIPT_SETTINGS_PATH` exported, run `cargo tauri dev`, toggle the
setting in the UI and check the file holds what `config set` writes; then `config set` while
the app runs and check the UI follows (watcher → `settings_reloaded`). Without a desktop
session (a headless agent), say so in the PR and leave this step to a reviewer with a Mac.
Finish with `npm run verify` ([CONTRIBUTING.md](../../CONTRIBUTING.md#running-tests)). The
automated part of the parity check, for every setting that has an operation or a row in the table,
is `cargo test -p sagascript-cli --test settings_parity` and `cargo test -p sagascript parity`
([#373](https://github.com/Magnus-Gille/sagascript/issues/373)).
