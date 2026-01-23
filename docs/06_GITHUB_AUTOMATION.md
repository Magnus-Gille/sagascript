# Git + GitHub automation plan

Goal: the agent should build with strong engineering hygiene:
- repeatable builds
- CI on macOS
- automated releases
- dependency updates
- security scanning where feasible

## Git conventions

- Commit early and often.
- Use clear commit messages.
- Prefer a single main branch (MVP) unless PR workflow is available.

## GitHub bootstrap (autonomous if creds exist)

If `gh` is available and authenticated:
- `gh repo create` (private by default)
- add remote
- push
- enable Actions

If not:
- keep everything local
- create `docs/GITHUB_SETUP.md` with the exact commands needed to push later.

## CI (GitHub Actions)

### CI workflow
- Trigger: push + PR
- Runner: `macos-latest`
- Steps:
  - checkout
  - set up Xcode
  - resolve SPM dependencies
  - build (xcodebuild)
  - run unit tests
  - upload build artifacts (optional)

### Release workflow (optional)
- On tag `v*.*.*`
- Build app
- Zip `.app` (unsigned is OK for now)
- Attach artifacts to GitHub Release

## Dependabot
- Enable for:
  - GitHub Actions
  - Swift Package Manager (if supported)
- Weekly cadence.

## Code scanning (optional)
CodeQL support for Swift may be limited depending on current GitHub capabilities.
If you enable it, validate that it actually scans Swift; otherwise document limitations.

## Claude Code + GitHub tooling (optional)

If using Claude Code GitHub Action:
- Add workflow that can run Claude Code on PRs for review tasks.
- Keep it opt-in due to cost + security.

Reference:
- anthropics/claude-code-action (GitHub)

