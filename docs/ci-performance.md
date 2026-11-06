# Native CI performance

PR CI builds the native Tauri release binary on macOS and Windows, then runs
platform-native CLI smoke tests against it. It intentionally does **not**
produce installers on pull requests: bundling, signing, notarization, and
artifact verification belong to the tagged release workflow.

## Baseline and target

The fully green run for PR #121 (2026-08-12) established this baseline:

| Platform | Total | Cargo cache restore | Tauri build + bundle |
| --- | ---: | ---: | ---: |
| macOS | 4m 41s | 50s | 2m 12s |
| Windows | 9m 30s | 1m 47s | 5m 08s |

The dominant repeatable cost was installer bundling in PR CI. `npx tauri build
--no-bundle` retains the native release binary while omitting that release-only
work. The next green PR run is the after measurement; GitHub Actions exposes
the step durations directly.

## Coverage map

| Check | macOS PR CI | Windows PR CI | Tagged release workflow |
| --- | --- | --- | --- |
| Native release binary | yes | yes | yes |
| Native CLI smoke tests | yes | yes | macOS installed-app smoke |
| Model-backed transcription smoke | yes | best-effort | yes |
| Installer/app bundle | no | no | macOS app + DMG |
| Signing/notarization/artifact verification | no | no | yes |

This split keeps fast feedback focused on code and platform behavior while the
release workflow remains the sole authority for distributable artifacts.

## Windows Package Candidate on pull requests

`windows-package.yml` takes about 30 minutes on two Windows runners and is not
a required check, so a pull request runs it only when it is relevant. A small
`decide` job (`scripts/ci/windows-package-gate.mjs`) lets the package build
run when the pull request carries the label `windows-package`, changes the app
version (`package.json` or `src-tauri/tauri.conf.json`, a release PR), changes
a dependency lockfile or `src-tauri/Cargo.toml`, or touches a file only Windows
packaging reads (the path list, with reasons, is in that script). Otherwise the
build job is skipped. A failed `decide` job never skips the release prebuild or
a manual run. To force a run on an open
pull request, add the label `windows-package`; removing it does nothing. The
release prebuild (`workflow_call`) and manual runs (`workflow_dispatch`) always
build both packages.
