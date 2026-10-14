// Every command the Playwright mock answers must still be a registered Rust
// command (#347): the mock returns null for unknown commands, so a command
// renamed in Rust would otherwise only go stale here. COMMAND_NAMES is
// generated from `generate_handler!` (src/lib/ipc-contract.generated.ts).
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";

const read = (path) => readFileSync(new URL(path, import.meta.url), "utf8");
const registered = new Set(
  [
    ...read("../src/lib/ipc-contract.generated.ts")
      .match(/COMMAND_NAMES = \[([^\]]*)\]/)[1]
      .matchAll(/"([^"]+)"/g),
  ].map((name) => name[1]),
);
const answered = [...read("./fixtures/transcription-browser-mock.js").matchAll(/case "([^"]+)":/g)].map((label) => label[1]);

test("the Playwright mock only answers registered commands", () => {
  assert.ok(registered.has("get_settings") && answered.includes("get_settings"), "both name lists were read");
  // Plugin commands (`plugin:dialog|open`) are not Rust commands of this app.
  assert.deepEqual(
    answered.filter((name) => !name.startsWith("plugin:") && !registered.has(name)),
    [],
  );
});
