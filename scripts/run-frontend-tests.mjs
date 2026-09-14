import { readdirSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { join } from "node:path";
import { fileURLToPath } from "node:url";

const scriptsDirectory = fileURLToPath(new URL(".", import.meta.url));
const tests = readdirSync(scriptsDirectory)
  .filter((name) => /^test-.*\.mjs$/.test(name))
  .sort()
  .map((name) => join(scriptsDirectory, name));

if (tests.length === 0) {
  console.error("No frontend tests found in scripts/");
  process.exit(1);
}

const result = spawnSync(process.execPath, ["--test", ...tests], {
  stdio: "inherit",
});

if (result.error) throw result.error;
if (result.status === null) {
  console.error(`Frontend tests terminated by signal ${result.signal ?? "unknown"}`);
  process.exit(1);
}
process.exit(result.status);
