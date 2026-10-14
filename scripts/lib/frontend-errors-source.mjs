// The source of src/lib/errors.ts as plain script text, for the Node harnesses
// that run functions extracted from the Svelte components (scripts/test-*.mjs).
// Those functions call `toErrorMessage` / `isBackendError`; a harness prepends
// this text to its own, so they run the component's real helpers rather than
// a copy (ADR 0006, #360).
import { readFileSync } from "node:fs";

export const frontendErrorsSource = readFileSync(new URL("../../src/lib/errors.ts", import.meta.url), "utf8").replace(/^export /gm, "");

const ERRORS_IMPORT = /^import\s*\{[^}]*\}\s*from\s*["']\.\/errors["'];?[ \t]*\r?\n/m;
const ANY_ERRORS_SPECIFIER = /from\s*["']\.\/errors["']/;

/**
 * `source` (a src/lib module) with its `import { … } from "./errors"` replaced
 * by the errors module's own code, for a harness that loads modules from
 * `data:` URLs, where relative imports cannot resolve. Line endings do not
 * matter (a Windows checkout has CRLF). Throws when the import is in a form
 * this does not handle, rather than leaving it for Node to fail on.
 */
export function inlineFrontendErrors(source) {
  const inlined = source.replace(ERRORS_IMPORT, () => `${frontendErrorsSource}\n`);
  if (ANY_ERRORS_SPECIFIER.test(inlined)) {
    throw new Error(
      'frontend-errors-source: could not inline the "./errors" import; ' +
        'use `import { … } from "./errors";` (a data: URL cannot resolve relative imports)',
    );
  }
  return inlined;
}
