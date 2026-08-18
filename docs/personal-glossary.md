# Personal glossary architecture

Sagascript keeps the persisted `initial_prompt` string as the backward-compatible
source of truth for vocabulary guidance. Existing comma- or newline-separated
terms continue to be passed to Whisper unchanged.

An entry may optionally authorize deterministic correction:

```text
OpenRouter = open router | open vrouter
merge = merch
Cloudflare = cloud flare
```

The text to the left is the canonical output. Text to the right contains exact
aliases separated by `|`. Only entries with explicit aliases can rewrite a
transcript. This keeps arbitrary prose and legacy prompts hint-only.

## Pipeline

1. Parse the persisted dictionary in `sagascript-core`.
2. Strip aliases from the decoder prompt so Whisper sees only preferred terms.
3. Transcribe locally as before.
4. Find case-insensitive, whole-word or whole-phrase alias matches in the raw
   transcript. Resolve longest matches first against the original text.
5. Fail closed when the same alias maps to more than one canonical term.
6. Apply accepted replacements before results reach the clipboard, auto-paste,
   GUI, or CLI output.

The GUI live path, GUI file path, CLI `record`, and CLI `transcribe` all use the
same parser and exact-alias corrector. Batch `--correct-hints` retains its
additional confidence-gated one-edit correction for plain single-word hints.

## Interfaces

- **Settings → Personal dictionary** edits the persistent source.
- **Transcribe → Extra context for this file** remains a one-run override.
- `sagascript glossary list|add|remove|clear` is the CLI equivalent.
- `sagascript config set initial_prompt ...` remains supported for scripts.

All processing stays local. Sagascript does not ship personal vocabulary,
phonetic guesses, cloud correction, or telemetry.
