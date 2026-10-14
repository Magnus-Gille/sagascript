# Recipe: add a Tauri command

A command is a Rust function the frontend calls with `invoke`. Nothing checks at compile
time that the Rust name, the `generate_handler!` entry, the TypeScript wrapper and the
browser mock agree, so each step below is manual. Example: `get_example_info(profile_id)`
returning a small struct.

## 1. Rust command

Put it in `src-tauri/src/commands.rs`, or in the domain file that already owns the area
(`meeting_jobs.rs`, `meeting_review_commands.rs`, `meeting_reprocessing_commands.rs`,
`meeting_media.rs`).

```rust
#[derive(serde::Serialize)]
pub struct ExampleInfo {
    profile_id: String,
    ready: bool,
}

#[tauri::command]
pub async fn get_example_info(
    controller: State<'_, SharedController>,
    profile_id: String,
) -> Result<ExampleInfo, String> {
    let ready = {
        let ctrl = lock_controller(&controller);
        example_ready(ctrl.settings(), &profile_id)
    };
    Ok(ExampleInfo { profile_id, ready })
}
```

- Lock the controller with `lock_controller` (never `lock().unwrap()`, [ADR 0005](../adr/0005-mutex-poison-policy.md)).
- `async fn` runs on the Tokio runtime. A command without `async` runs on the main thread,
  so only use a sync command for trivial work (`complete_update_preparation`).
  `#[tauri::command(async)]` on a plain `fn` moves it off the main thread, but into a task on
  the same Tokio runtime (tauri 2.10 `ipc/mod.rs`, `respond_async_serialized`), not onto the
  blocking pool; long blocking work still belongs in `spawn_blocking`.
- Async commands that borrow (`State<'_, T>`) must return `Result`. Errors are `String` today
  and the UI shows them. Planned: [#360](https://github.com/Magnus-Gille/sagascript/issues/360)
  introduces a typed `CommandError` (ADR 0006).
- Managed state you can ask for: `State<'_, SharedController>`, `State<'_, SharedWhisper>`,
  `State<'_, Arc<UpdateActivity>>`, `State<'_, HotkeyHealth>`, job slots, plus
  `tauri::AppHandle`. See `.manage(...)` calls in `main()`.
- Lock the controller in a short block; a `std::sync::MutexGuard` cannot cross `.await`, and
  blocking work belongs in `tokio::task::spawn_blocking`. Keep the lock order in
  [ARCHITECTURE.md §3](../ARCHITECTURE.md#3-threads-and-locks).
- Native UI, enigo or AppKit calls go through `app.run_on_main_thread(...)`
  ([ADR 0003](../adr/0003-main-thread-native-apis.md)).
- Put decisions in a plain function (`example_ready` above) and unit-test it in
  `commands_tests.rs`, the test file of `commands.rs`; see `apply_language_selection` and
  `language_selection_tests`.
- Settings writes follow [add-setting.md](add-setting.md) step 3.
- App commands need no entry in `src-tauri/capabilities/default.json`; that file only lists
  plugin and core permissions (`build.rs` declares no app manifest).

## 2. Register it

Add `commands::get_example_info,` to `tauri::generate_handler![...]` in `main()` in
`src-tauri/src/main.rs`. A missing entry compiles fine and fails at run time when the
frontend calls it.

## 3. Frontend wrapper and types

In `src/lib/api.ts` (or `src/lib/meeting-reprocessing-api.ts` for that area), next to the
related wrappers:

```ts
export interface ExampleInfo {
  profile_id: string;
  ready: boolean;
}

export async function getExampleInfo(profileId: string): Promise<ExampleInfo> {
  return invoke("get_example_info", { profileId });
}
```

- Argument keys are the Rust parameter names in camelCase (`profile_id` → `profileId`);
  returned field names follow serde (snake_case unless the struct renames them).
- Components import the wrapper; they do not call `invoke` themselves.
- The interface is a hand copy of the Rust struct; keep them in step.

## 4. Browser mock

`scripts/fixtures/transcription-browser-mock.js` answers every `invoke` in the Playwright
suites (`mockIPC` switch). Unknown commands return `null`. Add a `case "get_example_info":`
with realistic data if a view calls the command while loading or a `scripts/qa-*.mjs` flow
depends on it. Calls are recorded in `window.qa.calls` for assertions.

## 5. Tests

| Layer | Where |
|---|---|
| Rust logic | A test module in `commands_tests.rs`, the test file of `commands.rs` (pure function, no Tauri runtime). |
| Frontend logic | Put non-trivial handling in a `src/lib/*.ts` module and test it with Vitest in `src/lib/<module>.test.ts` (see `src/lib/overlay-state.test.ts`). Add the command to `CommandMap` in `src/test/ipc.ts` and its wrapper to `src/lib/api.test.ts`. |
| User flow | A `scripts/qa-*.mjs` Playwright check against the mock, run by `npm run test:browser`. |

Do not test by reading `.svelte` source with regexes ([ADR 0004](../adr/0004-test-strategy.md)).
`src/lib/api.test.ts` pins each wrapper's command name and arguments against the typed mock
([docs/testing/frontend.md](../testing/frontend.md)); planned:
[#347](https://github.com/Magnus-Gille/sagascript/issues/347) extends that contract to
`generate_handler!` and the Rust command signatures.

## 6. CLI equivalent

If the command exposes a capability (not just UI state), the CLI needs it too
([ADR 0001](../adr/0001-cli-first.md)): put the operation in `sagascript-core` and call it from
both the command and a subcommand ([add-cli-subcommand.md](add-cli-subcommand.md)).

## 7. Verify

```bash
npm run build && cd src-tauri
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cd .. && npx svelte-check --tsconfig ./tsconfig.json && npm run test:unit && npm run test:frontend
npm run test:browser        # if a view or qa flow uses the command
```

Then `npm run verify` ([CONTRIBUTING.md](../../CONTRIBUTING.md#running-tests)).
Planned: [#370](https://github.com/Magnus-Gille/sagascript/issues/370) splits `commands.rs` by
domain; put new commands in the matching module once it exists.
