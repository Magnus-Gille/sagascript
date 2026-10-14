// ESLint (#361): TypeScript and Svelte in src/, and the Node tooling in
// scripts/ and tests/. `npm run lint` (CI: test-macos; npm run verify).
// Formatting is Prettier's job (prettier.config.js); nothing here formats.
import js from "@eslint/js";
import { defineConfig, globalIgnores } from "eslint/config";
import svelte from "eslint-plugin-svelte";
import globals from "globals";
import ts from "typescript-eslint";
import svelteConfig from "./svelte.config.js";

export default defineConfig(
  globalIgnores([
    "dist/",
    "coverage/",
    "node_modules/",
    // Separate projects with their own tooling, and generated or vendored code.
    "site/",
    "tooling/",
    "src-tauri/",
    "**/fixtures/**",
    // The frozen code-health contract (sha256-checked, see .prettierignore).
    "scripts/lib/code-health-schema.mjs",
    "scripts/lib/code-health-objective.mjs",
    "scripts/lib/code-health-policy.mjs",
    "tests/scripts/validate-code-health-objective.mjs",
    "tests/scripts/test-code-health-policy.mjs",
  ]),
  js.configs.recommended,
  ts.configs.recommended,
  svelte.configs.recommended,
  {
    // The frontend runs in the Tauri webview.
    files: ["src/**/*.{ts,svelte}"],
    languageOptions: { globals: globals.browser },
  },
  {
    files: ["**/*.svelte", "**/*.svelte.ts"],
    languageOptions: {
      parserOptions: { parser: ts.parser, extraFileExtensions: [".svelte"], svelteConfig },
    },
  },
  {
    // Listeners go through src/lib/tauri-events.svelte.ts: a raw listen() or
    // onDragDropEvent() resolves with an unlisten that components dropped (#362).
    files: ["src/**/*.{ts,svelte}"],
    ignores: ["src/lib/tauri-events.svelte.ts", "src/**/*.test.ts", "src/test/**"],
    rules: {
      "no-restricted-imports": [
        "error",
        {
          paths: [
            {
              name: "@tauri-apps/api/event",
              message: "Listen through useTauriEvent() or subscribeTauriEvent() (src/lib/tauri-events.svelte.ts), which unlisten.",
            },
            {
              name: "@tauri-apps/api/webview",
              importNames: ["getCurrentWebview"],
              message: "Use useDragDropEvent() or subscribeDragDrop() (src/lib/tauri-events.svelte.ts), which unlisten.",
            },
          ],
        },
      ],
    },
  },
  {
    // Node tooling and config files.
    files: ["scripts/**/*.{js,mjs,cjs}", "tests/**/*.{js,mjs,cjs}", "*.config.{js,ts}"],
    languageOptions: { globals: globals.node },
    rules: {
      // The workflow and YAML tests match indentation literally
      // (/^        run: /); `/ {8}/` would hide what they compare.
      "no-regex-spaces": "off",
    },
  },
  {
    // Playwright flows: the callbacks of page.evaluate() run in the browser.
    files: ["scripts/qa-*.mjs"],
    languageOptions: { globals: { ...globals.node, ...globals.browser } },
  },
);
