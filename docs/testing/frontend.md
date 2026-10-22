# Frontend tests

How the Svelte/TypeScript frontend is tested ([ADR 0004](../adr/0004-test-strategy.md),
[#361](https://github.com/Magnus-Gille/sagascript/issues/361)). Status: Vitest with unit and
component tests (happy-dom, `@testing-library/svelte`), the typed IPC mock, Prettier and
ESLint are in place. The Settings pilot and component migration replaced the private
source harnesses (mappings below); the updater-owned recovery source test remains. happy-dom and ESLint bring BSD-2-Clause and BlueOak-1.0.0 packages, which
the owner accepted for dev tooling only
([decision](https://github.com/Magnus-Gille/sagascript/issues/361#issuecomment-6153321272)):
`scripts/lib/npm-license-policy.mjs` accepts them only for `"dev": true` packages outside the
closure of what the app bundle can contain: runtime dependencies, every package `src/` imports
or references as an asset (`new URL(…, import.meta.url)`, CSS `url()` and `@import`), their
dependencies, and Vite's injected helpers. `"dev"` alone is not enough: Vite bundles whatever
`src/` references, and svelte is a dev dependency. Reaching such a package from app code fails
`scripts/test-npm-license-policy.mjs` (every PR; it also checks the closure against everything
a real `vite build` bundles, emits or reads) and `npm run licenses:check` (release).

| Layer | Where | Command |
|---|---|---|
| Unit and component (Vitest) | `src/**/*.test.ts`, next to the module or component | `npm run test:unit` (`npm run test:unit:coverage`: report in `coverage/`, no gate) |
| Lint (ESLint) | `src/`, `scripts/`, `tests/`; `eslint.config.js` | `npm run lint` |
| Browser flows (Playwright) | `scripts/qa-*.mjs` against `scripts/fixtures/transcription-browser-mock.js` | `npm run test:browser` |
| Legacy source-text tests | `scripts/test-file-transcription-recovery.mjs` (updater owner); native assertions in Finnish/release scripts | `npm run test:ui`, `npm run test:release`; add no component source assertions |

## Where tests live

- `src/lib/<module>.test.ts` tests `src/lib/<module>.ts` (or `.svelte.ts`); component tests
  are `src/lib/<Component>.test.ts` (or `<Component>.<topic>.test.ts`, such as
  `Settings.recovery.test.ts` and `Settings.readback.test.ts`). Name tests after the behaviour they pin.
- Shared test code lives in `src/test/`: `setup.ts` (runs before every file), `ipc.ts`,
  `deferred.ts`, `EventProbe.svelte` and backends for rendering components (`settings.ts`). Nothing in the app
  imports it, so it is never bundled; `svelte-check` type-checks tests like app code.
- Import `describe`/`test`/`expect` from `"vitest"` (no globals). Every file runs in happy-dom
  (a DOM in Node) with Svelte's browser runtime; logic modules stay DOM-free by design, not by
  environment. There are no Node types (`"types": []` in `tsconfig.json`): use web APIs
  (`TextEncoder`, not `Buffer`).
- One file in watch mode: `npx vitest src/lib/hotkey.test.ts`; one test by name:
  `npx vitest run -t "F13 through F24"`. Vitest only strips types; `npx svelte-check
  --tsconfig ./tsconfig.json` type-checks the tests (including `@ts-expect-error` lines).

## Mocking IPC

`src/test/ipc.ts` wraps `mockIPC` from `@tauri-apps/api/mocks`. Handlers are keyed by
command name; arguments and results are typed by `CommandMap`, which is generated from the
Rust command signatures (`src/lib/ipc-contract.generated.ts`, #347), so a handler cannot use
an argument key or result shape the backend does not have.

```ts
import { backendError, mockIpc } from "../test/ipc";
const ipc = mockIpc({
  get_settings: () => settings,
  set_language: ({ language }) => (language === "fi" ? ["Profile model reset to Auto"] : []),
  save_update_recovery: () => {
    throw "disk full"; // a Rust Err(String): the wrapper rejects with it
  },
  start_recording: () => {
    // Result<T, CommandError> (ADR 0006): rejects with { code, message, retryable }
    throw backendError("busy", "Sagascript is busy.", true);
  },
});
await runTheCodeUnderTest();
expect(ipc.calls).toEqual([{ command: "get_settings", args: {} } /* … */]);
expect(ipc.callsTo("set_language")).toEqual([{ language: "fi" }]);
ipc.handle("get_platform", () => "windows"); // change an answer mid-test
```

- Every call is recorded in order with a copy of its arguments; results and event payloads
  are copies too. Copies go through JSON with the replacer of Tauri's IPC serializer, as across
  the real boundary: an `undefined` field disappears, Svelte `$state` proxies arrive as plain
  values, a `Map` as an object and a `Uint8Array` as an array of numbers.
- A command without a handler rejects, and `setup.ts` fails the test afterwards even if the
  code under test swallowed the rejection. Never touch real settings or recordings.
- Every call to a registered command is also checked against the Rust parameters
  (`COMMAND_ARGUMENTS` in the generated file): a missing required key or a key that is no
  parameter rejects the call and fails the test the same way. A wrapper that still sends an
  argument key Rust renamed therefore fails Vitest, not only svelte-check. An `Option`
  parameter may be left out.
- A fake rejects with what its Rust command rejects with: a `backendError(code, message)` for
  `Result<T, CommandError>` (`CommandMap[C]["error"]`, `BACKEND_ERROR_COMMANDS`; every command
  since #360 part 2a), a string only for a `Result<T, String>` signature, which no command has
  now. The other shape fails the test, so a fake cannot keep an error shape Rust no longer
  sends. The code under test branches on `isBackendError(error, code)`,
  never on the message.
- A new command: regenerate `CommandMap` (`SAGASCRIPT_BLESS_CONTRACTS=1 cargo test -p
  sagascript contract` in `src-tauri/`), then add the wrapper and its row in
  `src/lib/api.test.ts`, which checks every wrapper's command name, argument object and
  result type against `CommandMap`. A registered command without a wrapper must be listed in
  `COMMANDS_WITHOUT_A_CALLER` there. Plugin commands (`plugin:dialog|open`, …) are not mapped
  yet.

## Events

```ts
const ipc = mockIpc();
await startListening(); // whatever awaits listen(...)
ipc.emit("state-changed", "recording"); // typed by EventMap; returns the listener count
expect(ipc.listenerCount("state-changed")).toBe(1);
```

`emit` throws when nothing listens, which usually means the test emitted before an
asynchronous `listen()` resolved. `EventMap` (`src/lib/events.ts`) lists the events the
frontend listens for with their payloads; `src/test/transport.test.ts` checks it against the
Rust event catalogue and its sample payloads.

`ipc.listeners()` names every registration that is still active, including the webview's own
`tauri://drag-*` events. A component that unmounted must leave none:

```ts
view.unmount();
await waitFor(() => expect(ipc.listeners()).toEqual([]));
```

Components never call `listen()`: they use `useTauriEvent(name, handler)` and
`useDragDropEvent(handler)` from `src/lib/tauri-events.svelte.ts` while they initialize, or
`subscribeTauriEvent` and `dispose()` for a listener with its own lifetime. The helpers type
the payload from `EventMap`. ESLint rejects the raw imports in `src/` (tests and the helper
excepted). `src/lib/listeners.test.ts` pins that every window unlistens;
`src/test/EventProbe.svelte` is a component for testing the helper.

## Transport contract

Payloads that cross IPC are pinned by shared fixtures ([contracts/README.md](../../contracts/README.md)):
Rust writes `contracts/transport/*.json` from real payloads, and `src/test/transport.test.ts`
parses each with a schema (`src/test/transport-schemas.ts`, built from `src/test/schema.ts`)
that `expectTypeOf` proves equal to the frontend type. Use those fixtures in tests of code
that reads a payload (as the meeting-job, transcribe-stages and overlay tests there do)
instead of hand-written payloads that may not match what Rust sends. To change a payload type:
change the TypeScript type and its schema together; svelte-check fails while they differ, and
Vitest fails while the fixture does not parse.

## Component tests

Render the component against the IPC mock and drive it the way a user does. Query by role
and accessible name, then by visible text; never by class, markup or component internals.

```ts
import { screen, waitFor, within } from "@testing-library/svelte";
import userEvent from "@testing-library/user-event";
import { renderSettings, settingsBackend, settingsValue } from "../test/settings";

const { ipc } = await renderSettings(
  settingsBackend({
    get_settings: () => settingsValue({ file_transcription_model: "pianissimo-sv" }),
    save_update_recovery: () => undefined,
    complete_update_preparation: () => undefined,
  }),
);
const user = userEvent.setup();
await user.click(screen.getByRole("button", { name: "Transcribe" }));
expect(screen.getByRole("combobox", { name: "File transcription model" })).toHaveValue("pianissimo-sv");
ipc.emit("update-preparing", "nonce-1"); // what the updater would send
await waitFor(() => expect(ipc.callsTo("complete_update_preparation")).toEqual([{ nonce: "nonce-1", error: null }]));
```

- `renderSettings()` (`src/test/settings.ts`) mounts Settings with an answer for every command
  it invokes while mounting and waits until its listeners are registered. For another
  component: `mockIpc({...})`, then `render(Component, props)`; add `mockWindows("main")`
  from `@tauri-apps/api/mocks` when it calls `getCurrentWebview()` or other window APIs.
- The jest-dom matchers (`toBeInTheDocument`, `toHaveValue`, `toBeDisabled`, …) are
  registered in `setup.ts`. After each test it unmounts what was rendered, then removes the
  IPC mock.
- Updates that depend on IPC are asynchronous: use `findBy*` or `waitFor`. To hold a command
  mid-flight, return `deferred().promise` from its handler (`src/test/deferred.ts`) and
  settle it later; `settle()` lets pending work run before asserting that nothing more
  happened.
- Spy on `console.warn` (`vi.spyOn(console, "warn").mockImplementation(() => undefined)`)
  when a test expects a warning; Vitest fails a run with an unhandled rejection.

## Lint

`npm run lint` runs ESLint (`eslint.config.js`): the recommended rules of ESLint,
typescript-eslint (including `no-explicit-any` and `no-unused-vars`) and eslint-plugin-svelte
over `src/`, and the Node scripts in `scripts/` and `tests/`. Prettier formats; ESLint does
not.

- `eslint-suppressions.json` retains existing tooling findings in `scripts/`. There are
  no component or other `src/` entries. New findings fail. Prune resolved entries with
  `npx eslint --prune-suppressions`; never run `--suppress-all`.
- Prettier includes Svelte components except `FileTranscription.svelte`, whose updater-owned
  source-text recovery test remains unchanged. Remove that one exclusion when its owner
  migrates the test. Formatting is verified separately from semantic behavior.
- Two choices in the configuration itself: `no-regex-spaces` is off for `scripts/` and
  `tests/` (the workflow tests match YAML indentation literally), and `scripts/qa-*.mjs` get
  browser globals for their `page.evaluate()` callbacks.

## Extracting logic from a component

Tests reach logic through modules, not through component source:

1. Pure decisions become functions over plain values in `src/lib/<name>.ts`
   (`overlay-state.ts`, `update-preparation.ts`, `transcription-queue.ts`).
2. Stateful flows become a class in `src/lib/<name>.svelte.ts` with `$state` fields and
   methods that call the `api.ts` wrappers; the component creates one and renders from it.
   Test it with `mockIpc` and assert on its state and on `ipc.calls`.
3. Move first, behaviour unchanged, in its own commit (ADR 0004 §4); keep the component wired
   to the moved code. Example: MeetingReview.svelte's `<script module>` became
   `src/lib/meeting-review-drafts.ts`.

Code that more than one component needs lives in shared modules (#362), each with its own
unit tests: `tauri-events.svelte.ts` (listeners, above), `models.svelte.ts` (`PIANISSIMO_ID`,
`downloadModelById`, `ModelDownloads`), `permissions.ts` (`requestAccessibility`,
`waitForAccessibility`) and `platform.ts` (`loadPlatform`, one lookup per window; the test
setup forgets it after every test). The Pianissimo id appears once in `src/`; a component
asks `downloadModelById(id)` instead of choosing the command.

## Rules for new and converted tests

1. No test reads `.svelte` or other source as text, and no regex over source.
2. No `node:vm`, `new Function`, `ts.transpileModule` or cutting functions out of components.
3. Assert behaviour: return values, state, recorded IPC calls, emitted events and (with a DOM)
   roles and visible text. Not markup, CSS or identifier names.
4. A converted test keeps every behavioural claim. The PR maps each old assertion to its new
   test, or says why it only pinned markup trivia.
5. Delete the replaced `scripts/test-*.mjs` file (and its `node-test-groups.mjs` entry) in
   the same PR.

## Remaining source-text boundary

The `ui` manifest has two entries: `test-finnish-language.mjs` retains CLI/core and
model-registry checks; `test-file-transcription-recovery.mjs` remains with the updater
owner. Native window and command-registration checks remain in `test-release-surface.mjs`:
mocked frontend IPC does not prove native behavior. This migration does not close the
updater-owned remainder of #361.

The file and meeting integration scripts, their `frontend-errors-source.mjs` loader and
loader test have been removed after their behaviors passed in production-used typed modules,
rendered components and Chromium. There are no remaining private component executions in
those migrated flows. The detailed parity map below distinguishes behavior from incidental
source identifiers, exact syntax and property declarations.

## Completed rendered migrations

| Former source or harness coverage | Rendered/module coverage now present |
|---|---|
| Settings dictionary and glossary editing | `src/lib/Settings.dictionary.test.ts` (`personal dictionary editor`) and `src/lib/Settings.glossaryScopes.test.ts` (`profile dictionary scopes`) cover Save, Discard, Stay, profile scopes, refreshes, conflicts and retries; `src/lib/Settings.languageRecovery.test.ts` covers `a language change a personal dictionary blocks`. |
| Meeting reprocessing and its proposal controls | `src/lib/MeetingReprocessing.test.ts` covers `stale selected plans cannot execute against a newer review`, `proposal replacement and discard require confirmation`, and persistence gates; `src/lib/FileTranscription.meetingReprocessing.test.ts` covers rendered reprocessing and clock behavior. |
| File transcription flow | `src/lib/FileTranscription.flow.test.ts` covers `dropped files use one run-scoped progress listener`, `Stop keeps a file running until native settlement`, and saved-option `Re-run`; `src/lib/FileTranscription.test.ts` retains the other rendered retry/result cases. Same-instance re-entry and held pre-submit cancellation use `plain-transcription-run.test.ts`; the older saved request uses `scripts/qa-rerun-selection.mjs` in Chromium. |
| Settings meeting controls and saved reviews | `src/lib/Settings.meetingFlows.test.ts` holds begin acknowledgement and terminal polling separately: diarization/context remain disabled until completed, failed or cancelled settlement. Saved-review success renders the returned transcript; picker cancellation waits for a cancelled job; errors remain visible. All saved-review cases assert no new transcription. The same suite preserves an earlier unsaved draft while switching tabs and while a later import fails; `api.test.ts` verifies exact wrapper payloads. |
| App onboarding completion | `src/App.onboarding.test.ts` renders first-start onboarding and completes it through the actual controls. Deferred completion acknowledgement gates Settings; other URL parameters survive, listeners are released, and frontend IPC never hides the window. Replaces the three App source assertions in `test-release-surface.mjs`; the log-message assertion was incidental wording. The Rust command's no-hide source check stays: mocked IPC cannot prove native window visibility. |
| Meeting polling | `src/lib/FileTranscription.meetingPolling.test.ts` covers retrying the same job after a poll error, waiting for the cancelled terminal snapshot after cancellation acknowledgement, and reporting a completed job that has no transcript. |
| Meeting clock | `src/lib/FileTranscription.meetingReprocessing.test.ts` covers `the rendered meeting clock interpolates native time and ignores a stale poll` and `reprocessing resets the same rendered meeting clock before accepting its native timing`. |
| MeetingReview actions | `src/lib/MeetingReview.actions.test.ts` covers `Rename applies`, `Discard restores`, `Save and each export button`, persistence gating and `a rejected save` recovery. |
| Finnish selectors, onboarding, overlay and frontend type | `src/lib/Settings.test.ts` (`Finnish saves through Settings` and `the frontend language contract stays limited`), `src/lib/Onboarding.test.ts` (`choosing Finnish saves` and the recommendation fallback), and `src/lib/overlay-state.test.ts` (`the English overlay names a Finnish dictation profile`) cover the rendered controls, model/readback behavior, onboarding defaults and overlay label. CLI/core Finnish checks remain in `scripts/test-finnish-language.mjs`. |

## Settings pilot: old assertion → new test

The counts in this pilot section are historical A2 measurements; they are retained for the
original mapping and are not a current suite inventory.

`scripts/test-settings-layout.mjs` (6 tests, 48 assert calls) and
`scripts/test-settings-recovery.mjs` (14 tests, 97) were removed. Their claims are tested by 49
component tests: `src/lib/Settings.test.ts` (12, layout), `src/lib/Settings.recovery.test.ts`
(33, recovery) and `src/lib/Onboarding.test.ts` (4). With them and two new IPC mock tests,
phase A2 took the Vitest suite from 215 to 266 tests.
Assertions that only pinned an identifier, a declaration or a function signature map to the
test of the behaviour that code exists for (`void recoveryRestore.catch(…)`, for example, to
the failed-read test: Vitest fails a run with an unhandled rejection).

| Old test (assert calls) | New tests |
|---|---|
| layout: build identity above the ordered tabs (9) | header test ("the build identity sits in the one header, above … tabs in that order") |
| layout: recording shortcuts, clear mode names, independent helpers (10) | "every profile offers both recording modes, each with its own shortcut and helper" |
| layout: a model per profile, no global picker (5) | "each profile has its own speech model picker, filled from the backend lineup for its language" |
| layout: onboarding explains both modes (6) | Onboarding: "the last step explains both recording modes and where to configure them" |
| layout: engine host identity, Pianissimo controls, no outdated strings (7) | header test and "… only when the host reports a version"; "Advanced offers when to prepare and when to unload Pianissimo, and saves each choice"; "… hidden when Pianissimo is unsupported"; "choosing Pianissimo for files states its platform requirement and download size"; "no tab shows outdated Pianissimo size, model or macOS version" |
| layout: Swedish lineup from the backend, one-click switch, download first, Auto label, onboarding downloads the recommendation (11) | "a profile on a retired model is offered the recommended one, downloaded before the switch"; "a downloaded recommendation switches without a download, …"; "the file picker's Auto entry names the model Auto resolves to"; "Auto shows (recommended) …"; Onboarding: the three "speech engine download" tests (Pianissimo, KB-Whisper Medium, fallback not a retired model) |
| recovery: restores every result as a completed, reviewable job; Discard (6) | "every stored draft comes back as a completed, reviewable result …"; "Discard recovered drafts deletes the stored drafts …" |
| recovery: preparation snapshots dictation, files, meetings; acknowledges (11) | "the snapshot holds the open dictation, file and meeting results, then acknowledges the nonce"; "the snapshot waits for the startup restore …"; "which dictation the snapshot keeps" (5 tests); "a snapshot over the recovery limits …"; "the serialized snapshot is size-checked …"; "a failed write is reported for the active preparation …" |
| recovery: a failed startup read blocks the update (4) | "an unreadable recovery file blocks the update and keeps the drafts" |
| recovery: result callbacks clear entries after Copy/Save (6) | "copying or saving a recovered result removes exactly that entry …"; "not a native result the user already copied …"; "not a result the user already copied" |
| recovery: cleanup removes only delivered drafts (8) | "copying or saving a recovered result removes exactly that entry …"; "cleanup rereads the stored drafts and never adds unsaved results"; "an edited recovered dictation still removes its stored original …" |
| recovery: warnings name the file and the repair path (4) | "when cleanup cannot read the drafts, Copy still succeeds and the window names the repair path"; the failed-read test |
| recovery: one queue for every recovery write (4) | "cleanup, preparation and discard write the drafts one at a time, in request order"; "Discard waits for a running cleanup …"; "a delivery that matches no stored draft writes nothing" |
| recovery: replacement restricted to the editor lineage (14) | "a newer native result may not overwrite a recovered draft it did not come from"; "the editor that stored a dictation replaces it, and delivering a later edit removes it"; "a stored draft is removed only by delivering it from the editor it was restored into" |
| recovery: delivery cleanup before stale-editor handling, native origin (12) | "delivering a native result acknowledges the native text it came from, even after edits"; "a recovered draft that is also the pending native result is acknowledged …"; "a result that arrives during Copy stays open while the delivered draft is removed"; "a pending native result the editor never showed blocks the update"; "a native result the user cleared from the editor does not block the update" |
| recovery: recovered jobs never transcribe or paste (2) | "every stored draft comes back … not transcribed or pasted again" |
| recovery: reread before any destructive write (2) | "cleanup rereads the stored drafts …" (load before save); the IPC order in the snapshot test |
| recovery: cleanup failure does not fail Copy or Save (3) | "when cleanup cannot read the drafts, Copy still succeeds …" |
| recovery: every stale nonce continuation is fenced (13) | "an aborted or superseded preparation never stores or acknowledges anything"; "… aborted while it waits behind another recovery write …"; "… aborted while its snapshot is being written is not acknowledged"; "a superseded preparation that fails leaves the newer one in charge"; "a failed write is reported for the active preparation, not after an abort" |
| recovery: the native result drains before the queued capture and read (8) | "a native result still in transit is delivered to the editor before the snapshot"; the IPC order in the snapshot test |

## Release-surface and selected-request migration (#361)

These replacements execute the components with typed IPC, or the actual Svelte select
binding in Chromium. The browser scripts are registered in `npm run test:browser` and
run in fresh contexts against synthetic IPC. They do not claim native Mac acceptance.

| Former source assertion | Executed behavior and evidence |
|---|---|
| Only Dictate, Transcribe and Settings navigation | `Settings.test.ts`: the header test asserts the complete ordered tab list. The private `activeTab` identifier was incidental. |
| Model selection outside Advanced; decoder controls inside | `Settings.test.ts`: each profile's model lineup; `Settings.releaseFlows.test.ts`: model available initially, Advanced opens/closes decoding mode, temperature fallback and VAD. |
| Ordinary overlay and dictionary controls outside Advanced | `Settings.releaseFlows.test.ts`: both controls remain present with the disclosure closed. |
| Language first, no welcome, platform-appropriate steps and local-processing copy | `Onboarding.test.ts`: portable setup opens directly on language, progresses to download/ready; macOS also visits microphone/Accessibility. |
| Automatic hidden recommendation, skip after failure and missing-engine warning | `Onboarding.test.ts`: backend-recommended downloads, download progress/readiness, hidden engine name and failed-download skip flow. |
| Missing profile engine action, model name and stable 152px layout | `Settings.downloads.test.ts` exercises download/ready/error and IPC; `qa-profile-download-layout.mjs` measures action, row and neighbor before download and at 1%/99%, requiring unchanged geometry and single-line, unclipped text. |
| Backend loading/transcribing cannot start a conflicting recording | `Settings.releaseFlows.test.ts` injects loading and idle; `Settings.events.test.ts` exercises hotkey recording and transcribing state events. |
| Platform pending and language save block duplicate submission; four languages including Finnish | `Onboarding.test.ts`: disabled Preparing/Saving clicks produce no additional IPC; four choices and Finnish aria-pressed/save/download checks. |
| Reopening Accessibility Settings does not restart prompt/poll ownership | `Onboarding.test.ts` holds the permission request, reopens once without new request/check, then grants and advances the clock: the original operation still completes and retries registration once. `api.test.ts` covers the wrapper; the native registration source check remains. |
| F03: older selected request queues behind another active job with its saved options | `qa-rerun-selection.mjs` drives the actual recent-file select. It distinguishes English/Astrid saved options from current Swedish/Bjorn options, asserts fresh run IDs and serial native submissions, and reruns a saved diarized meeting behind a plain job. Existing `FileTranscription.flow.test.ts` retains saved-option and retry wiring coverage. |

The F03 browser test is needed because happy-dom does not consistently expose a selected
option through `option:checked`, which Svelte's select binding uses. No product behavior
was changed to accommodate that test environment. The other seven private cases are
mapped below. Updater recovery source coverage is owned separately and remains unchanged.


## File and meeting harness parity

| Former claim | Executed replacement |
|---|---|
| F01 same handler re-entry during a live plain run | `plain-transcription-run.test.ts`: starts the same controller twice while submission is held, observes exactly one IPC and one progress subscription. `FileTranscription.flow.test.ts` exercises the rendered caller. |
| F02 Stop while the meeting action queue holds submission | Same module test holds `waitForMeetingActions`, cancels, then releases it: no native transcription or native cancellation, one local terminal cleanup. |
| F04 review revision and F05 poll generation invalidate late acceptance independently | `meeting-reprocessing-flow.test.ts` changes each field separately while acceptance is deferred; `apply` remains uncalled and the existing conflict error is preserved. Rendered reprocessing tests cover actual component wiring. |
| F06 interpolation, late/missing polls, exact terminal clock, stop/restart/unmount | `FileTranscription.meetingReprocessing.test.ts` drives native snapshots and fake time; interval count is one while active, zero after terminal/unmount, one after restart. |
| F07 dirty draft and F08 another busy action arise while the queue is held | `meeting-reprocessing-flow.test.ts` changes each condition after requesting execution and before releasing the queue; neither reaches execution IPC. |
| Plain/diarized routing, profile/context, keyed panels | `api.test.ts`, `Settings.meetingFlows.test.ts`, `FileTranscription.flow.test.ts` and the rerun browser script assert actual payloads and routing. A saved meeting's edited DOM node survives tab switches, a new import and that import's failure; no discard prompt or reread occurs. The old internal same-panel re-import assertion is superseded by the public result-tab workflow, which creates a new job. |
| Serialized action queue and recovery after rejection | `FileTranscription.reviewIntegration.test.ts` holds Undo, queues Plan reprocessing, observes no plan IPC until Undo rejects, then observes the plan against the retained review. The typed run/meeting helpers separately prove their held-queue gates. |
| Save/export/undo/reset and pending recovery | `FileTranscription.reviewIntegration.test.ts` tests Save and every export format through actual IPC, clearing recovery only after successful persistence. `MeetingReview.actions.test.ts` covers refusal/error and dirty/busy gates; media/diagnostic tests drive Undo/Reset callbacks and confirmation. |
| Playback, local attachment, stale attachment cleanup | `MeetingReview.media.test.ts` checks the opaque `meeting-audio` token URL, no highlight without attachment, half-open segment highlights, seek without autoplay, pause on inactive, detach on unmount, scroll-follow opt-out and playback error. Parent integration unmounts before a deferred attachment returns and asserts exactly one detach. These are boundary mocks, not OS audio acceptance. |
| Diagnostics and safe rendering | `MeetingReview.diagnostics.test.ts` compares edited versus original segments, caps overlap rows at 100, explains omitted/missing evidence and avoids calibrated confidence claims. `MeetingReview.media.test.ts` renders markup-like transcript text as text. |
| Local-only review interactions and no autoplay | Diagnostic tests spy on fetch, local storage, AudioContext and MediaRecorder while editing, attaching, seeking, saving, exporting, undoing, resetting and detaching; none is called. Media `play` is also uncalled. This proves the exercised flows, not arbitrary future code paths or native transport security. |
| Types, draft reconciliation and polling | `api.test.ts` binds every wrapper to the generated Rust signatures; `transport.test.ts` checks payloads. `meeting-review-drafts.test.ts`, `meeting-types.test.ts`, `meeting-playback.test.ts` and `meeting-job-client.test.ts` retain pure contract tests; rendered polling tests cover retry, missing transcript and terminal cancellation. |

Exact markup, function names and source spelling were incidental assertions and have no
runtime replacement. Public behaviors, data contracts, privacy claims and concurrency cases
remain explicit above. Native backend registration/window assertions and updater-owned source
coverage were retained; no claim of globally zero source inspection is made.
