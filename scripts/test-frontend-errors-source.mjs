// The helper that lets the Node harnesses run src/lib/errors.ts (ADR 0006,
// #360). A Windows checkout has CRLF line endings; the first version matched
// the import with an LF-only regular expression and left `./errors` in place,
// which Node cannot resolve from a data: URL (ERR_UNSUPPORTED_RESOLVE_REQUEST).
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { frontendErrorsSource, inlineFrontendErrors } from "./lib/frontend-errors-source.mjs";

const moduleSource = await readFile(new URL("../src/lib/transcribe-ui-state.ts", import.meta.url), "utf8");
const toCrlf = (text) => text.replace(/\r?\n/g, "\r\n");

test("the module that imports ./errors really does, so the cases below are meaningful", () => {
  assert.match(moduleSource, /^import \{[^}]*\} from "\.\/errors";\r?$/m);
});

test("the errors import is inlined in an LF checkout", () => {
  const inlined = inlineFrontendErrors(moduleSource.replace(/\r\n/g, "\n"));
  assert.doesNotMatch(inlined, /from "\.\/errors"/);
  assert.match(inlined, /function isBackendError\(/);
  assert.doesNotMatch(inlined, /^export function isBackendError\(/m, "the inlined helpers are not re-exported twice");
});

test("the errors import is inlined in a CRLF checkout (Windows)", () => {
  const inlined = inlineFrontendErrors(toCrlf(moduleSource));
  assert.doesNotMatch(inlined, /from "\.\/errors"/);
  assert.match(inlined, /function isBackendError\(/);
  assert.match(inlined, /function isMissingTranscribeFileError\(/, "the module's own code is kept");
});

test("the errors module's own source is inlined without its exports in a CRLF checkout too", () => {
  assert.doesNotMatch(frontendErrorsSource, /^export /m);
  assert.match(frontendErrorsSource, /function toErrorMessage\(/);
});

test("a source without the import is returned unchanged", () => {
  const plain = "export const answer = 42;\n";
  assert.equal(inlineFrontendErrors(plain), plain);
});

test("a multi-line named import is inlined too", () => {
  const multiline = 'import {\r\n  isBackendError,\r\n} from "./errors";\r\nexport const x = 1;\r\n';
  const inlined = inlineFrontendErrors(multiline);
  assert.doesNotMatch(inlined, /from "\.\/errors"/);
  assert.match(inlined, /export const x = 1;/);
});

test("an import the helper does not handle fails with a readable error instead of a resolve error", () => {
  for (const odd of ['import * as errors from "./errors";\n', 'export { isBackendError } from "./errors";\n']) {
    assert.throws(() => inlineFrontendErrors(odd), /could not inline the "\.\/errors" import/, odd);
  }
});
