# Multi-file transcription regression checks

The Transcribe window queues every dropped/selected path. Each file has a keyed,
mounted result component, so result and meeting drafts survive file-tab and
Settings/Dictate navigation. Only one file runs at a time. A meeting polling error
holds the queue until Retry status check retrieves a terminal state; cancellation
also waits for the terminal snapshot. Results are session-only, with no automatic
transcript writes.

Run `npm run test:frontend` and `npm run check` for queue helpers, existing review
and reprocessing regressions, and Svelte diagnostics.

Run the full browser regression in an isolated worktree:

```sh
npm ci
npx playwright install chromium
npm run test:browser
```

Playwright is pinned in the lockfile. The runner starts and closes its own Vite
server on an available loopback port. The test starts a fresh browser context,
injects the official Tauri mocks, and uses synthetic paths/results only. It covers
three-file processing, failure continuation, appending without changing selection,
retained results, keyboard tabs, picker parity, serialized meetings, polling retry,
cancellation, and independent drafts across result and Settings/Dictate navigation.
Completed reviews remain editable while later files transcribe.

Review regressions cover separate reprocessing radio groups, attention status for
polling failures in hidden tabs, recovery navigation, and persistent live-region
outcome counts. Status is visible on all three main tabs, including after the queue
drains. No real audio, models, native file picker, permissions, or installed
application are used.

CI runs `npm run test:browser` in the required macOS test lane. Failure diagnostics
are uploaded as `transcription-browser-diagnostics`. Locally, screenshots and
failure diagnostics go to the operating system's temporary directory by default;
set `QA_OUTPUT_DIR` to choose an output folder.

For debugging against an existing Vite server, run `node
scripts/qa-transcription-tabs.mjs` with `QA_URL` pointing at that server's
`/?tab=transcribe` URL. `PLAYWRIGHT_MODULE` can override module resolution when
using an existing installed Playwright package.

Native acceptance: in a signed test build, drop three supported audio files,
check the three results against their source files, and repeat with speaker
diarization and one invalid file. This is separate from the synthetic browser
regression and was not performed for this frontend PR.

CLI parity already exists: `sagascript transcribe one.wav two.wav three.wav
--jsonl` reports each file separately and continues after individual failures.
Use `sagascript transcribe --help` for current batch options.
