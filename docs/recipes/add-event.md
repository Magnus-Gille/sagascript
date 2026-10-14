# Recipe: add an event

Events carry backend → frontend notifications (state changes, progress, results). Both
sides declare each event once: the Rust catalogue in `src-tauri/src/events.rs` (name, payload
type, producer, consumer) and `EventMap` in `src/lib/events.ts`; the shared fixture
`contracts/transport/events.json` ties them together ([contracts/README.md](../../contracts/README.md)).
Example: `example-progress` with payload `{ percent: number }`.

## 1. Catalogue entry: `src-tauri/src/events.rs`

1. Declare the payload type next to the others (`#[derive(Clone, Serialize)]`; borrow with a
   lifetime when the emit site has the data borrowed, like `PlainTranscriptionProgress<'a>`).
2. Add an entry to the `catalogue!` block in `pub mod event`, with a doc comment that names the
   producer and the consumer:

   ```rust
   /// Example job progress. Producer: example.rs. Consumer: Example.svelte.
   EXAMPLE_PROGRESS: ExampleProgressEvent = "example-progress",
       payload<'a> = ExampleProgress,
       consumer = Consumer::Gui,
       scope = None;
   ```

   `scope = Some("runId")` names the payload field that ties an event to one job or run.
   `Consumer::Internal("reason")` marks an event nothing in the frontend listens for (a named
   legacy exception); it needs no listener. Names are kebab-case; the uniqueness and
   kebab-case tests cover every entry by construction.
3. Add sample payloads to `all_samples()` in `src-tauri/src/events_tests.rs`, built with the
   same types and builders as the emit site; a sample of the wrong type does not compile.
4. Do not put non-state values on `state-changed`; it already carries `settings_reloaded`
   besides dictation states. Give a new concern its own event.

## 2. Emit from Rust

```rust
use crate::events::{event, EmitEvent, ExampleProgress};

let _ = app.emit_event(event::EXAMPLE_PROGRESS, ExampleProgress { percent });
```

- `emit_event` broadcasts to every webview (Settings, overlay, onboarding);
  `emit_event_to("settings", …)` targets one window label (see `prepare_update_recovery`).
  The compiler checks the payload type. `tauri::Emitter::emit*` is a clippy error
  (`disallowed-methods` in `src-tauri/clippy.toml`), so a raw name cannot slip in.
- Emitting is thread-safe and needs no main-thread hop; touching tray, overlay or windows does
  (`dispatch_to_main`, [ADR 0003](../adr/0003-main-thread-native-apis.md)).
- Keep field names stable; the frontend reads them by name. Do not put transcripts into
  payloads that are logged ([ADR 0002](../adr/0002-privacy-first.md)).
- Emit after state is committed (after the controller lock is released), so a listener that
  calls back into a command sees the new state.
- Regenerate the shared fixture and review it:
  `cd src-tauri && SAGASCRIPT_BLESS_CONTRACTS=1 cargo test -p sagascript contract`.

## 3. Listen in the frontend

Add the event to `EventMap` in `src/lib/events.ts` (payload type next to it) and its schema
to `eventPayloads` in `src/test/transport-schemas.ts`; `src/test/transport.test.ts` then
parses the Rust samples with it. Listen through `useTauriEvent` from
`src/lib/tauri-events.svelte.ts`, typed by `EventMap`; it owns the unlisten, so a component
that unmounts before the registration resolved leaves no listener behind (ESLint forbids a
raw `listen()` in `src/`). Call it while the component initializes, not inside `onMount`:

```ts
import { useTauriEvent } from "./tauri-events.svelte";

useTauriEvent("example-progress", (payload) => {
  progress = payload.percent;
});
```

It returns a subscription whose `ready` promise settles once the listener is active; use it
to order work that must not start before the listener. A listener with its own lifetime (one
run, one request) uses `subscribeTauriEvent(name, handler)` and calls `dispose()` when done;
`src/lib/FileTranscription.svelte` is the example. Keep payload interpretation in a DOM-free
`src/lib/*.ts` module (for example the reducer in `overlay-state.ts`) so it can be unit-tested.

## 4. Tests

| Layer | What |
|---|---|
| Rust | `events_tests.rs`: name format, uniqueness, and `transport_contract_events` against `contracts/transport/events.json`. Unit-test the code that builds the payload. |
| Contract | `src/test/transport.test.ts` parses every sample with the `EventMap` schema; feed the samples to the module that interprets them there. |
| Frontend logic | `src/lib/<module>.test.ts` (Vitest) for the module that interprets the payload (see `src/lib/overlay-state.test.ts`); `ipc.emit()` is typed by `EventMap`. |
| Browser | Add a helper to `window.qa` in `scripts/fixtures/transcription-browser-mock.js` that calls `emit("example-progress", {...})` (like `dictationResult`), and drive it from a `scripts/qa-*.mjs` flow. Emits reach `listen` only because the mock installs `mockIPC(..., { shouldMockEvents: true })`; keep that option. |

No regex tests over `.svelte` source ([ADR 0004](../adr/0004-test-strategy.md)).

## 5. Verify

```bash
npm run build && cd src-tauri
cargo test -- events contract     # app crate: catalogue tests and the shared fixture
cargo clippy --workspace --all-targets -- -D warnings
cd .. && npx svelte-check --tsconfig ./tsconfig.json && npm run test:unit && npm run test:frontend
npm run test:browser              # if a qa flow covers the event
```

Then `npm run verify` ([CONTRIBUTING.md](../../CONTRIBUTING.md#running-tests)).
