import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { test } from "node:test";
import { fileURLToPath } from "node:url";

const generator = fileURLToPath(new URL("./generate-third-party-notices.mjs", import.meta.url));

test("Pianissimo notice requirements parse LF and CRLF", () => {
  const output = execFileSync(process.execPath, [generator, "--test-pianissimo-line-endings"], {
    encoding: "utf8",
  });
  assert.match(output, /accept LF and CRLF/);
});
