# Recipe: add an event

Events carry backend → frontend notifications (state changes, progress, results). The name
is a string on both sides; only the Rust side has constants. Example: `example-progress`
with payload `{ percent: number }`.

## 1. Name constant: `src-tauri/src/events.rs`

1. Add to `pub mod event`, with a doc comment that states the payload shape and who listens:

   ```rust
   /// Example job progress. Payload: `{ percent: number }`.
   pub const EXAMPLE_PROGRESS: &str = "example-progress";
   ```

2. Add the constant to both arrays in the test module: `event_names_are_kebab_case` and
   `event_names_are_unique`. Names are kebab-case.
3. Do not emit raw string literals. Existing exceptions are debt, not precedent:
   `update-preparing`, `update-aborted`, `navigate_tab` (in `main.rs`) and
   `transcription-progress` (raw in `commands.rs`). `ENGINE_LOAD_STATE` is missing from the
   uniqueness array.
4. Do not put non-state values on `state-changed`; it already carries `settings_reloaded`
   besides dictation states. Give a new concern its own event.

## 2. Emit from Rust

```rust
use tauri::Emitter;

#[derive(Clone, serde::Serialize)]
struct ExampleProgress { percent: u8 }

let _ = app.emit(crate::events::event::EXAMPLE_PROGRESS, ExampleProgress { percent });
```

- `app.emit(...)` broadcasts to every webview (Settings, overlay, onboarding);
  `app.emit_to("settings", ...)` targets one window label (see `prepare_update_recovery`).
- Emitting is thread-safe and needs no main-thread hop; touching tray, overlay or windows does
  (`dispatch_to_main`, [ADR 0003](../adr/0003-main-thread-native-apis.md)).
- Payloads must be `Serialize + Clone`. Keep field names stable; the frontend reads them by
  name. Do not put transcripts into payloads that are logged
  ([ADR 0002](../adr/0002-privacy-first.md)).
- Emit after state is committed (after the controller lock is released), so a listener that
  calls back into a command sees the new state.

## 3. Listen in the frontend

Declare the payload type next to the related wrappers in `src/lib/api.ts` (or the area's
types module) and listen inside `onMount`, cleaning up on unmount. `listen` resolves
asynchronously, so a component can unmount before the unlisten function arrives; handle that.
The pattern in `src/lib/Overlay.svelte`:

```ts
onMount(() => {
  let disposed = false;
  const stops: Array<() => void> = [];
  const remember = (stop: () => void) => (disposed ? stop() : stops.push(stop));
  listen<ExampleProgress>("example-progress", (event) => {
    progress = event.payload.percent;
  }).then(remember);
  return () => { disposed = true; stops.forEach((stop) => stop()); };
});
```

`src/lib/onboarding-download-state.js` shows the same ownership rules as a tested helper.
Several older listeners in `Settings.svelte` drop the unlisten promise; do not copy them.
Keep payload interpretation in a DOM-free `src/lib/*.ts` module (for example the reducer in
`overlay-state.ts`) so it can be unit-tested. Planned:
[#362](https://github.com/Magnus-Gille/sagascript/issues/362) adds a shared
`useTauriEvent` helper; [#347](https://github.com/Magnus-Gille/sagascript/issues/347) adds a
typed event catalogue and contract test.

## 4. Tests

| Layer | What |
|---|---|
| Rust | The `events.rs` tests (name format, uniqueness). Unit-test the code that builds the payload. |
| Frontend logic | `scripts/test-*.mjs` for the module that interprets the payload (see `scripts/test-overlay-state.mjs`). |
| Browser | Add a helper to `window.qa` in `scripts/fixtures/transcription-browser-mock.js` that calls `emit("example-progress", {...})` (like `dictationResult`), and drive it from a `scripts/qa-*.mjs` flow. |

No regex tests over `.svelte` source ([ADR 0004](../adr/0004-test-strategy.md)).

## 5. Verify

```bash
npm run build && cd src-tauri
cargo test events                 # app crate: events.rs tests
cargo clippy --workspace --all-targets -- -D warnings
cd .. && npx svelte-check --tsconfig ./tsconfig.json && npm run test:frontend
npm run test:browser              # if a qa flow covers the event
```

Then the full list in [CONTRIBUTING.md](../../CONTRIBUTING.md#running-tests).
