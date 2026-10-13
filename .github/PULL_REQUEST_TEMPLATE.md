## What

Brief description of the change.

## Why

What problem does this solve or what feature does it add?

## Testing

`npm run verify` runs the checks from CONTRIBUTING.md, "Running tests" (`npm run verify -- --list` shows them).

- [ ] `npm run verify` passes, or: the checks run with `--only`/`--skip` and why the rest do not apply
- [ ] Opt-in checks the change touches (`--only browser` for UI flows, `msrv`, `python-diarization`): list which
- [ ] Bug fix: a regression test fails before the fix and passes after (docs/adr/0004-test-strategy.md)
- [ ] Manually tested (describe how)
