# Personal glossary architecture

Sagascript keeps the persisted `initial_prompt` string as the backward-compatible
source of truth for vocabulary guidance. Existing comma- or newline-separated
terms continue to be passed to Whisper unchanged.

New entries learned through **Teach Sagascript** are stored separately per
dictation profile. At transcription time, the legacy global dictionary is
combined with only the active profile's entries. Conflicting aliases still
fail closed. This prevents a correction learned for one language/profile from
silently rewriting another profile's output.

Removing a profile makes its scoped entries inactive but does not silently
delete the user's vocabulary. Its old profile ID remains reserved, preventing
a later profile from accidentally reactivating those aliases.

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
- **Teach** records locally, keeps raw and effective transcripts ephemeral,
  compares the effective transcript with the user's correction, and applies
  only explicitly reviewed candidates to the selected profile.
- **Transcribe → Extra context for this file** remains a one-run override.
- `sagascript glossary list|add|remove|clear [--profile ID]` manages either the
  legacy global dictionary or one profile.
- `sagascript glossary suggest training.wav --corrected corrected.txt --profile ID`
  transcribes audio/video locally and prints conservative candidates without
  writing. A `.txt` or `.md` transcript is also accepted. Add `--apply` to
  atomically save the displayed candidates, or `--json` for machine-readable
  output. The selected profile must have an explicit language and its model
  must already be downloaded.
- `sagascript config set initial_prompt ...` remains supported for scripts.

Suggestion generation uses a deterministic Unicode-aware word diff. Bounded
replacements may become aliases, insertions may become hint-only entries, and
deletions, punctuation/case changes, repeated-token ambiguity, or broad
rewrites produce no alias. The backend recomputes reviewed candidates against
the latest settings while holding the cross-process settings lock. A reviewer
may edit the preferred spelling or downgrade an observed replacement to a
decoder-only hint; the observed phrase and context must still match a fresh
candidate, so a stale or fabricated UI payload cannot be persisted.

A fixed core evaluation fixture tracks candidate recall, unsafe-alias proposal
rate, and p50/p95 suggestion latency independently of Whisper transcription.

All processing stays local. Training does not mutate model weights, persist
audio, ship personal vocabulary, make cloud correction calls, or emit
telemetry.
