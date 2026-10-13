# Frontend tests

How the Svelte/TypeScript frontend is tested ([ADR 0004](../adr/0004-test-strategy.md),
[#361](https://github.com/Magnus-Gille/sagascript/issues/361)). Status: Vitest, the typed IPC
mock and Prettier are in place. Component tests need a DOM environment (happy-dom) and
`@testing-library/svelte`; they come with ESLint and the Settings pilot (#361 phase A2). The
owner accepted the new license families of those packages (BSD-2-Clause, BlueOak-1.0.0) for
dev-only packages ([decision](https://github.com/Magnus-Gille/sagascript/issues/361#issuecomment-6153321272)).

| Layer | Where | Command |
|---|---|---|
| Unit (Vitest) | `src/**/*.test.ts`, next to the module | `npm run test:unit` (`npm run test:unit:coverage`: report in `coverage/`, no gate) |
| Browser flows (Playwright) | `scripts/qa-*.mjs` against `scripts/fixtures/transcription-browser-mock.js` | `npm run test:browser` |
| Legacy source-text tests | `scripts/test-*.mjs`: the `ui` group of `scripts/node-test-groups.mjs` and the component checks in `test-release-surface.mjs` (`release` group) | `npm run test:ui`, `npm run test:release`; being replaced (phase B), add none |

## Where tests live

- `src/lib/<module>.test.ts` tests `src/lib/<module>.ts` (or `.svelte.ts`); component tests
  will be `src/lib/<Component>.test.ts`. Name tests after the behaviour they pin.
- Shared test code lives in `src/test/`: `setup.ts` (runs before every file) and `ipc.ts`.
  Nothing in the app imports it, so it is never bundled; `svelte-check` type-checks tests
  like app code.
- Import `describe`/`test`/`expect` from `"vitest"` (no globals). Tests run in Node without
  a DOM for now and without Node types: use web APIs (`TextEncoder`, not `Buffer`).
- One file in watch mode: `npx vitest src/lib/hotkey.test.ts`; one test by name:
  `npx vitest run -t "F13 through F24"`. Vitest only strips types; `npx svelte-check
  --tsconfig ./tsconfig.json` type-checks the tests (including `@ts-expect-error` lines).

## Mocking IPC

`src/test/ipc.ts` wraps `mockIPC` from `@tauri-apps/api/mocks`. Handlers are keyed by
command name; arguments and results are typed from the wrappers in `src/lib/api.ts` and
`meeting-reprocessing-api.ts` (`CommandMap`, until #347 generates it from Rust).

```ts
import { mockIpc } from "../test/ipc";
const ipc = mockIpc({
  get_settings: () => settings,
  set_language: ({ language }) => (language === "fi" ? ["Profile model reset to Auto"] : []),
  save_update_recovery: () => {
    throw "disk full"; // a Rust Err(String): the wrapper rejects with it
  },
});
await runTheCodeUnderTest();
expect(ipc.calls).toEqual([{ command: "get_settings", args: {} } /* … */]);
expect(ipc.callsTo("set_language")).toEqual([{ language: "fi" }]);
ipc.handle("get_platform", () => "windows"); // change an answer mid-test
```

- Every call is recorded in order with a copy of its arguments; results are copies too, as
  across the real IPC boundary.
- A command without a handler rejects, and `setup.ts` fails the test afterwards even if the
  code under test swallowed the rejection. Never touch real settings or recordings.
- A new command: add it to `CommandMap` and `COMMANDS` in `ipc.ts` and a row to
  `src/lib/api.test.ts`, which checks every wrapper's command name and argument object.
  Plugin commands (`plugin:dialog|open`, …) are not mapped yet.

## Events

```ts
const ipc = mockIpc();
await startListening(); // whatever awaits listen(...)
ipc.emit("state-changed", "recording"); // typed by EventMap; returns the listener count
expect(ipc.listenerCount("state-changed")).toBe(1);
```

`emit` throws when nothing listens, which usually means the test emitted before an
asynchronous `listen()` resolved. `EventMap` lists the events the frontend listens for with
the payload its listeners read (TODO #347: generate from `src-tauri/src/events.rs`).

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

## Rules for new and converted tests

1. No test reads `.svelte` or other source as text, and no regex over source.
2. No `node:vm`, `new Function`, `ts.transpileModule` or cutting functions out of components.
3. Assert behaviour: return values, state, recorded IPC calls, emitted events and (with a DOM)
   roles and visible text. Not markup, CSS or identifier names.
4. A converted test keeps every behavioural claim. The PR maps each old assertion to its new
   test, or says why it only pinned markup trivia.
5. Delete the replaced `scripts/test-*.mjs` file (and its `node-test-groups.mjs` entry) in
   the same PR.

## Remaining source-text tests (phase B)

Split by the component a branch has to change, so B1–B3 can run in parallel. Assertions over
`api.ts` text (`invoke("…", { … })`) are superseded by `src/lib/api.test.ts`: delete them.

- **A2 (pilot):** `test-settings-layout.mjs`, `test-settings-recovery.mjs` (update recovery in
  Settings.svelte).
- **B1, Settings.svelte:** `test-language-glossary.mjs`, `test-language-switch-recovery.mjs`
  and `test-dictionary-explicit-save.mjs` (both `vm`: extract the dictionary editor and
  language-switch recovery into `.svelte.ts` controllers), `test-finnish-language.mjs`, the
  Settings parts of `test-meeting-integration.mjs` (file job options, "Open saved meeting")
  and of `test-release-surface.mjs` (Dictate, Advanced, profile engines), and the
  `retryLastTranscription` case of `test-file-transcription-integration.mjs`.
- **B2, FileTranscription.svelte and the meeting components:** the rest of
  `test-file-transcription-integration.mjs` (`new Function` harness: extract the plain-run and
  meeting-job controllers), `test-meeting-reprocessing-integration.mjs` (`vm`),
  `test-file-transcription-recovery.mjs`, `test-meeting-integration.mjs`,
  `test-meeting-reprocessing.mjs` (MeetingReprocessing.svelte).
- **B3, Onboarding.svelte and App.svelte:** the onboarding and window parts of
  `test-release-surface.mjs` (release group) and the Onboarding check of
  `test-finnish-language.mjs` (coordinate the file with B1).
