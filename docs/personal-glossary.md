# Personal glossary architecture

Sagascript keeps the legacy `initial_prompt` setting as a backward-compatible
interface for vocabulary guidance. Its persisted source is the human-editable
`glossary.txt` file in the XDG configuration directory. Existing embedded
comma- or newline-separated terms remain stored and continue to be passed to
Whisper as decoder hints; global aliases are no longer deterministic
replacements.

Entries reviewed through the CLI `glossary suggest` training workflow (and the
underlying backend suggestion support) are stored in
`glossaries/<profile-id>.txt`. The former GUI **Teach Sagascript** tab is not
currently exposed. At transcription time, only a selected known profile with
an explicit non-Auto language contributes deterministic aliases.
The global dictionary and a one-run prompt remain hint-only; no profile means
no deterministic aliases, including after Auto language detection. Conflicting
aliases still fail closed. This prevents a correction learned for one
language/profile from silently rewriting another profile's output.

Removing a profile makes its scoped entries inactive but does not silently
delete the user's vocabulary. Its old profile ID remains reserved, preventing
a later profile from accidentally reactivating those aliases.

A profile-scoped entry may optionally authorize deterministic correction:

```text
OpenRouter = open router | open vrouter
merge = merch
Cloudflare = cloud flare
```

The text to the left is the canonical output. Text to the right contains exact
aliases separated by `|`. Only entries with explicit aliases in the selected
explicit-language profile can rewrite a transcript. Global entries, one-run
prompt text, arbitrary prose, and legacy prompts remain hint-only.

## Pipeline

1. Parse the persisted global and profile sources in `sagascript-core`.
2. Keep global and one-run terms as decoder hints; strip aliases from the
   decoder prompt so Whisper sees preferred terms.
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

- **Settings → Personal dictionary** edits the persistent source through a
  Global hints scope or a selected explicit-language profile scope. The global
  file remains visible and editable; clearing or migrating it is not required
  to disable its aliases.
  If that same dictionary changes through the CLI while you are editing, the
  GUI rejects the stale save and preserves your draft. Copy your edits, then
  switch away and reselect the scope to load the current stored text before
  reconciling them. An unrelated profile's changes do not block your save.
- The CLI `glossary suggest` workflow records/transcribes locally, keeps raw and
  effective transcripts ephemeral, compares the effective transcript with the
  user's correction, and applies only explicitly reviewed candidates to the
  selected profile. The former GUI Teach tab is not part of the current
  surface.
- **Transcribe → Profile (optional)** selects the file's language and profile
  dictionary together. With no profile, the saved/default language is used and
  the global dictionary remains hint-only.
- **Transcribe → Extra context for this file** remains a one-run, hint-only
  override. It replaces the saved global hint source but never activates
  global aliases; a selected profile's aliases remain in scope.
- `sagascript glossary path [--profile ID]` prints the exact external file.
- `sagascript glossary list|add|remove|clear [--profile ID]` manages either the
  legacy global dictionary or one profile. Omitting `--profile` stores global
  hint terms; deterministic aliases require a known explicit-language profile.
- `sagascript glossary suggest training.wav --corrected corrected.txt --profile ID`
  transcribes audio/video locally and prints conservative candidates without
  writing. A `.txt` or `.md` transcript is also accepted. Add `--apply` to
  atomically save the displayed candidates, or `--json` for machine-readable
  output. The selected profile must have an explicit language and its model
  must already be downloaded.
- See [Configuration files](configuration.md) for the XDG layout, dotfiles,
  migration, and path precedence.
- Set `SAGASCRIPT_SETTINGS_PATH=/absolute/path/to/settings.json` to run an
  isolated CLI session against that exact settings file. Relative values resolve
  from the process working directory. While the override is active, Sagascript
  does not inspect or migrate legacy settings. This is useful for automation,
  end-to-end tests, and disposable training profiles.
- `sagascript config set initial_prompt ...` remains supported for scripts as a
  global decoder hint source; it does not activate global aliases.

The CLI exposes the complete review workflow without requiring an interactive
prompt. Run `glossary suggest` as a dry run, then either apply every displayed
candidate with `--apply` or save only selected/edited candidates explicitly:

```console
sagascript glossary add Quuxmark --alias testar --profile swedish
sagascript glossary add Loveable --profile swedish
```

The first command accepts an alias replacement; the second deliberately
downgrades a candidate to a decoder-only hint. Omitting a candidate rejects it.

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
