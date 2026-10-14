# Change recipes

Step-by-step guides for the most common changes. Each names the exact files, functions and
tests to touch and the commands to run. They describe the current code; "Planned" lines link
the issues that will change a step. Background: [ARCHITECTURE.md](../ARCHITECTURE.md);
rules and their reasons: [docs/adr/](../adr/README.md).

| Recipe | Use it when you… |
|---|---|
| [add-setting.md](add-setting.md) | add a user setting (struct field, CLI `config` key, Tauri setter, UI, tests, CLI/GUI parity) |
| [add-tauri-command.md](add-tauri-command.md) | expose a backend operation to the frontend (`#[tauri::command]`, `generate_handler!`, `api.ts`, browser mock) |
| [add-cli-subcommand.md](add-cli-subcommand.md) | add a `sagascript` subcommand (clap, `--json`, exit codes, both feature modes) |
| [add-event.md](add-event.md) | push a backend notification to the frontend (`events.rs`, emit, listener cleanup) |
| [add-engine.md](add-engine.md) | add an engine host or speech engine (protocol, test vectors, conformance, packaging) |

If a recipe is wrong or incomplete for your change, fix the recipe in the same PR.
