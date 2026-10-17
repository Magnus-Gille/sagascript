# 0007. Settings file evolution

- Status: Accepted (2026-10-12)
- Related: [#365](https://github.com/Magnus-Gille/sagascript/issues/365),
  [#340](https://github.com/Magnus-Gille/sagascript/issues/340),
  [#372](https://github.com/Magnus-Gille/sagascript/issues/372)

## Context

`sagascript-settings.json` has had eight shapes since the first pre-release (see
`crates/sagascript-core/tests/fixtures/settings/README.md`). Their migrations were spread over
five mechanisms: key canonicalisation on every parse, a copy from legacy locations, raw-file
sniffing (`has_embedded_glossary_fields`, `needs_profile_model_migration`), ad hoc markers
(`model_lineup_version`, `engine_defaults_version`, `profile_glossary_migrated`) and fix-ups on
every load. Each one-time migration had its own locked load-and-save cycle.

A value the current `Settings` could not read (an unknown model or language, a wrong type)
loaded the whole file as defaults, and the next write saved them over everything else. The
owner reproduced this with `config set beam_size 9` on a file with `"auto_paste":
"invalid-bool"`: `show_overlay=false` was lost (#340). The hardening audit of 2026-07-04 had
deferred "preserve parseable fields" as not worth the churn. #340 then made every write preserve
the original bytes first.

## Decision

1. **`schema_version` and ordered steps.** The file records `schema_version`.
   `settings/migrations.rs` lists the migration steps in order, each a function on the
   settings object that can be tested alone and is idempotent. New steps are appended. Steps
   that need machine state (what is downloaded, whether Pianissimo runs) get it injected on
   every load whose result may be written back: the startup load and field updates. A write
   therefore never records a version with such a step still pending, which would later run
   over a value the user chose in that write. Plain loads stop before them.
2. **Where a file starts.** A file starts at its `schema_version`, lowered to just below the
   first step whose legacy record (marker or shape) it lacks. A file without `schema_version`
   starts where its markers put it. A file an older release rewrote is migrated again only
   where it regressed.
3. **Downgrade safety.** Releases up to 1.4.3 key their migrations off the legacy markers, so
   the store keeps writing them, along with complete `hotkey_profiles`/`profile_models` and no
   embedded dictionaries. A step that changes a user's value must keep a marker of its own: a
   replay after an older release rewrote the file must change nothing. Unknown keys survive
   through read-merge-write. A file from a newer release is written back with this build's
   version, so the newer release checks it again.
4. **Lenient per-field parsing.** If the object does not decode strictly, each top-level field
   that does not decode alone takes its default. A bad value nested inside a field resets the
   whole field. The rest is kept, and the reset fields are logged by name, never by value.
   `hotkey_profiles` and `profile_models` are lenient per entry: an entry that does not read
   (a language or model id from a newer release) is dropped alone, and the field counts as
   reset. The primary profile (`default`, else the first), which `hotkey` and `language`
   mirror, is never dropped: its unreadable parts take the values a file without profiles
   would give it.
5. **Preserve the bytes before any replacement** (from #340). A write over a file that does not
   decode strictly, or that the field update could not load (a migration step failed), first
   saves its exact bytes as `settings.json.bak[.N]`. The startup load never rewrites a file it
   could not load or had to repair. Only an explicit save does.

This reverses the July 2026 deferral of "preserve parseable fields" because of the data loss
reproduced in #340.

## Consequences

- One bad value no longer costs the user every other setting. The dropped value can be
  recovered from the `.bak` copy.
- A repaired file keeps its bad value on disk until the next save, and warns on every load
  until then. If it also has a migration pending, that migration runs in memory only, so every
  `store::load()` until then (each launch, each settings-watcher reload, each command or engine
  call that loads settings) takes the writer lock and runs the pending steps again. A pending
  lineup step probes the machine each time.
- The legacy markers are written forever. Removing one would make older releases re-migrate.
- Adding a migration: append a step, bump `SCHEMA_VERSION`, give it a `recorded` check, add
  or update a fixture, and regenerate the snapshots with `SAGASCRIPT_BLESS_SETTINGS=1`,
  reviewing the diff. A step that changes a value is guarded by a marker of its own: a file
  from a newer release is written back with this build's version, so `schema_version` alone
  cannot keep a step from running again.
- The store does not probe the machine: the machine state is injected (`MachineProbe`,
  `store::load_with`, `store::try_update_with`), and `crate::machine` supplies this machine's
  probes. The `settings` → `transcription` module cycle is gone (#372, part 1); only the
  re-exports of `machine::{load, update, try_update}` in `settings::store` still point upward,
  for one release.
