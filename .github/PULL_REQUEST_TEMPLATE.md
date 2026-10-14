## What

Brief description of the change.

## Why

What problem does this solve or what feature does it add?

## Testing

Checks from CONTRIBUTING.md, "Running tests". Run `npm run build` first; Rust commands run from `src-tauri/`.

- [ ] `cargo fmt --all -- --check` and `cargo fmt --manifest-path engine-host/ort/Cargo.toml --all -- --check`
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` and `cargo clippy -p sagascript-cli --all-targets --no-default-features -- -D warnings`
- [ ] `cargo test --workspace` and `cargo test -p sagascript-cli --no-default-features` (a bare `cargo test` covers only the app crate)
- [ ] `npx svelte-check --tsconfig ./tsconfig.json` and `npm run test:frontend`
- [ ] Area-specific checks from CONTRIBUTING.md, if the change touches them (browser, core feature sets, Swift, ORT host, Python): list which
- [ ] Bug fix: a regression test fails before the fix and passes after (docs/adr/0004-test-strategy.md)
- [ ] Manually tested (describe how)
