# Start here

This folder is meant to be **unzipped into a cloud sandbox** (recommended) and then run with **Claude Code + Ralph loop**.

## Why a cloud sandbox?

You asked for “completely sandboxed so it cannot mess up my computer”.

The safest practical approach is:
- run Claude Code in a cloud VM / Codespace / hosted environment
- only later download the resulting repo or release artifacts

## Minimal start (recommended)

1. Open Claude Code in this folder.
2. Install the Ralph loop plugin once:

```text
/plugin install ralph-loop@claude-plugins-official
```

3. Start the loop:

```text
/ralph-loop:ralph-loop "Read PROMPT.md and execute it exactly. Re-read PROMPT.md at the start of every iteration. Do not ask the user questions. Make reasonable assumptions and document them." --max-iterations 80 --completion-promise "FLOWDICTATE_COMPLETE"
```

That’s all you should need.

## Emergency stop

```text
/ralph-loop:cancel-ralph
```

## What “Ralph loop” does (mental model)

- It repeatedly re-runs the same prompt.
- Claude sees the accumulated file + git state and keeps improving until completion.
- The loop ends only when:
  - completion promise is printed, OR
  - max iterations is reached, OR
  - you cancel it.

## If you must run locally

If you run locally on macOS:
- Prefer running inside a container (Dev Container) or a dedicated working directory
- Don’t run with elevated privileges
- Only grant Claude Code access to this one folder

Note: the resulting app will still require macOS user permissions for:
- Microphone access
- Accessibility (if we simulate paste / keystrokes)

Those permissions cannot be avoided for this class of application.
