# Multi-file transcription regression checks

The Transcribe window queues every dropped/selected path. Each file has a keyed,
mounted result component, so result and meeting drafts survive file-tab and
Settings/Dictate navigation. Only one file runs at a time. A meeting polling error
holds the queue until Retry status check retrieves a terminal state; cancellation
also waits for the terminal snapshot. Results are session-only, with no automatic
transcript writes.

Queued GUI imports also disable auto-paste: use Copy or Save explicitly. Live
dictation keeps its existing auto-paste preference. File-specific profile,
context and diarization choices are captured; model/language/dictionary settings
are resolved when a job starts, not frozen for the whole queue.

Run `npm run test:frontend` and `npm run check` for queue helpers, existing review
and reprocessing regressions, and Svelte diagnostics.

For the full browser regression, start Vite in an isolated worktree:

```sh
npm run dev -- --host 127.0.0.1 --port 5242
```

Then run with an installed Playwright package and its bundled browser:

```sh
PLAYWRIGHT_MODULE=/absolute/path/to/playwright/index.mjs node scripts/qa-transcription-tabs.mjs
```

If Playwright is already resolvable from Node, omit `PLAYWRIGHT_MODULE`.
`QA_URL` can override the default `http://127.0.0.1:5242/?tab=transcribe`.
The test starts a fresh browser context, injects the official Tauri mocks, and
uses synthetic paths/results only. It verifies three-file processing, failure
continuation, another drop while busy, retained results, keyboard tab navigation,
file-picker parity, serialized diarized jobs, polling retry, cancellation, and independent
unsaved meeting drafts across both kinds of tab navigation. Completed reviews stay
editable during later transcriptions; appending while busy keeps the selected tab. No real audio,
models, native file picker, permissions, or installed application are used.
Screenshots are written to `dist/sagascript-242-results.png` and
`dist/sagascript-242-meetings.png` in the worktree.

Native acceptance: in a signed test build, drop three supported audio files,
check the three results against their source files, and repeat with speaker
diarization and one invalid file. This is separate from the synthetic browser
regression and was not performed for this frontend PR.

CLI parity already exists: `sagascript transcribe one.wav two.wav three.wav
--jsonl` reports each file separately and continues after individual failures.
Use `sagascript transcribe --help` for current batch options.
