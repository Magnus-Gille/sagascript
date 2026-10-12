# 0001. CLI first

- Status: Accepted (practice predates this record; recorded 2026-10-12)
- Related: [#373](https://github.com/Magnus-Gille/sagascript/issues/373),
  [#364](https://github.com/Magnus-Gille/sagascript/issues/364),
  [#374](https://github.com/Magnus-Gille/sagascript/issues/374)

## Context

Sagascript is a menu-bar app, but much of its use is scripted: batch transcription,
evaluation tooling in `scripts/dictation_eval/` and `scripts/diarization_sv/`, CI smoke tests,
and agents working on the code. The rule already exists in three places:

- AGENTS.md: "every feature needs a CLI equivalent. Commands in
  `src-tauri/crates/sagascript-cli/src/` define the source-of-truth capability surface".
- CONTRIBUTING.md, "CLI-first design rule": "The GUI is a convenience layer on top of CLI
  commands."
- `main()` in `src-tauri/src/main.rs`: "The desktop binary is a full CLI (CLI-first design) —
  the GUI only launches on a bare invocation."

The code follows it structurally. The desktop binary dispatches any subcommand to
`sagascript_cli::run`; a headless build without Tauri exists for Linux and batch use
(`cargo build -p sagascript-cli --no-default-features`); even GUI recovery has CLI entry
points (`sagascript open`, `sagascript update`). CI exercises the release binary through the
CLI (`config`, `list-models`, `transcribe --json`, in `.github/workflows/ci.yml`), and
`valid_keys_count_matches_settings_struct` in `crates/sagascript-cli/src/config.rs` fails when
a `Settings` field has no `config` key.

The rule has drifted where the two surfaces implement the same intent twice. `config set
language` calls only `set_default_profile_language`, while the app's `set_language`
(`apply_language_selection` in `commands.rs`) also resets `whisper_model` and
`auto_select_model`; only some subcommands offer `--json` (#373, #364).

## Decision

1. Every user-facing capability is reachable as `sagascript <subcommand>`; the clap
   definitions in `crates/sagascript-cli/src/` are the inventory (`sagascript --help`), not a
   list in AGENTS.md.
2. New capabilities land in the CLI first or in the same PR as the GUI. Presentation-only
   behaviour (the overlay pill, tray layout) needs no command, but the settings that control
   it still get `config` keys.
3. Domain logic lives in `sagascript-core` and both surfaces call it. Surface-specific side
   effects (hotkey registration, permission prompts, windows) stay in the surface.
4. For the same intent, CLI and GUI persist the same state. A deliberate difference is
   documented next to both implementations.
5. Commands that produce data offer machine-readable output (`--json`, or `--jsonl` for
   streams) on stdout, with diagnostics on stderr.

## Consequences

- Features are testable and scriptable without a GUI, signing or TCC permissions, and the lean
  CLI builds on platforms the GUI does not target.
- Two surfaces can drift. Until #373 lands (shared operations and a parity test table),
  reviewers check parity by hand; [add-setting.md](../recipes/add-setting.md) includes the
  manual parity check.
- The app depends on the CLI crate, also for meeting pipelines, which pulls clap and friends
  into the GUI; #374 moves those pipelines to core.
- A new setting touches `config.rs` in several places; the key-count test catches omissions.
