// The source of src/lib/errors.ts as plain script text, for the Node harnesses
// that run functions extracted from the Svelte components (scripts/test-*.mjs).
// Those functions call `toErrorMessage` / `isBackendError`; a harness prepends
// this text to its own, so they run the component's real helpers rather than
// a copy (ADR 0006, #360).
import { readFileSync } from "node:fs";

export const frontendErrorsSource = readFileSync(new URL("../../src/lib/errors.ts", import.meta.url), "utf8").replace(/^export /gm, "");

/**
 * `source` (a src/lib module) with its `import … from "./errors"` replaced by
 * the errors module's own code, for a harness that loads modules from `data:`
 * URLs, where relative imports cannot resolve.
 */
export function inlineFrontendErrors(source) {
  return source.replace(/^import \{[^}]*\} from "\.\/errors";\n/m, `${frontendErrorsSource}\n`);
}
