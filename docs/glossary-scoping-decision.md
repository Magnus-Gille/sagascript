# Personal dictionary scope (#150)

Decision, 2026-09-06: reuse explicit-language profile dictionaries. Do not add
a competing language-section grammar or infer language metadata for old aliases.

## Evaluation and baseline

At6cedf58, global `initial_prompt`/`glossary.txt` and a selected profile dictionary
are concatenated before parsing. `merge = merch` in global scope therefore
authorizes the same deterministic replacement for Swedish and English live
profiles, CLI record and batch. GUI file transcription uses the global or
one-run prompt only, despite earlier documentation suggesting profile parity.
Existing profile aliases are already restricted to an explicit-language profile;
unknown/Auto profile dictionaries are ignored. Two profiles may share a language
but need different terminology, so automatically choosing by detected language
would be ambiguous. Cross-language examples also include Swedish mishearings of
English product names and a preferred technical term colliding with ordinary
English words. Model recognition and deterministic postprocessing are distinct:
this change prevents automatic replacement leakage, not all decoder hint bias.

The regression suite must first reproduce the global Swedish/English collision,
then cover all effective-source consumers with no models or audio. Existing
matcher guarantees (whole terms/phrases, Unicode case equivalence, longest-first,
non-cascading, ambiguity rejection and fragment boundaries) remain authoritative.

## Chosen contract

- Global dictionary is hint-only. Parse legacy entries and use their existing
  decoder prompt (preferred canonical terms), not their alias mappings, when
  composing an effective transcription glossary. Plain terms retain their hints.
- Only the selected known, non-Auto profile contributes deterministic aliases.
  Profiles of the same language remain independent. No selected profile means
  no deterministic aliases, including batch Auto after language detection.
  Unknown language/profile never selects a fallback alias dictionary.
- Explicit one-run prompt, when nonempty, replaces the global hint source, not
  the selected profile. It is also hint-only. To disable profile aliases, select
  no profile. Empty/whitespace one-run input keeps the normal saved global hints.
- No stored text is rewritten/deleted or automatically assigned to a language.
  Old global aliases become inactive replacements but remain visible/editable
  and continue supplying their preferred terms as hints. Users explicitly copy
  desired entries into a language profile. Existing profile dictionaries retain
  their explicit scope. Surface the global hint-only rule in UI and CLI help.
- Changing the language of a profile with a nonempty dictionary is rejected
  before mutation. This includes the legacy top-level language setter/default
  profile path. Clear or move the dictionary explicitly first. Removed-profile
  dictionaries remain retained/reserved by the existing persistence rules.
  Reset-all validates its proposed default profile before mutation and cannot
  reactivate an orphaned default dictionary or change its language implicitly.
- Conflicting aliases inside the effective selected source remain fail-closed;
  no new scope overrides or cascading rewriting. Storage is still plain global
  glossary.txt plus existing glossaries/<profile-id>.txt, not a new format.

## CLI and GUI parity

CLI already has glossary path/list/add/remove/clear/suggest --profile ID and
record/transcribe --profile ID. Preserve that inventory, reject unknown/Auto
profiles before audio/model/file work, and route effective source composition
through the shared Settings helper rather than parsing raw global text.

Settings Personal dictionary gets a scope selector: Global hints, or each
explicit-language profile. Editing a profile saves through a validated
set_profile_glossary command using the existing atomic settings store. Global
and profile text remain separately visible. Leaving an edited field saves only
the scope that was edited; changing the scope never copies text into another
dictionary. GUI saves include the original per-scope source as a compare-and-set
baseline. Concurrent changes to that same file reject the save atomically;
the typed draft is retained with visible conflict guidance. Clean fields follow
settings reloads. Unrelated dictionary changes do not block a save.

GUI file transcription gets an optional profile selector. Explicit profile
selection fixes its language and dictionary together; no profile uses the saved
default language plus hint-only global prompt, not a previously active hotkey
profile's transient language. This keeps the displayed scope and backend in
agreement. Language/model/dictionary are snapshotted before file decoding. The one-run hint field
uses the same composition rule as CLI. Record/live already pass their active
profile. Teach's existing CLI/backend profile behavior remains scoped; do not
restore the removed Teach tab.

## Verification and rollout

### Migration note for users

Your global glossary remains stored and editable. Plain terms still act as
local Whisper hints. An old global entry such as `merge = merch` now supplies
the preferred term as a hint, but no longer authorizes automatic replacement.

To enable that replacement for Swedish, copy the entry into your Swedish
profile dictionary in Settings, or use the glossary CLI with `--profile ID`.
An English profile without that alias will not apply this dictionary replacement.
Alias scope is never inferred automatically; no files are deleted and this
change introduces no model downloads or cloud processing.

Run model-free red/green collisions, original plain-hint equivalence, preserved
legacy storage, nonempty language-change guard (no partial mutation), unknown
and Auto scope, per-profile isolation, one-run prompt and GUI/CLI file parity,
Unicode/phrase/segment-boundary/ambiguity tests. Run all workspace checks plus
frontend checks and visual inspection, actual independent Claude review and
all platform CI. No model defaults, downloads, cloud or recordings are changed.

This is an intentional fail-closed change in legacy unscoped replacement
behavior. Release notes must explain migration before users test R2. Reverting
the code restores prior global replacements; dictionary text remains intact,
but that rollback reintroduces the cross-language risk. Prefer fixing forward.
