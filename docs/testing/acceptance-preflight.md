# Acceptance preflight

`acceptance preflight` is a read-only check of the executable process that runs
the CLI. It compares generated build metadata with the candidate revision,
checks this process's microphone and Accessibility permissions, and hashes this
process's executable. It does not inspect another running Sagascript GUI.

Run it with the full 40-character source revision expected for the candidate:

```sh
sagascript acceptance preflight --expect-revision "$CANDIDATE_SHA" --json
```

The revision must be exactly 40 hexadecimal characters. The command reports
the build version, generated source revision and dirty state, platform and
architecture, current-process observations, optional onboarding observation,
and a summary with named checks and remaining manual steps. Each observation
has a `status`, `source`, and nullable `value`; status values include
`observed`, `unknown`, `unsupported`, `not_observed`, `invalid`, and `error`.
Passing requires the generated revision to match the expected revision, a
clean-source marker, a readable SHA-256 of this executable, microphone status
`authorized`, and Accessibility status `trusted`. An explicitly supplied
onboarding fixture adds a required onboarding check.
The stable JSON shape is covered by
[`report.fixture.json`](../../src-tauri/crates/sagascript-core/src/acceptance/report.fixture.json).

Exit status is part of the result:

| Exit | Meaning |
|---|---|
| `0` | All required checks passed. |
| `1` | A required check is blocked or the report could not be written. With `--json`, the command writes the report before returning this status when stdout is available. |
| `2` | Invalid command usage, including a missing or malformed expected revision. |

Unknown, unsupported, invalid, or unavailable required observations never pass.
On Linux and Windows, microphone and Accessibility are reported as unsupported,
so the preflight blocks with exit `1`. A pass means only that the report's
required checks passed for this process. The report still lists engine/model
and physical microphone, hotkey, and paste checks as manual steps. It does not
verify code signing, notarization, installation, or release readiness. The
executable hash is SHA-256 of the file resolved through `current_exe`; it does
not hash a memory image or verify a signature or external artifact binding.

The command does not load ordinary settings, contact the engine, load a model,
start capture, request or reset TCC permissions, inspect process lists, or use
the network. Permission values are observations; they do not grant or reset
permission. Without `--settings-file`, onboarding is `not_observed` and remains
a manual step; it is not an implicit settings read. To supply an explicit test
fixture, use:

```sh
sagascript acceptance preflight \
  --expect-revision "$CANDIDATE_SHA" \
  --settings-file /path/to/synthetic-settings.json \
  --json
```

The fixture must be a JSON object containing a boolean
`has_completed_onboarding`, for example:

```json
{"has_completed_onboarding": true}
```

Only that boolean is returned. The reader ignores other JSON values, accepts at
most 1 MiB, and rejects symlinks in the fixture path or its existing path
components. It checks that the opened file still matches the inspected regular
file. Use a disposable synthetic fixture; do not point this option at a user's
ordinary settings file. On macOS, symlink aliases such as `/tmp` or `/var` may
be rejected as path components; use a direct path through the real parent
directory when needed.

To save the same report as a private receipt, add `--receipt PATH`:

```sh
sagascript acceptance preflight \
  --expect-revision "$CANDIDATE_SHA" \
  --receipt /path/to/new-receipt.json \
  --json
```

The receipt is a separate output path, not an input or a substitute for stdout.
The destination must be new; an existing file is never overwritten. The
writer rejects symlink destinations and symlinked parent components and
publishes the complete file privately (mode `0600` on Unix). If receipt writing
fails, the stdout report is marked blocked and the command exits `1`. Keep
receipts in a private location because they record build and permission
observations.

The GUI has an additive Tauri `acceptance_preflight` invoke that observes the
already-running GUI process and returns `scope: "live_gui"`. There is no
external transport for the CLI to query that command; CLI scope is always
`current_process`. Calling the invoke itself is diagnostic, but launching the
GUI first is not a read-only setup: existing startup code loads settings and
may persist `auto_paste: false` when Accessibility is not granted. Do not use a
GUI launch as evidence that a read-only CLI preflight inspected the GUI.

The report snapshot and receipt tests use synthetic observations and temporary
files. They verify serialization, blocked outcomes, exclusive receipt
publication, and unchanged existing destinations; they are not native macOS
permission, signature/artifact binding, installation, or physical-device
verification.
