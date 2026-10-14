# 0004. Test strategy

- Status: Accepted (recorded 2026-10-12)
- Related: [#361](https://github.com/Magnus-Gille/sagascript/issues/361),
  [#344](https://github.com/Magnus-Gille/sagascript/issues/344),
  [#381](https://github.com/Magnus-Gille/sagascript/issues/381),
  [#350](https://github.com/Magnus-Gille/sagascript/issues/350)

## Context

The suites are large and mostly green (the quality-sprint audit on `a69d5d1` counted about
1 490 Rust, 404 Node and 127 Python tests), but some of them lock the code rather than its
behaviour, and the coming refactors (splitting `main.rs`, `commands.rs`, `Settings.svelte`)
need tests that survive a move. Where tests live and how CI runs them is in
[ARCHITECTURE.md §5](../ARCHITECTURE.md#5-where-tests-live-and-how-ci-runs-them).

Known problems on this branch:

- Twelve `scripts/test-*.mjs` files read `.svelte` source as text and assert with regexes
  (for example `test-settings-layout.mjs`, `test-settings-recovery.mjs`,
  `test-language-glossary.mjs`); three cut functions out of a component and run them in
  `node:vm` (`test-dictionary-explicit-save.mjs` and `test-language-switch-recovery.mjs` from
  `Settings.svelte`, `test-meeting-reprocessing-integration.mjs` from
  `FileTranscription.svelte`). The audit counted 13–14 such files and 869 regex assertions. They pass when the
  component is wrong and fail on pure renames.
- The live dictation lifecycle has no fake-driven test; it is covered by helpers and CI smoke
  only (#344).
- Some tests assert on wall-clock durations and some mutate process environment (#381).
- Settings are global user state. `SAGASCRIPT_SETTINGS_PATH`
  (`crates/sagascript-core/src/settings/store.rs`) selects an exact file and disables legacy
  migration "so an isolated session can never import or mutate the user's normal application
  settings by accident"; the CLI integration tests use it.

## Decision

1. **Layers.** Rust unit tests (inline or sibling `*_tests.rs`) for logic; Rust integration
   tests for process boundaries (CLI binary, fake engine host); `node:test` for DOM-free
   frontend modules in `src/lib/*.ts`; Playwright (`scripts/qa-*.mjs`, mocked IPC) for user
   flows; Swift tests for the Core ML host; Python `unittest` for evaluation tooling; CLI
   smoke in CI for the release binary. Test each fact at the lowest layer that can see it.
2. **Behaviour, not source text.** No new test reads `.svelte` or other source as text to
   assert on its content. Extract logic into `.ts` modules and test them, or test the rendered
   component. The existing source-text tests are replaced by #361 (Vitest, testing-library,
   typed `mockIPC`); while they exist, update them only as far as a change requires.
3. **Regression test for every bug.** A bug fix starts with a test that fails before the fix
   and passes after; the PR says so. If no automated test is possible (TCC, signing, native
   focus), the PR states the manual check instead.
4. **Characterize before refactoring.** Before a behaviour-preserving refactor, add tests that
   pin current behaviour at the seam being moved, run them before and after unchanged, and
   keep pure moves in separate commits.
5. **Fixtures never touch real user state.** Child processes get
   `SAGASCRIPT_SETTINGS_PATH` pointing into a temp directory (set on the `Command`, not with
   `std::env::set_var` in a parallel test process); in-process tests use the path-taking store
   helpers (`load_from`, the `*_at` functions) on temp files. Never write the real settings,
   dictionaries, models or recordings. Fixtures are synthetic.
6. **Deterministic.** Wait on events or conditions instead of wall-clock thresholds; no
   automatic retries to hide flakes (#381).
7. **Testable seams over Tauri.** Keep decisions in plain functions that take plain values
   (`apply_language_selection`, `effective_auto_paste`, `overlay-state.ts`) so they need no
   Tauri runtime; inject environment probes (`Probe` in `crates/sagascript-cli/src/engine.rs`).

## Consequences

- Refactors in waves 2 and 3 start with characterization tests; reviewers ask for them.
- Some frontend behaviour stays thinly tested until #361 lands; Playwright covers the main
  Transcribe and update-recovery flows meanwhile.
- The local verification list stays in CONTRIBUTING.md; #350 will provide `npm run verify`
  as one entry point.
